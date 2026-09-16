//! Groove extract / transfer.
//!
//! A [`GrooveTemplate`](crate::midi::GrooveTemplate) is the feel of one
//! clip, distilled: per-grid-step average timing offsets (late/early, in
//! beats) and per-step velocity scales. `extract` learns it from a
//! performance; `apply` blends it into another clip with an `amount`
//! 0..=1 (0 = unchanged, 1 = full template feel). Both are pure functions
//! over beats — tempo never enters, so the same template works at any BPM.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::MidiClip;

/// Feel of one clip, quantized to a grid of `steps_per_beat` steps per beat
/// (4 = 16ths at 4/4). `offsets[i]` is the mean (actual − grid) shift in
/// beats for notes nearest step `i`; `vel_scale[i]` is the mean
/// (velocity / 100.0) ratio (1.0 = no emphasis information).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrooveTemplate {
    pub steps_per_beat: u32,
    pub offsets: Vec<f64>,
    pub vel_scale: Vec<f64>,
}

impl GrooveTemplate {
    /// Flat template: no swing, no accent (identity transfer).
    pub fn flat(steps_per_beat: u32) -> Self {
        Self {
            steps_per_beat,
            offsets: vec![0.0; steps_per_beat as usize],
            vel_scale: vec![1.0; steps_per_beat as usize],
        }
    }

    fn step_count(&self) -> usize {
        self.offsets.len()
    }
}

/// Learn the feel of `clip`: bucket each unmuted note to its nearest grid
/// step (`step = round(start * steps_per_beat) mod steps_per_beat`) and
/// average timing offsets and velocity ratios per step. Empty steps stay
/// neutral (offset 0, scale 1). Requires `steps_per_beat >= 1`.
pub fn extract(clip: &MidiClip, steps_per_beat: u32) -> Result<GrooveTemplate, String> {
    if steps_per_beat < 1 {
        return Err("steps_per_beat must be >= 1".to_string());
    }
    let n = steps_per_beat as usize;
    let mut offsets = vec![0.0f64; n];
    let mut vel_scale = vec![0.0f64; n];
    let mut counts = vec![0usize; n];
    for note in clip.notes.iter().filter(|n| !n.muted) {
        let grid_pos = (note.start_beats * steps_per_beat as f64).round() as i64;
        let step = grid_pos.rem_euclid(n as i64) as usize;
        let grid = grid_pos as f64 / steps_per_beat as f64;
        offsets[step] += note.start_beats - grid + note.timing_offset_beats;
        vel_scale[step] += note.velocity as f64 / 100.0;
        counts[step] += 1;
    }
    for i in 0..n {
        if counts[i] > 0 {
            offsets[i] /= counts[i] as f64;
            vel_scale[i] /= counts[i] as f64;
        } else {
            offsets[i] = 0.0;
            vel_scale[i] = 1.0;
        }
    }
    // Clamp learned offsets to the microtiming lane so transfer can never
    // push a note past its neighbour grid line rendering.
    for o in &mut offsets {
        *o = o.clamp(-0.25, 0.25);
    }
    Ok(GrooveTemplate {
        steps_per_beat,
        offsets,
        vel_scale,
    })
}

/// Per-knob blend depths for [`apply_full`]. Each is 0..=1 (0 = that lane
/// untouched, 1 = full effect). `quantize` pulls note onsets toward the
/// template grid; `timing` adds the template's microtiming offsets;
/// `velocity` blends toward the template's per-step emphasis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ApplyParams {
    pub quantize: f64,
    pub timing: f64,
    pub velocity: f64,
}

impl ApplyParams {
    /// Single-knob blend: no quantize, timing and velocity move together.
    pub fn amount(amount: f64) -> Self {
        Self {
            quantize: 0.0,
            timing: amount,
            velocity: amount,
        }
    }

    fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("quantize", self.quantize),
            ("timing", self.timing),
            ("velocity", self.velocity),
        ] {
            if !(0.0..=1.0).contains(&v) || !v.is_finite() {
                return Err(format!("{name} {v} out of range 0..=1"));
            }
        }
        Ok(())
    }
}

/// Blend `template` into `clip` with `amount` 0..=1. Each unmuted note moves
/// `timing_offset_beats += template_offset(step) * amount` (clamped to the
/// ±0.25 microtiming lane) and scales velocity toward
/// `velocity * vel_scale(step)` by `amount` (clamped 1..=127, rounded).
/// Muted notes are untouched. Step lookup uses the note's grid position
/// *before* its own microtiming, so repeated transfers converge instead of
/// drifting.
pub fn apply(clip: &mut MidiClip, template: &GrooveTemplate, amount: f64) -> Result<(), String> {
    apply_full(clip, template, &ApplyParams::amount(amount))
}

/// Blend `template` into `clip` with separate [`ApplyParams`] depths.
///
/// Order per unmuted note: (1) `quantize` pulls the onset toward the
/// template grid (`1 / steps_per_beat` beats) inside the ±0.25 feel lane —
/// grid positions never move, only feel; (2) `timing` adds the template
/// offset; (3) `velocity` blends toward `velocity * vel_scale(step)`.
/// Muted notes are untouched. Step lookup uses `start_beats` only, so
/// repeated transfers converge instead of drifting.
pub fn apply_full(
    clip: &mut MidiClip,
    template: &GrooveTemplate,
    params: &ApplyParams,
) -> Result<(), String> {
    params.validate()?;
    if template.steps_per_beat < 1 || template.step_count() != template.steps_per_beat as usize {
        return Err("template steps inconsistent".to_string());
    }
    if template.vel_scale.len() != template.step_count() {
        return Err("template offsets/vel_scale length mismatch".to_string());
    }
    let spb = template.steps_per_beat as f64;
    let n = template.steps_per_beat as usize;
    for note in clip.notes.iter_mut().filter(|n| !n.muted) {
        let grid_pos = (note.start_beats * spb).round() as i64;
        let step = grid_pos.rem_euclid(n as i64) as usize;
        if params.quantize > 0.0 {
            let onset = note.start_beats + note.timing_offset_beats;
            let snapped = (onset * spb).round() / spb;
            let shift = (snapped - onset) * params.quantize;
            note.timing_offset_beats = (note.timing_offset_beats + shift).clamp(-0.25, 0.25);
        }
        note.timing_offset_beats =
            (note.timing_offset_beats + template.offsets[step] * params.timing).clamp(-0.25, 0.25);
        let target = note.velocity as f64 * template.vel_scale[step];
        let blended = note.velocity as f64 + (target - note.velocity as f64) * params.velocity;
        note.velocity = blended.round().clamp(1.0, 127.0) as u8;
        note.validate().map_err(|e| format!("groove transfer produced invalid note: {e}"))?;
    }
    Ok(())
}

/// Named pool of groove templates: the user-facing groove library.
///
/// `extract` learns a template from a clip; the pool files it under a name
/// for later transfer. Names are non-empty trimmed strings; inserting under
/// an existing name replaces that entry. Serializes as plain JSON so a host
/// can persist it beside the project without touching the frozen schema.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroovePool {
    entries: BTreeMap<String, GrooveTemplate>,
}

impl GroovePool {
    pub fn new() -> Self {
        Self::default()
    }

    /// File `template` under `name` (replaces any entry already there).
    pub fn insert(&mut self, name: &str, template: GrooveTemplate) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("groove name must be non-empty".to_string());
        }
        if template.steps_per_beat < 1
            || template.step_count() != template.steps_per_beat as usize
            || template.vel_scale.len() != template.step_count()
        {
            return Err("template steps inconsistent".to_string());
        }
        self.entries.insert(name.to_string(), template);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&GrooveTemplate> {
        self.entries.get(name.trim())
    }

    /// Remove by name. Returns `true` when something was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.entries.remove(name.trim()).is_some()
    }

    /// Entry names in sort order.
    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("groove pool encode: {e}"))
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("groove pool decode: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiNote;

    #[test]
    fn extract_rejects_zero_resolution() {
        let clip = MidiClip::new(4.0);
        assert!(extract(&clip, 0).is_err());
    }

    #[test]
    fn apply_rejects_bad_amount() {
        let mut clip = MidiClip::new(4.0);
        let t = GrooveTemplate::flat(4);
        assert!(apply(&mut clip, &t, 2.0).is_err());
        assert!(apply(&mut clip, &t, f64::NAN).is_err());
        assert!(
            apply_full(
                &mut clip,
                &t,
                &ApplyParams {
                    quantize: 0.0,
                    timing: 0.0,
                    velocity: 2.0
                },
            )
            .is_err()
        );
    }

    /// Performance clip: step 1 (of 4) played late (+0.06) and loud (120),
    /// everything else on-grid at velocity 100.
    fn swung_clip() -> MidiClip {
        let mut clip = MidiClip::new(4.0);
        for (i, start) in [0.0, 0.25, 0.5, 0.75].iter().enumerate() {
            let mut n = MidiNote::new(i as u32 + 1, 60, 100, *start, 0.2);
            if i == 1 {
                n.velocity = 120;
                n.timing_offset_beats = 0.06;
            }
            clip.add_note(n).unwrap();
        }
        clip
    }

    #[test]
    fn extract_learns_per_step_timing_and_velocity() {
        let t = extract(&swung_clip(), 4).unwrap();
        assert_eq!(t.steps_per_beat, 4);
        assert_eq!(t.offsets.len(), 4);
        // Step 1 caught the late loud note; other steps stay neutral.
        assert!((t.offsets[1] - 0.06).abs() < 1e-12, "{}", t.offsets[1]);
        assert!((t.vel_scale[1] - 1.2).abs() < 1e-12, "{}", t.vel_scale[1]);
        for i in [0, 2, 3] {
            assert_eq!(t.offsets[i], 0.0);
            assert_eq!(t.vel_scale[i], 1.0);
        }
    }

    #[test]
    fn extract_ignores_muted_notes_and_empty_steps() {
        let mut clip = swung_clip();
        let mut loud = MidiNote::new(9, 72, 127, 1.5, 0.2);
        loud.muted = true;
        loud.timing_offset_beats = 0.25;
        clip.add_note(loud).unwrap();
        // Bar 1.5 step (1.5*4=6 mod 4 = step 2) is empty apart from the mute.
        let t = extract(&clip, 4).unwrap();
        assert_eq!(t.offsets[2], 0.0);
        assert_eq!(t.vel_scale[2], 1.0);
    }

    #[test]
    fn apply_zero_is_identity_and_full_transfers_feel() {
        let template = extract(&swung_clip(), 4).unwrap();
        let mut clip = MidiClip::new(4.0);
        for (i, start) in [0.0, 0.25, 0.5, 0.75].iter().enumerate() {
            clip.add_note(MidiNote::new(i as u32 + 1, 60, 100, *start, 0.2))
                .unwrap();
        }
        let mut untouched = clip.clone();
        apply(&mut untouched, &template, 0.0).unwrap();
        assert_eq!(untouched, clip);
        apply(&mut clip, &template, 1.0).unwrap();
        assert!((clip.notes[1].timing_offset_beats - 0.06).abs() < 1e-12);
        assert_eq!(clip.notes[1].velocity, 120);
        assert_eq!(clip.notes[0].timing_offset_beats, 0.0);
        assert_eq!(clip.notes[0].velocity, 100);
    }

    #[test]
    fn apply_full_splits_timing_and_velocity_depths() {
        let template = extract(&swung_clip(), 4).unwrap();
        let fresh = || {
            let mut clip = MidiClip::new(4.0);
            for (i, start) in [0.0, 0.25, 0.5, 0.75].iter().enumerate() {
                clip.add_note(MidiNote::new(i as u32 + 1, 60, 100, *start, 0.2))
                    .unwrap();
            }
            clip
        };
        // Timing only: offsets move, velocities stay.
        let mut clip = fresh();
        apply_full(
            &mut clip,
            &template,
            &ApplyParams {
                quantize: 0.0,
                timing: 1.0,
                velocity: 0.0,
            },
        )
        .unwrap();
        assert!((clip.notes[1].timing_offset_beats - 0.06).abs() < 1e-12);
        assert_eq!(clip.notes[1].velocity, 100);
        // Velocity only: emphasis moves, timing stays.
        let mut clip = fresh();
        apply_full(
            &mut clip,
            &template,
            &ApplyParams {
                quantize: 0.0,
                timing: 0.0,
                velocity: 0.5,
            },
        )
        .unwrap();
        assert_eq!(clip.notes[1].timing_offset_beats, 0.0);
        assert_eq!(clip.notes[1].velocity, 110);
    }

    #[test]
    fn apply_full_quantize_pulls_onsets_to_grid_in_lane() {
        let flat = GrooveTemplate::flat(4);
        let mut clip = MidiClip::new(4.0);
        let mut early = MidiNote::new(1, 60, 100, 1.0, 0.2);
        early.timing_offset_beats = -0.08;
        clip.add_note(early).unwrap();
        apply_full(
            &mut clip,
            &flat,
            &ApplyParams {
                quantize: 1.0,
                timing: 0.0,
                velocity: 0.0,
            },
        )
        .unwrap();
        assert!(clip.notes[0].timing_offset_beats.abs() < 1e-12);
        assert_eq!(clip.notes[0].start_beats, 1.0);
    }

    #[test]
    fn apply_clamps_to_the_microtiming_lane_and_skips_muted() {
        let mut template = GrooveTemplate::flat(2);
        template.offsets = vec![0.25, -0.25];
        let mut clip = MidiClip::new(4.0);
        let mut late = MidiNote::new(1, 60, 100, 0.0, 0.2);
        late.timing_offset_beats = 0.2;
        clip.add_note(late).unwrap();
        let mut quiet = MidiNote::new(2, 64, 40, 0.5, 0.2);
        quiet.muted = true;
        clip.add_note(quiet).unwrap();
        apply(&mut clip, &template, 1.0).unwrap();
        assert_eq!(clip.notes[0].timing_offset_beats, 0.25);
        assert_eq!(clip.notes[1].timing_offset_beats, 0.0);
        assert_eq!(clip.notes[1].velocity, 40);
    }

    #[test]
    fn pool_files_replaces_removes_and_round_trips() {
        let mut pool = GroovePool::new();
        assert!(pool.is_empty());
        assert!(pool.insert("", GrooveTemplate::flat(4)).is_err());
        assert!(pool.insert("  ", GrooveTemplate::flat(4)).is_err());
        pool.insert("swing", extract(&swung_clip(), 4).unwrap())
            .unwrap();
        pool.insert("flat", GrooveTemplate::flat(4)).unwrap();
        assert_eq!(pool.names(), vec!["flat".to_string(), "swing".to_string()]);
        assert_eq!(pool.len(), 2);
        // Replace under the same name (trims whitespace).
        pool.insert(" swing ", GrooveTemplate::flat(4)).unwrap();
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.get("swing"), Some(&GrooveTemplate::flat(4)));
        let json = pool.to_json().unwrap();
        assert_eq!(GroovePool::from_json(&json).unwrap(), pool);
        assert!(GroovePool::from_json("not json").is_err());
        assert!(pool.remove("swing"));
        assert!(!pool.remove("swing"));
        assert!(pool.get("swing").is_none());
    }
}
