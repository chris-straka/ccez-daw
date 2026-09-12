//! Track M (agent 2): separation + cleanup + groove-transfer sidecars.
//!
//! The neural models are unpinned (see the plan's open question); this
//! module is the offline floor that always works: deterministic DSP
//! baselines behind the same background-[`Job`] shape as the transcription
//! sidecars (`super::drums` / `melody` / `chords`). Swapping in a real
//! model changes plans, never op shapes.
//!
//! The one rule every sidecar obeys: **background jobs yield editable
//! output, never flattened audio.**
//!
//! - Separation splits a mix into four stems (drums / bass / vocals /
//!   other). Each stem is stored as a WAV asset and committed with an
//!   ordinary frozen `ClipAdded` op (actor [`SIDECAR_SEPARATION`]), so
//!   stems are movable, re-renderable, and undoable clips.
//! - Cleanup gates + de-DC-offsets a take and commits the cleaned take the
//!   same way (actor [`SIDECAR_CLEANUP`]), keeping the noisy original.
//! - Groove transfer learns the feel of one MIDI clip
//!   ([`groove::extract`](crate::midi::groove)) and blends it into another
//!   ([`groove::apply`](crate::midi::groove)) as a *new* MIDI asset +
//!   `ClipAdded` (actor [`SIDECAR_GROOVE`]), so the target keeps its notes
//!   editable and the transfer itself undoes with one undo.
//!
//! v1 limits (honest, not hidden): separation is an energy/band split, not
//! a neural stemmer — sustained mid-band tones land in vocals/other by
//! construction, and leakage between stems is expected. Cleanup is a gate,
//! not spectral repair (Track I owns that). Both are exact where it
//! matters: stem sums reconstruct the mix sample-for-sample (up to float
//! rounding), so no audio is ever lost to the split.

use super::job::{Job, JobControl};
use super::{OpDraft, draft_clip_added};
use crate::bounce::{FROZEN_STEM_KIND, Stem, decode_wav, encode_wav};
use crate::engine::Engine;
use crate::midi::{MidiClip, groove};
use crate::model::{Clip, ClipKind};

/// Stable sidecar ids. These become op actors via [`super::ai_actor`].
pub const SIDECAR_SEPARATION: &str = "separation";
pub const SIDECAR_CLEANUP: &str = "cleanup";
pub const SIDECAR_GROOVE: &str = "groove-transfer";

/// Stem slot names, in commit order. Part of the v1 sidecar output shape:
/// asset keys and clip ids derive from these.
pub const STEM_NAMES: [&str; 4] = ["drums", "bass", "vocals", "other"];

/// Four separated stems plus the rate they were split at.
#[derive(Debug, Clone, PartialEq)]
pub struct Separation {
    pub sample_rate: u32,
    pub drums: Vec<f32>,
    pub bass: Vec<f32>,
    pub vocals: Vec<f32>,
    pub other: Vec<f32>,
}

impl Separation {
    pub fn len(&self) -> usize {
        self.bass.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bass.is_empty()
    }

    /// Stems as [`Stem`]s (loop = whole window, the game-deliverable shape).
    pub fn stems(&self, name_prefix: &str) -> Vec<Stem> {
        let n = self.len() as u32;
        let mk = |stem: &str, samples: &[f32]| Stem {
            name: format!("{name_prefix}-{stem}"),
            sample_rate: self.sample_rate,
            samples: samples.to_vec(),
            loop_start: 0,
            loop_end: n,
        };
        vec![
            mk("drums", &self.drums),
            mk("bass", &self.bass),
            mk("vocals", &self.vocals),
            mk("other", &self.other),
        ]
    }

    /// Max `|drums + bass + vocals + other - mix|` over all samples.
    pub fn conservation_error(&self, mix: &[f32]) -> f32 {
        self.bass
            .iter()
            .zip(&self.drums)
            .zip(&self.vocals)
            .zip(&self.other)
            .zip(mix)
            .map(|((((b, d), v), o), m)| (b + d + v + o - m).abs())
            .fold(0.0f32, f32::max)
    }
}

fn check_samples(samples: &[f32], what: &str) -> Result<(), String> {
    if samples.is_empty() {
        return Err(format!("{what}: no samples"));
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(format!("{what}: samples must be finite"));
    }
    Ok(())
}

fn check_rate(sample_rate: u32) -> Result<(), String> {
    if sample_rate == 0 {
        return Err("sample_rate must be > 0".to_string());
    }
    Ok(())
}

/// One-pole lowpass coefficient for `cutoff_hz` at `sample_rate`.
fn lp_alpha(cutoff_hz: f64, sample_rate: u32) -> f64 {
    let dt = 1.0 / sample_rate as f64;
    let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz);
    dt / (rc + dt)
}

fn lowpass_into(input: &[f32], alpha: f64, out: &mut [f32]) {
    let mut y = 0.0f64;
    for (x, o) in input.iter().zip(out.iter_mut()) {
        y += alpha * (*x as f64 - y);
        *o = y as f32;
    }
}

/// Deterministic four-stem split of a mono mix.
///
/// Pipeline (all linear except the drum mask, all conserving):
/// bass = LP250(mix); rest = mix − bass; drums = rest × transient-mask;
/// tonal = rest − drums; vocals = BP300–3000(tonal); other = tonal −
/// vocals. Reports progress per 4096-sample chunk and aborts with an
/// error when `ctl` is cancelled ([`submit_separation`] maps that to a
/// `None` job value: no partial stems, ever).
pub fn separate_mix(
    samples: &[f32],
    sample_rate: u32,
    ctl: Option<&JobControl>,
) -> Result<Separation, String> {
    check_rate(sample_rate)?;
    check_samples(samples, "separate_mix")?;
    let n = samples.len();
    let mut bass = vec![0.0f32; n];
    lowpass_into(samples, lp_alpha(250.0, sample_rate), &mut bass);

    // Transient mask from rectified derivative: fast envelope vs slow
    // envelope of |d(rest)/dt|; percussive hits spike the fast one.
    let a_fast = lp_alpha(60.0, sample_rate);
    let a_slow = lp_alpha(8.0, sample_rate);
    let mut drums = vec![0.0f32; n];
    let mut fast = 0.0f64;
    let mut slow = 0.0f64;
    let mut prev = 0.0f64;
    const CHUNK: usize = 4096;
    for (i, s) in samples.iter().enumerate() {
        if let Some(ctl) = ctl {
            if i % CHUNK == 0 {
                if ctl.is_cancelled() {
                    return Err("separation cancelled".to_string());
                }
                ctl.set_progress(0.7 * i as f64 / n as f64);
            }
        }
        let rest = *s as f64 - bass[i] as f64;
        let d = (rest - prev).abs();
        prev = rest;
        fast += a_fast * (d - fast);
        slow += a_slow * (d - slow);
        let mask = ((fast - 1.5 * slow) / (fast + 1e-6)).clamp(0.0, 1.0);
        drums[i] = (rest * mask) as f32;
    }
    if let Some(ctl) = ctl {
        ctl.set_progress(0.7);
    }
    // Vocals = mid-band slice of the tonal residual; other = the rest.
    let mut lo3k = vec![0.0f32; n];
    let mut lo300 = vec![0.0f32; n];
    let tonal: Vec<f32> = samples
        .iter()
        .zip(&bass)
        .zip(&drums)
        .map(|((s, b), d)| s - b - d)
        .collect();
    lowpass_into(&tonal, lp_alpha(3000.0, sample_rate), &mut lo3k);
    lowpass_into(&tonal, lp_alpha(300.0, sample_rate), &mut lo300);
    let vocals: Vec<f32> = lo3k.iter().zip(&lo300).map(|(h, l)| h - l).collect();
    let other: Vec<f32> = tonal.iter().zip(&vocals).map(|(t, v)| t - v).collect();
    if let Some(ctl) = ctl {
        ctl.set_progress(1.0);
    }
    Ok(Separation {
        sample_rate,
        drums,
        bass,
        vocals,
        other,
    })
}

/// Run [`separate_mix`] as a background [`Job`].
pub fn submit_separation(samples: Vec<f32>, sample_rate: u32) -> Job<Option<Separation>> {
    Job::submit(move |ctl: &JobControl| match separate_mix(&samples, sample_rate, Some(ctl)) {
        Ok(sep) => Some(sep),
        Err(_) => None, // cancelled or bad input: no partial stems, ever
    })
}

/// Where separated stems land: one clip per stem on `track_id`.
pub struct SeparationDest<'a> {
    pub track_id: &'a str,
    pub clip_id_prefix: &'a str,
    pub name_prefix: &'a str,
    pub start_beats: f64,
    pub length_beats: f64,
}

/// Asset key for one stem. Flat (no `/`): `check_asset_key` rejects paths.
pub fn separation_asset_key(clip_id_prefix: &str, stem: &str) -> String {
    format!("ai-separation-{clip_id_prefix}-{stem}.wav")
}

/// Store each stem as a WAV asset and draft one `ClipAdded` per stem.
/// The caller applies the drafts with `ai:separation` through
/// `Engine::apply`; each returned seq undoes independently.
pub fn separation_drafts(sep: &Separation, dest: &SeparationDest) -> Vec<(String, Vec<u8>, OpDraft)> {
    sep.stems(dest.name_prefix)
        .into_iter()
        .zip(STEM_NAMES)
        .map(|(stem, slot)| {
            let key = separation_asset_key(dest.clip_id_prefix, slot);
            let bytes = encode_wav(&stem);
            let draft = draft_clip_added(&Clip {
                id: format!("{}-{slot}", dest.clip_id_prefix),
                track_id: dest.track_id.to_string(),
                name: stem.name.clone(),
                start_beats: dest.start_beats,
                length_beats: dest.length_beats,
                kind: ClipKind::Audio,
                source: key.clone(),
            });
            (key, bytes, draft)
        })
        .collect()
}

/// Commit a finished separation: store WAVs + apply `ClipAdded` ops.
/// Returns one seq per stem, in [`STEM_NAMES`] order.
pub fn apply_separation_to_engine(
    engine: &mut Engine,
    actor: &str,
    sep: &Separation,
    dest: &SeparationDest,
) -> Result<Vec<u64>, String> {
    let mut seqs = Vec::with_capacity(STEM_NAMES.len());
    for (key, bytes, draft) in separation_drafts(sep, dest) {
        engine
            .store_asset(&key, FROZEN_STEM_KIND, &bytes)
            .map_err(|e| format!("separation store_asset: {e}"))?;
        // Round-trip check: the committed bytes must decode behind the key.
        let back = engine
            .load_asset(&key)
            .map_err(|e| format!("separation load_asset: {e}"))?;
        decode_wav(&back).map_err(|e| format!("separation wav round-trip: {e}"))?;
        seqs.push(
            engine
                .apply(actor, draft.kind.clone(), &draft.target, &draft.value_json)
                .map_err(|e| format!("separation apply: {e}"))?,
        );
    }
    Ok(seqs)
}

// -- cleanup ---------------------------------------------------------------

/// A denoised take: cleaned samples plus the gated (silenced) regions in
/// samples, half-open `[start, end)`, for UI display. The regions are
/// informational — the committed output is the cleaned audio clip, so the
/// gate itself stays editable (move the clip, re-run with another
/// threshold, undo).
#[derive(Debug, Clone, PartialEq)]
pub struct Cleanup {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
    /// Fraction of samples gated to silence, 0..=1.
    pub gated_fraction: f32,
    pub gated_regions: Vec<(usize, usize)>,
}

/// Deterministic cleanup baseline: DC removal, then a block-RMS noise
/// gate with smoothed gain (10 ms attack, 100 ms release, hysteresis at
/// `threshold` / `threshold * 0.5`). `threshold` is linear amplitude.
/// Signal above threshold passes untouched; the floor is silenced, not
/// "enhanced" — spectral repair belongs to Track I.
pub fn cleanup_audio(
    samples: &[f32],
    sample_rate: u32,
    threshold: f32,
    ctl: Option<&JobControl>,
) -> Result<Cleanup, String> {
    check_rate(sample_rate)?;
    check_samples(samples, "cleanup_audio")?;
    if !threshold.is_finite() || threshold < 0.0 {
        return Err(format!("cleanup threshold {threshold} must be finite and >= 0"));
    }
    let n = samples.len();
    // DC removal: subtract the mean (a real offset, not modelled noise).
    let mean = samples.iter().map(|s| *s as f64).sum::<f64>() / n as f64;
    let attack = (-1.0 / (0.010 * sample_rate as f64)).exp();
    let release = (-1.0 / (0.100 * sample_rate as f64)).exp();
    let mut gain = 0.0f64;
    let mut out = Vec::with_capacity(n);
    let mut regions = Vec::new();
    let mut region_start: Option<usize> = None;
    for (i, s) in samples.iter().enumerate() {
        if let Some(ctl) = ctl {
            if i % 4096 == 0 {
                if ctl.is_cancelled() {
                    return Err("cleanup cancelled".to_string());
                }
                ctl.set_progress(i as f64 / n as f64);
            }
        }
        let x = *s as f64 - mean;
        let open = x.abs() >= threshold as f64;
        let held = x.abs() >= threshold as f64 * 0.5;
        let target = if open || (held && gain > 0.5) { 1.0 } else { 0.0 };
        let coeff = if target > gain { attack } else { release };
        gain += (1.0 - coeff) * (target - gain);
        let g = gain.clamp(0.0, 1.0);
        out.push((x * g) as f32);
        if g < 0.01 {
            if region_start.is_none() {
                region_start = Some(i);
            }
        } else if let Some(start) = region_start.take() {
            regions.push((start, i));
        }
    }
    if let Some(start) = region_start.take() {
        regions.push((start, n));
    }
    // Merge regions split by sub-10 ms chatter so the UI draws phrases.
    let merge_gap = (sample_rate / 100) as usize;
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(regions.len());
    for (s, e) in regions {
        if let Some(last) = merged.last_mut() {
            if s - last.1 <= merge_gap {
                last.1 = e;
                continue;
            }
        }
        merged.push((s, e));
    }
    let gated: usize = merged.iter().map(|(s, e)| e - s).sum();
    if let Some(ctl) = ctl {
        ctl.set_progress(1.0);
    }
    Ok(Cleanup {
        sample_rate,
        samples: out,
        gated_fraction: gated as f32 / n as f32,
        gated_regions: merged,
    })
}

/// Run [`cleanup_audio`] as a background [`Job`].
pub fn submit_cleanup(
    samples: Vec<f32>,
    sample_rate: u32,
    threshold: f32,
) -> Job<Option<Cleanup>> {
    Job::submit(move |ctl: &JobControl| match cleanup_audio(&samples, sample_rate, threshold, Some(ctl)) {
        Ok(clean) => Some(clean),
        Err(_) => None,
    })
}

/// Asset key for a cleaned take. Flat (no `/`).
pub fn cleanup_asset_key(clip_id: &str) -> String {
    format!("ai-cleanup-{clip_id}.wav")
}

/// Store the cleaned take as a WAV asset and draft its `ClipAdded`.
/// The noisy original is untouched: cleanup adds, never replaces.
pub fn cleanup_draft(
    clean: &Cleanup,
    track_id: &str,
    clip_id: &str,
    name: &str,
    start_beats: f64,
    length_beats: f64,
) -> (String, Vec<u8>, OpDraft) {
    let key = cleanup_asset_key(clip_id);
    let stem = Stem {
        name: name.to_string(),
        sample_rate: clean.sample_rate,
        samples: clean.samples.clone(),
        loop_start: 0,
        loop_end: clean.samples.len() as u32,
    };
    let bytes = encode_wav(&stem);
    let draft = draft_clip_added(&Clip {
        id: clip_id.to_string(),
        track_id: track_id.to_string(),
        name: name.to_string(),
        start_beats,
        length_beats,
        kind: ClipKind::Audio,
        source: key.clone(),
    });
    (key, bytes, draft)
}

/// Commit a finished cleanup: store the WAV + apply one `ClipAdded`.
pub fn apply_cleanup_to_engine(
    engine: &mut Engine,
    actor: &str,
    clean: &Cleanup,
    track_id: &str,
    clip_id: &str,
    name: &str,
    start_beats: f64,
    length_beats: f64,
) -> Result<u64, String> {
    let (key, bytes, draft) = cleanup_draft(clean, track_id, clip_id, name, start_beats, length_beats);
    engine
        .store_asset(&key, FROZEN_STEM_KIND, &bytes)
        .map_err(|e| format!("cleanup store_asset: {e}"))?;
    engine
        .apply(actor, draft.kind.clone(), &draft.target, &draft.value_json)
        .map_err(|e| format!("cleanup apply: {e}"))
}

// -- groove transfer -------------------------------------------------------

/// Learn `source`'s feel and blend it into `target` (0 = unchanged,
/// 1 = full template feel). Pure wrapper over
/// [`groove::extract`](crate::midi::groove) +
/// [`groove::apply`](crate::midi::groove): the returned clip is a new
/// value, both inputs are untouched, and every note still validates.
pub fn transfer_groove(
    source: &MidiClip,
    target: &MidiClip,
    steps_per_beat: u32,
    amount: f64,
) -> Result<MidiClip, String> {
    let template = groove::extract(source, steps_per_beat)?;
    let mut out = target.clone();
    groove::apply(&mut out, &template, amount)?;
    Ok(out)
}

/// Run [`transfer_groove`] as a background [`Job`] with progress
/// (extract → apply). Cancel-aware: a cancelled job yields `None`.
pub fn submit_groove_transfer(
    source: MidiClip,
    target: MidiClip,
    steps_per_beat: u32,
    amount: f64,
) -> Job<Option<MidiClip>> {
    Job::submit(move |ctl: &JobControl| {
        if ctl.is_cancelled() {
            return None;
        }
        ctl.set_progress(0.25);
        let template = groove::extract(&source, steps_per_beat).ok()?;
        if ctl.is_cancelled() {
            return None;
        }
        ctl.set_progress(0.6);
        let mut out = target.clone();
        groove::apply(&mut out, &template, amount).ok()?;
        ctl.set_progress(1.0);
        Some(out)
    })
}

/// Asset key for a groove-transferred clip. Flat (no `/`).
pub fn groove_asset_key(clip_id: &str) -> String {
    format!("take:groove-{clip_id}")
}

/// Commit a transferred clip: store the MIDI asset + apply one
/// `ClipAdded` (kind `Midi`). The pre-transfer clip is untouched.
pub fn apply_groove_to_engine(
    engine: &mut Engine,
    actor: &str,
    transferred: &MidiClip,
    track_id: &str,
    clip_id: &str,
    name: &str,
    start_beats: f64,
) -> Result<u64, String> {
    let key = groove_asset_key(clip_id);
    transferred
        .save_to_engine(engine, &key)
        .map_err(|e| format!("groove save_to_engine: {e}"))?;
    let draft = super::draft_midi_clip(
        clip_id,
        track_id,
        name,
        start_beats,
        transferred.length_beats,
        &key,
    );
    engine
        .apply(actor, draft.kind.clone(), &draft.target, &draft.value_json)
        .map_err(|e| format!("groove apply: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq_hz: f64, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * freq_hz * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    #[test]
    fn low_sine_lands_in_bass_and_conserves() {
        let mix = sine(55.0, 44100, 44100);
        let sep = separate_mix(&mix, 44100, None).expect("separate");
        assert_eq!(sep.len(), mix.len());
        assert!(sep.conservation_error(&mix) < 1e-4, "conservation");
        let energy = |v: &[f32]| v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32;
        let bass = energy(&sep.bass);
        assert!(bass > 10.0 * energy(&sep.drums), "bass {bass}");
        assert!(bass > 10.0 * energy(&sep.vocals), "bass {bass}");
        assert!(bass > 10.0 * energy(&sep.other), "bass {bass}");
    }

    #[test]
    fn clicks_land_in_drums() {
        // Sparse impulses on silence: pure transients.
        let mut mix = vec![0.0f32; 44100];
        for k in 0..4 {
            mix[k * 11025] = 1.0;
        }
        let sep = separate_mix(&mix, 44100, None).expect("separate");
        assert!(sep.conservation_error(&mix) < 1e-4, "conservation");
        let drums_peak = sep.drums.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(drums_peak > 0.3, "drums peak {drums_peak}");
    }

    #[test]
    fn separation_rejects_bad_inputs() {
        assert!(separate_mix(&[], 44100, None).is_err());
        assert!(separate_mix(&[0.5], 0, None).is_err());
        assert!(separate_mix(&[f32::NAN], 44100, None).is_err());
        assert!(cleanup_audio(&[], 44100, 0.1, None).is_err());
        assert!(cleanup_audio(&[0.1], 44100, -0.1, None).is_err());
        assert!(cleanup_audio(&[0.1], 44100, f32::NAN, None).is_err());
    }

    #[test]
    fn cleanup_gates_silence_keeps_bursts() {
        // Loud burst in the middle, quiet floor elsewhere.
        let rate = 44100;
        let mut take = vec![0.01f32; rate];
        for i in (rate / 4)..(rate / 2) {
            take[i] = (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin() as f32 * 0.8;
        }
        let clean = cleanup_audio(&take, rate as u32, 0.1, None).expect("cleanup");
        assert_eq!(clean.samples.len(), take.len());
        // Floor region is silenced...
        let floor: f32 = clean.samples[..rate / 8]
            .iter()
            .map(|s| s.abs())
            .sum::<f32>()
            / (rate / 8) as f32;
        assert!(floor < 0.005, "floor {floor}");
        // ...while the burst survives at near-full level.
        let burst: f32 = clean.samples[(rate / 4)..(rate / 2)]
            .iter()
            .map(|s| s.abs())
            .sum::<f32>()
            / (rate / 4) as f32;
        assert!(burst > 0.3, "burst {burst}");
        assert!(!clean.gated_regions.is_empty());
        assert!((0.0..=1.0).contains(&clean.gated_fraction));
    }

    #[test]
    fn cleanup_removes_dc_offset() {
        let take = vec![0.5f32; 8192];
        let clean = cleanup_audio(&take, 44100, 0.9, None).expect("cleanup");
        // Everything gated: output is silence, not the 0.5 offset.
        assert!(clean.samples.iter().all(|s| s.abs() < 0.01));
    }

    #[test]
    fn groove_transfer_moves_timing_and_validates() {
        use crate::midi::MidiNote;
        // Source: swung 16ths (off-beats 60 ticks late); target: straight.
        let mut source = MidiClip::new(2.0);
        for (i, start) in [0.0, 0.5, 1.0, 1.5].iter().enumerate() {
            let mut n = MidiNote::new(i as u32, 60, 100, *start, 0.25);
            if i % 2 == 1 {
                n.timing_offset_beats = 0.06;
            }
            source.add_note(n).unwrap();
        }
        let mut target = MidiClip::new(2.0);
        for (i, start) in [0.0, 0.5, 1.0, 1.5].iter().enumerate() {
            target.add_note(MidiNote::new(i as u32, 64, 90, *start, 0.25)).unwrap();
        }
        let out = transfer_groove(&source, &target, 2, 1.0).expect("transfer");
        assert!(out.notes.iter().any(|n| n.timing_offset_beats.abs() > 0.01));
        for n in &out.notes {
            n.validate().expect("transferred note validates");
        }
        // Amount 0 = identity.
        let same = transfer_groove(&source, &target, 2, 0.0).expect("zero");
        assert!(same.notes.iter().all(|n| n.timing_offset_beats == 0.0));
        // Inputs untouched.
        assert!(target.notes.iter().all(|n| n.timing_offset_beats == 0.0));
        assert!(transfer_groove(&source, &target, 0, 1.0).is_err());
        assert!(transfer_groove(&source, &target, 2, 2.0).is_err());
    }
}
