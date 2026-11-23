//! Track Q (agent 6): loudness-normalized bounce target.
//!
//! Teaching note: a mix that *measures* louder always *sounds* better in an
//! A/B, so exports are normalized to a stated integrated-loudness target
//! (LUFS) with a true-peak ceiling (dBFS) as a safety net. The pipeline is
//! measure → gain → re-measure: [`measure_integrated_lufs`] hears the mix
//! the way broadcast does (K-weighted, gated), [`gain_for_target`] picks
//! one linear gain that hits the target without breaching the ceiling, and
//! [`normalize_to_target`] applies it to a [`Stem`](crate::bounce::Stem).
//! [`normalize_mix_to_target`] renders the mix through
//! [`crate::bounce::render_mix`] first, so this is a bounce *target* — a new
//! way to finish a render — not a new stem codec (WAV bytes still come from
//! [`crate::bounce::encode_wav`]).
//!
//! Honest simplifications (mono DAW, v1 stems are mono):
//! - Single-channel BS.1770: K-weighting (high-shelf + high-pass biquads,
//!   coefficients solved per sample rate) with 400 ms blocks, 75 % overlap,
//!   −70 LUFS absolute gate and −10 LU relative gate. No channel weights —
//!   there is only one channel.
//! - True peak is a 4× linear-oversampling approximation, not the full
//!   BS.1770 polyphase interpolator. It over-reads sharp transients by a
//!   hair, which errs toward safety (a tighter ceiling, never a looser one).
//! - Reference tracks (`ref_*` / `[REF] `) are excluded from the mix render:
//!   they are comparison material, never shippable audio.
//!
//! No IPC or project-schema surface, so the typegen drift gate is unaffected.

use crate::bounce::{encode_wav, render_mix, BounceConfig, Stem};
use crate::mixer::{gain_to_db, is_reference_track};
use crate::model::Project;

/// Streaming-style default: −16 LUFS integrated under a −1 dBFS ceiling.
pub const DEFAULT_TARGET_LUFS: f64 = -16.0;
/// Default true-peak ceiling: −1 dBFS (codec headroom for lossy transcodes).
pub const DEFAULT_TRUE_PEAK_CEILING_DBFS: f64 = -1.0;
/// Absolute gate: blocks quieter than this never count toward integrated.
pub const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// Relative gate: blocks 10+ LU below the ungated mean are gated out.
pub const RELATIVE_GATE_LU: f64 = -10.0;
/// Loudness reported for digital silence (nothing passed the gate).
pub const SILENCE_LUFS: f64 = -70.0;
/// Oversampling factor for the true-peak approximation.
pub const TRUE_PEAK_OVERSAMPLE: usize = 4;

#[derive(Debug)]
pub enum LoudnessError {
    BadTarget(String),
    Bounce(crate::bounce::BounceError),
}

impl std::fmt::Display for LoudnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadTarget(m) => write!(f, "bad loudness target: {m}"),
            Self::Bounce(e) => write!(f, "loudness bounce: {e}"),
        }
    }
}

impl std::error::Error for LoudnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bounce(e) => Some(e),
            _ => None,
        }
    }
}

impl From<crate::bounce::BounceError> for LoudnessError {
    fn from(e: crate::bounce::BounceError) -> Self {
        Self::Bounce(e)
    }
}

pub type Result<T> = std::result::Result<T, LoudnessError>;

/// Where to finish the mix: integrated loudness plus a true-peak ceiling.
///
/// Typical presets: streaming (−16 LUFS / −1 dBFS), club/CD-style
/// (−9 LUFS / −1 dBFS), broadcast (−23 LUFS / −2 dBFS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessTarget {
    pub target_lufs: f64,
    pub true_peak_ceiling_dbfs: f64,
}

impl LoudnessTarget {
    pub fn new(target_lufs: f64, true_peak_ceiling_dbfs: f64) -> Result<Self> {
        if !target_lufs.is_finite() || target_lufs < -70.0 || target_lufs > -4.0 {
            return Err(LoudnessError::BadTarget(format!(
                "target_lufs {target_lufs} out of [-70, -4]"
            )));
        }
        if !true_peak_ceiling_dbfs.is_finite()
            || true_peak_ceiling_dbfs < -12.0
            || true_peak_ceiling_dbfs > 0.0
        {
            return Err(LoudnessError::BadTarget(format!(
                "true_peak_ceiling_dbfs {true_peak_ceiling_dbfs} out of [-12, 0]"
            )));
        }
        Ok(Self {
            target_lufs,
            true_peak_ceiling_dbfs,
        })
    }

    pub fn default_target() -> Self {
        Self {
            target_lufs: DEFAULT_TARGET_LUFS,
            true_peak_ceiling_dbfs: DEFAULT_TRUE_PEAK_CEILING_DBFS,
        }
    }
}

/// What normalization did: the measurement, the gain, and the proof.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessReport {
    pub measured_lufs: f64,
    pub measured_true_peak_dbfs: f64,
    /// Linear gain applied (1.0 = silence no-op).
    pub applied_gain: f32,
    /// Same gain in dB (0.0 = silence no-op).
    pub applied_gain_db: f64,
    /// True when the ceiling — not the target — set the gain.
    pub limited_by_ceiling: bool,
    /// Re-measured integrated loudness of the output.
    pub output_lufs: f64,
    /// Re-measured true peak of the output.
    pub output_true_peak_dbfs: f64,
}

/// A finished bounce: normalized stem, its report, and its WAV bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedBounce {
    pub stem: Stem,
    pub report: LoudnessReport,
    pub wav_bytes: Vec<u8>,
}

/// Integrated loudness in LUFS (ITU-R BS.1770-4 via `af_core::loudness`).
///
/// Too-short input (< 1 block) measures its whole body as one block.
/// Silence (nothing passes the gate) reports [`SILENCE_LUFS`].
pub fn measure_integrated_lufs(samples: &[f32], sample_rate: u32) -> f64 {
    if samples.is_empty() || sample_rate == 0 {
        return SILENCE_LUFS;
    }
    let min_block = ((0.4 * f64::from(sample_rate)).round() as usize).max(1);
    let lufs = if samples.len() >= min_block {
        af_core::loudness::measure(samples, 1, sample_rate).integrated_lufs
    } else {
        let mut kw = af_core::loudness::KWeighting::new(f64::from(sample_rate));
        let sum_sq: f64 = samples
            .iter()
            .map(|&s| {
                let y = kw.process_f32(s);
                y * y
            })
            .sum();
        af_core::loudness::energy_to_lufs(sum_sq / samples.len() as f64)
    };
    if lufs.is_finite() && lufs >= SILENCE_LUFS {
        lufs
    } else {
        SILENCE_LUFS
    }
}

/// True-peak estimate (linear): max |sample| over the raw samples and a
/// 4× linear-oversampled pass, so inter-sample overs read hot rather than
/// invisible. Returns the linear peak (≥ 0.0).
pub fn measure_true_peak(samples: &[f32]) -> f32 {
    let mut peak = 0.0f32;
    for s in samples {
        peak = peak.max(s.abs());
    }
    if samples.len() >= 2 {
        for w in samples.windows(2) {
            for k in 1..TRUE_PEAK_OVERSAMPLE {
                let t = k as f32 / TRUE_PEAK_OVERSAMPLE as f32;
                let v = (w[0] * (1.0 - t) + w[1] * t).abs();
                peak = peak.max(v);
            }
        }
    }
    peak
}

/// True peak in dBFS (silence floors at [`crate::mixer::SILENCE_DB`]).
pub fn true_peak_dbfs(samples: &[f32]) -> f64 {
    gain_to_db(measure_true_peak(samples) as f64)
}

/// Pick the linear gain that lands `measured_lufs` on the target without
/// breaching the ceiling. Returns `(gain, limited_by_ceiling)`.
///
/// Silence (at or below the absolute gate, or a zero peak) is the safe
/// no-op: gain 1.0, never limited — matching silence would divide by zero
/// and prove nothing (same rule as [`crate::mixer::match_gain_for`]).
pub fn gain_for_target(measured_lufs: f64, true_peak: f32, target: LoudnessTarget) -> (f32, bool) {
    if !measured_lufs.is_finite() || measured_lufs <= ABSOLUTE_GATE_LUFS || true_peak <= 0.0 {
        return (1.0, false);
    }
    let mut gain = 10.0f64.powf((target.target_lufs - measured_lufs) / 20.0);
    let ceiling_lin = 10.0f64.powf(target.true_peak_ceiling_dbfs / 20.0);
    let hot = true_peak as f64 * gain;
    if hot > ceiling_lin {
        gain = ceiling_lin / true_peak as f64;
        return (gain as f32, true);
    }
    (gain as f32, false)
}

/// Normalize one rendered stem to the target: measure, gain, clamp, and
/// re-measure so the report carries the proof, not the promise.
pub fn normalize_to_target(stem: &Stem, target: LoudnessTarget) -> (Stem, LoudnessReport) {
    let measured_lufs = measure_integrated_lufs(&stem.samples, stem.sample_rate);
    let peak = measure_true_peak(&stem.samples);
    let (gain, limited_by_ceiling) = gain_for_target(measured_lufs, peak, target);
    let out_samples: Vec<f32> = stem
        .samples
        .iter()
        .map(|s| (s * gain).clamp(-1.0, 1.0))
        .collect();
    let output_lufs = measure_integrated_lufs(&out_samples, stem.sample_rate);
    let output_tp = true_peak_dbfs(&out_samples);
    let out = Stem {
        name: stem.name.clone(),
        sample_rate: stem.sample_rate,
        samples: out_samples,
        loop_start: stem.loop_start,
        loop_end: stem.loop_end,
    };
    let report = LoudnessReport {
        measured_lufs,
        measured_true_peak_dbfs: gain_to_db(peak as f64),
        applied_gain: gain,
        applied_gain_db: gain_to_db(gain as f64),
        limited_by_ceiling,
        output_lufs,
        output_true_peak_dbfs: output_tp,
    };
    (out, report)
}

/// Render the mix (reference tracks excluded) and normalize it to the
/// target, returning the stem, its report, and ready-to-write WAV bytes.
pub fn normalize_mix_to_target(
    project: &Project,
    config: &BounceConfig,
    target: LoudnessTarget,
) -> Result<NormalizedBounce> {
    let mut mix_only = project.clone();
    mix_only
        .tracks
        .retain(|t| !is_reference_track(&t.id, &t.name));
    let keep: std::collections::BTreeSet<String> =
        mix_only.tracks.iter().map(|t| t.id.clone()).collect();
    mix_only.clips.retain(|c| keep.contains(&c.track_id));
    let stem = render_mix(&mix_only, config)?;
    let (stem, report) = normalize_to_target(&stem, target);
    let wav_bytes = encode_wav(&stem);
    Ok(NormalizedBounce {
        stem,
        report,
        wav_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounce::BounceConfig;
    use crate::model::{Clip, ClipKind, Track};

    fn tone_project() -> Project {
        let mut p = Project::new("p", "P");
        p.tempo = 120.0;
        p.tracks.push(Track {
            id: "trk".to_string(),
            name: "Lead".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec!["clip".to_string()],
            device_ids: vec![],
        });
        p.clips.push(Clip {
            id: "clip".to_string(),
            track_id: "trk".to_string(),
            name: "Phrase".to_string(),
            start_beats: 0.0,
            length_beats: 4.0,
            kind: ClipKind::Midi,
            source: "take:1".to_string(),
        });
        p
    }

    fn cfg() -> BounceConfig {
        BounceConfig::new(8000, 0.0, 4.0).expect("config")
    }

    #[test]
    fn normalize_to_target_lands_on_lufs() {
        let p = tone_project();
        let stem = render_mix(&p, &cfg()).expect("render");
        let before = measure_integrated_lufs(&stem.samples, stem.sample_rate);
        assert!(before.is_finite() && before > SILENCE_LUFS);
        let target = LoudnessTarget::new(-16.0, -1.0).expect("target");
        let (out, report) = normalize_to_target(&stem, target);
        assert!(!report.limited_by_ceiling);
        assert!(
            (report.output_lufs - target.target_lufs).abs() < 0.5,
            "output {} LUFS vs target {}",
            report.output_lufs,
            target.target_lufs
        );
        assert!(
            report.output_true_peak_dbfs <= target.true_peak_ceiling_dbfs + 0.1,
            "ceiling holds: {} dBFS",
            report.output_true_peak_dbfs
        );
        assert_eq!(
            (out.loop_start, out.loop_end),
            (stem.loop_start, stem.loop_end)
        );
        assert_eq!(out.sample_rate, stem.sample_rate);
    }

    #[test]
    fn hot_target_is_limited_by_the_ceiling() {
        let p = tone_project();
        let stem = render_mix(&p, &cfg()).expect("render");
        // −8 LUFS with a −9 dBFS ceiling: the target wants it louder than
        // the ceiling allows, so the ceiling must win.
        let target = LoudnessTarget::new(-8.0, -9.0).expect("target");
        let (_, report) = normalize_to_target(&stem, target);
        assert!(report.limited_by_ceiling);
        assert!(
            (report.output_true_peak_dbfs - target.true_peak_ceiling_dbfs).abs() < 0.25,
            "output peak {} dBFS vs ceiling {}",
            report.output_true_peak_dbfs,
            target.true_peak_ceiling_dbfs
        );
    }

    #[test]
    fn silence_is_the_safe_no_op() {
        let stem = Stem {
            name: "empty".to_string(),
            sample_rate: 8000,
            samples: vec![0.0; 64],
            loop_start: 0,
            loop_end: 64,
        };
        let target = LoudnessTarget::default_target();
        let (out, report) = normalize_to_target(&stem, target);
        assert_eq!(report.applied_gain, 1.0);
        assert!(!report.limited_by_ceiling);
        assert!(out.is_silent());
    }

    #[test]
    fn mix_target_excludes_reference_tracks() {
        let mut p = tone_project();
        p.tracks.push(Track {
            id: "ref_commercial".to_string(),
            name: "[REF] Commercial".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        let target = LoudnessTarget::default_target();
        let with_ref = normalize_mix_to_target(&p, &cfg(), target).expect("mix");
        p.tracks.pop();
        let without_ref = normalize_mix_to_target(&p, &cfg(), target).expect("mix");
        assert_eq!(with_ref.stem.samples, without_ref.stem.samples);
        assert_eq!(&with_ref.wav_bytes[0..4], b"RIFF");
        assert!(
            (with_ref.report.output_lufs - target.target_lufs).abs() < 0.5,
            "mix lands on target: {}",
            with_ref.report.output_lufs
        );
    }

    #[test]
    fn bad_targets_error_cleanly() {
        assert!(LoudnessTarget::new(-80.0, -1.0).is_err());
        assert!(LoudnessTarget::new(-16.0, 3.0).is_err());
        assert!(LoudnessTarget::new(f64::NAN, -1.0).is_err());
    }
}
