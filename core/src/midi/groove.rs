//! Groove extract / transfer.
//!
//! A [`GrooveTemplate`](crate::midi::GrooveTemplate) is the feel of one
//! clip, distilled: per-grid-step average timing offsets (late/early, in
//! beats) and per-step velocity scales. `extract` learns it from a
//! performance; `apply` blends it into another clip with an `amount`
//! 0..=1 (0 = unchanged, 1 = full template feel). Both are pure functions
//! over beats — tempo never enters, so the same template works at any BPM.

use super::MidiClip;

/// Feel of one clip, quantized to a grid of `steps_per_beat` steps per beat
/// (4 = 16ths at 4/4). `offsets[i]` is the mean (actual − grid) shift in
/// beats for notes nearest step `i`; `vel_scale[i]` is the mean
/// (velocity / 100.0) ratio (1.0 = no emphasis information).
#[derive(Debug, Clone, PartialEq)]
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

/// Blend `template` into `clip` with `amount` 0..=1. Each unmuted note moves
/// `timing_offset_beats += template_offset(step) * amount` (clamped to the
/// ±0.25 microtiming lane) and scales velocity toward
/// `velocity * vel_scale(step)` by `amount` (clamped 1..=127, rounded).
/// Muted notes are untouched. Step lookup uses the note's grid position
/// *before* its own microtiming, so repeated transfers converge instead of
/// drifting.
pub fn apply(clip: &mut MidiClip, template: &GrooveTemplate, amount: f64) -> Result<(), String> {
    if !(0.0..=1.0).contains(&amount) || !amount.is_finite() {
        return Err(format!("amount {amount} out of range 0..=1"));
    }
    if template.steps_per_beat < 1 || template.step_count() != template.steps_per_beat as usize {
        return Err("template steps inconsistent".to_string());
    }
    if template.vel_scale.len() != template.step_count() {
        return Err("template offsets/vel_scale length mismatch".to_string());
    }
    let n = template.steps_per_beat as usize;
    for note in clip.notes.iter_mut().filter(|n| !n.muted) {
        let grid_pos = (note.start_beats * template.steps_per_beat as f64).round() as i64;
        let step = grid_pos.rem_euclid(n as i64) as usize;
        note.timing_offset_beats =
            (note.timing_offset_beats + template.offsets[step] * amount).clamp(-0.25, 0.25);
        let target = note.velocity as f64 * template.vel_scale[step];
        let blended = note.velocity as f64 + (target - note.velocity as f64) * amount;
        note.velocity = blended.round().clamp(1.0, 127.0) as u8;
        note.validate().map_err(|e| format!("groove transfer produced invalid note: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
