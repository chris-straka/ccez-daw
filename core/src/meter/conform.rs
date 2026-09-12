//! Track S-3: platform loudness conformance — presets plus one-click
//! conform on bounce.
//!
//! Teaching note: measuring tells you the number; conforming *moves* it.
//! Every preset below is just a [`LoudnessTarget`] (integrated LUFS +
//! true-peak ceiling) with a platform name on it, and every conform is
//! the same measure → gain → re-measure pipeline from [`export`] —
//! [`normalize_to_target`] for a single stem, [`normalize_mix_to_target`]
//! for the whole mix. The only new ideas are the preset table, the
//! pass/fail verdict, and the JSON report artifact written next to the
//! stems so the game importer can prove what shipped.
//!
//! Preset sources: PlayStation follows Sony ASWG-R001 (−24 LKFS, max true
//! peak −2 dBTP); console/mobile follow the Wwise mastering guidance
//! (console −24 LUFS ±2, mobile/portable −18 LUFS ±2, max peak −1 dBTP).
//! PC, Switch, and Xbox carry no published platform-holder LUFS spec, so
//! their rows are marked `approximate: true` — console/desktop convention,
//! one commit to update when a spec appears.
//!
//! No IPC or project-schema surface, so the typegen drift gate is
//! unaffected.

use crate::bounce::{BounceConfig, Stem};
use crate::meter::export::{
    gain_for_target, measure_integrated_lufs, measure_true_peak, normalize_mix_to_target,
    normalize_to_target, LoudnessError, LoudnessReport, LoudnessTarget, NormalizedBounce, Result,
};
use crate::mixer::gain_to_db;
use crate::model::Project;

/// Pass window around the integrated target (±1 LU; platform specs quote
/// ±2, so a conformed bounce sits comfortably inside).
pub const CONFORM_TOLERANCE_LU: f64 = 1.0;
/// Re-measurement slack at the ceiling (inter-sample estimates wobble).
pub const CONFORM_CEILING_SLACK_DB: f64 = 0.1;
/// Filename written next to the stems: `<stem-name>.conform.json`.
pub const REPORT_SUFFIX: &str = ".conform.json";

/// One platform row: a loudness target plus provenance.
///
/// `approximate` is true when no published platform-holder spec backs the
/// numbers (console/desktop convention instead) — surfaced in the report
/// JSON and the primer so nobody mistakes convention for certification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConformancePreset {
    /// Stable id used in report artifacts (`"playstation"`).
    pub id: &'static str,
    /// Human label (`"PlayStation (ASWG-R001)"`).
    pub label: &'static str,
    pub target: LoudnessTarget,
    /// Where the numbers came from (spec name or "convention").
    pub source: &'static str,
    pub approximate: bool,
}

impl ConformancePreset {
    const fn of(
        id: &'static str,
        label: &'static str,
        target_lufs: f64,
        ceiling_dbfs: f64,
        source: &'static str,
        approximate: bool,
    ) -> Self {
        // LoudnessTarget::new is fallible; every row below is inside
        // [-70, -4] LUFS x [-12, 0] dBFS by construction, so build direct.
        Self {
            id,
            label,
            target: LoudnessTarget {
                target_lufs,
                true_peak_ceiling_dbfs: ceiling_dbfs,
            },
            source,
            approximate,
        }
    }
}

/// PC: no platform-holder spec — desktop/streaming convention (−16 LUFS,
/// the same finish as the default bounce target) under a −1 dBFS ceiling.
pub const PRESET_PC: ConformancePreset = ConformancePreset::of(
    "pc",
    "PC (desktop convention)",
    -16.0,
    -1.0,
    "convention: desktop/streaming finish",
    true,
);
/// Switch: Nintendo publishes no LUFS spec — console convention.
pub const PRESET_SWITCH: ConformancePreset = ConformancePreset::of(
    "switch",
    "Switch (console convention)",
    -24.0,
    -1.0,
    "convention: console target (Wwise mastering guidance)",
    true,
);
/// PlayStation: Sony ASWG-R001 (−24 LKFS, max true peak −2 dBTP).
pub const PRESET_PLAYSTATION: ConformancePreset = ConformancePreset::of(
    "playstation",
    "PlayStation (ASWG-R001)",
    -24.0,
    -2.0,
    "Sony ASWG-R001: -24 LKFS, max true peak -2 dBTP",
    false,
);
/// Xbox: no separate published spec — console convention.
pub const PRESET_XBOX: ConformancePreset = ConformancePreset::of(
    "xbox",
    "Xbox (console convention)",
    -24.0,
    -1.0,
    "convention: console target (Wwise mastering guidance)",
    true,
);
/// Mobile: handheld/portable convention (−18 LUFS per Wwise guidance).
pub const PRESET_MOBILE: ConformancePreset = ConformancePreset::of(
    "mobile",
    "Mobile (portable convention)",
    -18.0,
    -1.0,
    "convention: portable target (Wwise mastering guidance)",
    true,
);

/// All five platform presets, in plan order.
pub const PRESETS: [ConformancePreset; 5] = [
    PRESET_PC,
    PRESET_SWITCH,
    PRESET_PLAYSTATION,
    PRESET_XBOX,
    PRESET_MOBILE,
];

/// Look up a preset by [`ConformancePreset::id`].
pub fn preset_by_id(id: &str) -> Option<ConformancePreset> {
    PRESETS.iter().copied().find(|p| p.id == id)
}

/// Preview the gain one-click conform would apply, without rendering.
pub fn preview_conform_gain(
    measured_lufs: f64,
    true_peak: f32,
    preset: ConformancePreset,
) -> (f32, bool) {
    gain_for_target(measured_lufs, true_peak, preset.target)
}

/// The verdict + proof written next to the stems.
///
/// `target_met` means the re-measured output sits within
/// [`CONFORM_TOLERANCE_LU`] of the preset (and the ceiling — not the
/// target — didn't set the gain); `ceiling_met` means the re-measured
/// true peak clears the ceiling within [`CONFORM_CEILING_SLACK_DB`].
/// `passed` is both. A ceiling-limited bounce *fails* the target on
/// purpose: shipping quieter than the preset is a decision, not a pass.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformanceReport {
    pub preset_id: String,
    pub preset_label: String,
    pub preset_approximate: bool,
    pub target_lufs: f64,
    pub true_peak_ceiling_dbfs: f64,
    pub measured_lufs: f64,
    pub measured_true_peak_dbfs: f64,
    pub applied_gain: f32,
    pub applied_gain_db: f64,
    pub limited_by_ceiling: bool,
    pub output_lufs: f64,
    pub output_true_peak_dbfs: f64,
    pub target_met: bool,
    pub ceiling_met: bool,
    pub passed: bool,
}

impl ConformanceReport {
    fn from_measurements(preset: ConformancePreset, report: &LoudnessReport) -> Self {
        let target_met = !report.limited_by_ceiling
            && (report.output_lufs - preset.target.target_lufs).abs() <= CONFORM_TOLERANCE_LU;
        let ceiling_met = report.output_true_peak_dbfs
            <= preset.target.true_peak_ceiling_dbfs + CONFORM_CEILING_SLACK_DB;
        Self {
            preset_id: preset.id.to_string(),
            preset_label: preset.label.to_string(),
            preset_approximate: preset.approximate,
            target_lufs: preset.target.target_lufs,
            true_peak_ceiling_dbfs: preset.target.true_peak_ceiling_dbfs,
            measured_lufs: report.measured_lufs,
            measured_true_peak_dbfs: report.measured_true_peak_dbfs,
            applied_gain: report.applied_gain,
            applied_gain_db: report.applied_gain_db,
            limited_by_ceiling: report.limited_by_ceiling,
            output_lufs: report.output_lufs,
            output_true_peak_dbfs: report.output_true_peak_dbfs,
            target_met,
            ceiling_met,
            passed: target_met && ceiling_met,
        }
    }

    /// Artifact filename for a stem: `<stem-name>.conform.json`.
    pub fn filename_for(stem_name: &str) -> String {
        format!("{stem_name}{REPORT_SUFFIX}")
    }

    /// Serialize the report (what lands next to the stems).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_json_value()).unwrap_or_else(|_| "{}".to_string())
    }

    fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "preset": self.preset_id,
            "preset_label": self.preset_label,
            "preset_approximate": self.preset_approximate,
            "target_lufs": self.target_lufs,
            "true_peak_ceiling_dbfs": self.true_peak_ceiling_dbfs,
            "measured_lufs": self.measured_lufs,
            "measured_true_peak_dbfs": self.measured_true_peak_dbfs,
            "applied_gain": self.applied_gain,
            "applied_gain_db": self.applied_gain_db,
            "limited_by_ceiling": self.limited_by_ceiling,
            "output_lufs": self.output_lufs,
            "output_true_peak_dbfs": self.output_true_peak_dbfs,
            "target_met": self.target_met,
            "ceiling_met": self.ceiling_met,
            "passed": self.passed,
        })
    }
}

/// A conformed stem: normalized audio plus its verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformedStem {
    pub stem: Stem,
    pub conformance: ConformanceReport,
}

/// One-click conform of a single rendered stem to a preset.
///
/// Unknown preset ids error cleanly (no silent fallback to another
/// platform's numbers). Silence conforms to itself: gain 1.0, and the
/// verdict reports the target as unmet rather than pretending −70 LUFS
/// silence hit −16.
pub fn conform_to_preset(stem: &Stem, preset_id: &str) -> Result<ConformedStem> {
    let preset = preset_by_id(preset_id).ok_or_else(|| {
        LoudnessError::BadTarget(format!(
            "unknown conformance preset '{preset_id}' (want one of: {})",
            PRESETS
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let (stem, loudness) = normalize_to_target(stem, preset.target);
    Ok(ConformedStem {
        stem,
        conformance: ConformanceReport::from_measurements(preset, &loudness),
    })
}

/// A conformed mix bounce: WAV bytes plus its verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformedBounce {
    pub bounce: NormalizedBounce,
    pub conformance: ConformanceReport,
}

/// One-click conform on bounce: render the mix (reference tracks excluded,
/// per [`normalize_mix_to_target`]) and finish it to the preset, reusing
/// the `meter::export` measurement throughout.
pub fn conform_mix_to_preset(
    project: &Project,
    config: &BounceConfig,
    preset_id: &str,
) -> Result<ConformedBounce> {
    let preset = preset_by_id(preset_id).ok_or_else(|| {
        LoudnessError::BadTarget(format!("unknown conformance preset '{preset_id}'"))
    })?;
    let bounce = normalize_mix_to_target(project, config, preset.target)?;
    let conformance = ConformanceReport::from_measurements(preset, &bounce.report);
    Ok(ConformedBounce {
        bounce,
        conformance,
    })
}

/// Measure-first helper the bounce panel previews with: integrated LUFS +
/// true peak of a stem, in one call.
pub fn measure_stem(stem: &Stem) -> (f64, f64) {
    (
        measure_integrated_lufs(&stem.samples, stem.sample_rate),
        gain_to_db(measure_true_peak(&stem.samples) as f64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, ClipKind, Track};

    /// A steady tone: far above the gate, honest peak, deterministic.
    fn tone_stem(sample_rate: u32, secs: f32, amp: f32) -> Stem {
        let n = (secs * sample_rate as f32) as usize;
        let samples: Vec<f32> = (0..n)
            .map(|t| amp * (t as f32 * core::f32::consts::TAU * 440.0 / sample_rate as f32).sin())
            .collect();
        Stem {
            name: "tone".to_string(),
            sample_rate,
            samples,
            loop_start: 0,
            loop_end: n as u32,
        }
    }

    fn check_preset(preset: ConformancePreset) {
        let stem = tone_stem(48_000, 5.0, 0.5);
        let out = conform_to_preset(&stem, preset.id).expect("conform");
        let c = &out.conformance;
        assert_eq!(c.preset_id, preset.id);
        assert!(
            (c.output_lufs - preset.target.target_lufs).abs() <= CONFORM_TOLERANCE_LU,
            "{}: output {} LUFS vs target {}",
            preset.id,
            c.output_lufs,
            preset.target.target_lufs
        );
        assert!(
            c.output_true_peak_dbfs <= preset.target.true_peak_ceiling_dbfs + CONFORM_CEILING_SLACK_DB,
            "{}: peak {} dBFS vs ceiling {}",
            preset.id,
            c.output_true_peak_dbfs,
            preset.target.true_peak_ceiling_dbfs
        );
        assert!(c.target_met, "{}: target_met", preset.id);
        assert!(c.ceiling_met, "{}: ceiling_met", preset.id);
        assert!(c.passed, "{}: passed", preset.id);
        // Loop points and rate survive the conform untouched.
        assert_eq!((out.stem.loop_start, out.stem.loop_end), (stem.loop_start, stem.loop_end));
        assert_eq!(out.stem.sample_rate, stem.sample_rate);
    }

    #[test]
    fn conform_to_pc_target() {
        check_preset(PRESET_PC);
    }

    #[test]
    fn conform_to_switch_target() {
        check_preset(PRESET_SWITCH);
    }

    #[test]
    fn conform_to_playstation_target() {
        check_preset(PRESET_PLAYSTATION);
    }

    #[test]
    fn conform_to_xbox_target() {
        check_preset(PRESET_XBOX);
    }

    #[test]
    fn conform_to_mobile_target() {
        check_preset(PRESET_MOBILE);
    }

    #[test]
    fn unknown_preset_errors_cleanly() {
        let stem = tone_stem(8000, 1.0, 0.5);
        assert!(conform_to_preset(&stem, "gameboy").is_err());
        let p = Project::new("p", "P");
        let cfg = BounceConfig::new(8000, 0.0, 1.0).expect("config");
        assert!(conform_mix_to_preset(&p, &cfg, "gameboy").is_err());
    }

    #[test]
    fn silence_conforms_to_itself_and_does_not_pass() {
        let stem = Stem {
            name: "empty".to_string(),
            sample_rate: 8000,
            samples: vec![0.0; 1024],
            loop_start: 0,
            loop_end: 1024,
        };
        let out = conform_to_preset(&stem, "pc").expect("conform");
        assert_eq!(out.conformance.applied_gain, 1.0);
        assert!(!out.conformance.limited_by_ceiling);
        assert!(!out.conformance.passed, "silence must not claim the target");
        assert!(out.conformance.ceiling_met, "silence clears any ceiling");
        assert!(out.stem.is_silent());
    }

    #[test]
    fn report_artifact_names_itself_and_round_trips_json() {
        assert_eq!(
            ConformanceReport::filename_for("boss_layer"),
            "boss_layer.conform.json"
        );
        let stem = tone_stem(48_000, 5.0, 0.5);
        let out = conform_to_preset(&stem, "playstation").expect("conform");
        let json = out.conformance.to_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["preset"], "playstation");
        assert_eq!(v["passed"], true);
        assert_eq!(v["preset_approximate"], false);
        let pc = conform_to_preset(&stem, "pc").expect("conform").conformance;
        let pv: serde_json::Value = serde_json::from_str(&pc.to_json()).expect("valid JSON");
        assert_eq!(pv["preset_approximate"], true);
    }

    #[test]
    fn one_click_conform_on_bounce_lands_on_target() {
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
        let cfg = BounceConfig::new(8000, 0.0, 4.0).expect("config");
        for preset in PRESETS {
            let out = conform_mix_to_preset(&p, &cfg, preset.id).expect("conform");
            assert!(out.conformance.passed, "{} must pass", preset.id);
            assert_eq!(&out.bounce.wav_bytes[0..4], b"RIFF");
        }
    }

    #[test]
    fn preview_gain_matches_the_real_conform() {
        let stem = tone_stem(48_000, 5.0, 0.5);
        let (lufs, _) = measure_stem(&stem);
        let peak = measure_true_peak(&stem.samples);
        for preset in PRESETS {
            let (gain, limited) = preview_conform_gain(lufs, peak, preset);
            let out = conform_to_preset(&stem, preset.id).expect("conform");
            assert!(
                (gain - out.conformance.applied_gain).abs() < 1e-6,
                "{}: preview {gain} vs applied {}",
                preset.id,
                out.conformance.applied_gain
            );
            assert_eq!(limited, out.conformance.limited_by_ceiling);
        }
    }
}
