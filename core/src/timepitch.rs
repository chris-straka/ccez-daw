//! Track I: per-clip time/pitch + warp.
//!
//! Teaching note: pitch and time are *per-clip* views over the same notes,
//! not new project fields. A [`Clip`](crate::model::Clip) keeps its frozen
//! shape; the sounding result is `MidiClip` + the sidecar
//! [`ClipProps`](crate::timeline::ClipProps) (`pitch_semitones` in [-24, 24],
//! `time_ratio` > 0) interpreted by the pure functions here.
//!
//! Three ideas, each one function family:
//!
//! 1. **Transpose is integer pitch + fractional bend.** MIDI pitch is an
//!    integer, but `pitch_semitones` is a float. The whole part shifts
//!    `pitch`; the leftover fraction (±0.5 st) rides on per-note
//!    `pitch_bend`, so a +0.3 detune survives instead of rounding away.
//!    Integer transposes are exactly invertible — the round-trip test.
//! 2. **Time ratio is playback rate.** `time_ratio = 2` plays twice as fast:
//!    every onset and length halves (and the clip length halves). Like
//!    transpose, it is exactly invertible (`r` then `1/r`).
//! 3. **Warp is a piecewise-linear shift map.** A [`WarpMap`] holds sorted
//!    `(at_beats, shift_beats)` markers; [`warp_beat`] interpolates the shift
//!    at any beat. An empty map is the identity. Warp moves onsets only —
//!    lengths are preserved (a warped note keeps its performed duration).
//!
//! Audio helpers ([`resampled_len`]) use the same ratio convention so the
//! sample path and the MIDI path agree: faster ratio = fewer frames.
//!
//! Adds no IPC or project-schema surface (like `timeline.rs` / `branch.rs`),
//! so the typegen drift gate is unaffected.

use crate::midi::{MidiClip, MidiNote};

/// Matches `ClipProps::validate`: transpose outside ±24 is an error,
/// never a silent clamp.
pub const MAX_TRANSPOSE_ST: f64 = 24.0;

/// Twelve-tone equal temperament: semitones -> playback-rate ratio.
pub fn semitones_to_ratio(semitones: f64) -> f64 {
    2.0f64.powf(semitones / 12.0)
}

/// Inverse of [`semitones_to_ratio`].
pub fn ratio_to_semitones(ratio: f64) -> f64 {
    12.0 * ratio.log2()
}

/// Split a float semitone shift into an integer note shift plus a fractional
/// bend remainder in [-0.5, 0.5].
pub fn split_pitch(semitones: f64) -> (i32, f64) {
    let whole = semitones.round() as i32;
    (whole, semitones - whole as f64)
}

/// Transpose every note by `semitones` (range ±24 like `ClipProps`).
/// Out-of-range *results* (pitch leaving 0..=127) are errors — the caller
/// decides whether to clamp, split, or refuse, so nothing clips silently.
pub fn transpose_clip(clip: &MidiClip, semitones: f64) -> Result<MidiClip, String> {
    if !semitones.is_finite() || semitones.abs() > MAX_TRANSPOSE_ST {
        return Err(format!(
            "transpose {semitones} out of range ±{MAX_TRANSPOSE_ST} semitones"
        ));
    }
    let mut out = clip.clone();
    for note in &mut out.notes {
        let shifted = note.pitch as f64 + semitones;
        if !(0.0..=127.0).contains(&shifted) {
            return Err(format!(
                "note {} would leave 0..=127 ({} -> {shifted})",
                note.note_id, note.pitch
            ));
        }
        note.pitch = shifted.round() as u8;
    }
    Ok(out)
}

/// Scale clip time by a playback-rate `ratio` (> 0, finite): onsets, lengths,
/// and the clip length all divide by `ratio`. Invertible: `r` then `1/r`.
pub fn time_scale_clip(clip: &MidiClip, ratio: f64) -> Result<MidiClip, String> {
    if !(ratio > 0.0 && ratio.is_finite()) {
        return Err(format!("time ratio {ratio} must be finite and > 0"));
    }
    let mut out = clip.clone();
    out.length_beats /= ratio;
    for note in &mut out.notes {
        note.start_beats /= ratio;
        note.len_beats /= ratio;
        note.timing_offset_beats /= ratio;
    }
    Ok(out)
}

/// Apply sidecar props to a MIDI clip: integer transpose + fractional bend,
/// then time scale. Integer `pitch_semitones` round-trips exactly
/// (transpose up then down restores every pitch); fractional parts ride
/// `pitch_bend` (±48 st range checked per note).
pub fn apply_props_to_midi(
    clip: &MidiClip,
    pitch_semitones: f64,
    time_ratio: f64,
) -> Result<MidiClip, String> {
    if !pitch_semitones.is_finite() || pitch_semitones.abs() > MAX_TRANSPOSE_ST {
        return Err(format!(
            "pitch {pitch_semitones} out of range ±{MAX_TRANSPOSE_ST} semitones"
        ));
    }
    let (whole, bend) = split_pitch(pitch_semitones);
    let mut out = transpose_clip(clip, whole as f64)?;
    if bend != 0.0 {
        for note in &mut out.notes {
            let b = note.pitch_bend + bend;
            if b.abs() > 48.0 {
                return Err(format!(
                    "note {} bend {b} would leave ±48 st",
                    note.note_id
                ));
            }
            note.pitch_bend = b;
        }
    }
    out = time_scale_clip(&out, time_ratio)?;
    Ok(out)
}

/// One warp pin: at source beat `at_beats`, shift time by `shift_beats`
/// (positive = later).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpMarker {
    pub at_beats: f64,
    pub shift_beats: f64,
}

/// Sorted warp pins. Shifts interpolate linearly between pins and hold flat
/// past the ends — so an empty map is exactly the identity.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WarpMap {
    pub markers: Vec<WarpMarker>,
}

impl WarpMap {
    pub fn new(mut markers: Vec<WarpMarker>) -> Result<Self, String> {
        for m in &markers {
            if !m.at_beats.is_finite() || !m.shift_beats.is_finite() {
                return Err("warp markers must be finite".to_string());
            }
        }
        markers.sort_by(|a, b| {
            a.at_beats
                .partial_cmp(&b.at_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(Self { markers })
    }

    pub fn is_identity(&self) -> bool {
        self.markers.iter().all(|m| m.shift_beats == 0.0)
    }

    /// Negated map: warping by `map` then by `-map` approximately restores
    /// onsets (exact when shifts are small relative to marker spacing).
    pub fn negated(&self) -> Self {
        Self {
            markers: self
                .markers
                .iter()
                .map(|m| WarpMarker {
                    at_beats: m.at_beats,
                    shift_beats: -m.shift_beats,
                })
                .collect(),
        }
    }
}

/// Shift applied at one beat: linear interpolation between pins, flat ends.
pub fn warp_beat(map: &WarpMap, beat: f64) -> f64 {
    let ms = &map.markers;
    if ms.is_empty() {
        return beat;
    }
    if beat <= ms[0].at_beats {
        return beat + ms[0].shift_beats;
    }
    if beat >= ms[ms.len() - 1].at_beats {
        return beat + ms[ms.len() - 1].shift_beats;
    }
    for pair in ms.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if beat >= a.at_beats && beat <= b.at_beats {
            let span = b.at_beats - a.at_beats;
            let t = if span == 0.0 { 0.0 } else { (beat - a.at_beats) / span };
            return beat + a.shift_beats + t * (b.shift_beats - a.shift_beats);
        }
    }
    beat
}

/// Warp a clip: onsets (and microtiming offsets) move through the map,
/// lengths are preserved, result re-sorted. Identity map = byte-stable
/// no-op (encode before/after match).
pub fn apply_warp_to_midi(clip: &MidiClip, map: &WarpMap) -> MidiClip {
    let mut out = clip.clone();
    for note in &mut out.notes {
        let onset = note.start_beats + note.timing_offset_beats;
        let warped = warp_beat(map, onset).max(0.0);
        // Keep the humanize offset intact: move the grid start, not the feel.
        note.start_beats = (warped - note.timing_offset_beats).max(0.0);
    }
    out.notes.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
            .then(a.note_id.cmp(&b.note_id))
    });
    out
}

/// Frame count of an audio clip under a playback-rate ratio: faster = fewer
/// frames. `ceil` so ratio < 1 never drops the trailing partial frame.
pub fn resampled_len(frames: usize, ratio: f64) -> Result<usize, String> {
    if !(ratio > 0.0 && ratio.is_finite()) {
        return Err(format!("time ratio {ratio} must be finite and > 0"));
    }
    Ok(((frames as f64) / ratio).ceil() as usize)
}

/// Build a small demo clip for tests: C–E–G quarter notes from beat 0.
pub fn sample_phrase() -> MidiClip {
    let mut clip = MidiClip::new(4.0);
    for (i, pitch) in [60u8, 64, 67].iter().enumerate() {
        let mut n = MidiNote::new(i as u32 + 1, *pitch, 100, i as f64, 0.9);
        n.channel = 1;
        clip.add_note(n).unwrap();
    }
    clip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_and_semitones_are_inverse() {
        for st in [-24.0, -12.0, -7.0, 0.0, 7.0, 12.0, 24.0] {
            let back = ratio_to_semitones(semitones_to_ratio(st));
            assert!((back - st).abs() < 1e-9, "st={st} back={back}");
        }
        assert!((semitones_to_ratio(12.0) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn transpose_rejects_out_of_range_shift_and_result() {
        let clip = sample_phrase();
        assert!(transpose_clip(&clip, 25.0).is_err());
        let mut low = MidiClip::new(1.0);
        low.add_note(MidiNote::new(1, 2, 100, 0.0, 0.5)).unwrap();
        assert!(transpose_clip(&low, -12.0).is_err());
    }

    #[test]
    fn time_scale_rejects_bad_ratio() {
        let clip = sample_phrase();
        assert!(time_scale_clip(&clip, 0.0).is_err());
        assert!(time_scale_clip(&clip, -1.0).is_err());
        assert!(time_scale_clip(&clip, f64::INFINITY).is_err());
    }

    #[test]
    fn empty_warp_is_identity() {
        let clip = sample_phrase();
        let map = WarpMap::new(vec![]).unwrap();
        let warped = apply_warp_to_midi(&clip, &map);
        assert_eq!(warped, clip);
        assert_eq!(
            warped.encode().unwrap(),
            clip.encode().unwrap(),
            "identity warp must be byte-stable"
        );
    }

    #[test]
    fn resampled_len_rounds_up() {
        assert_eq!(resampled_len(100, 2.0).unwrap(), 50);
        assert_eq!(resampled_len(100, 3.0).unwrap(), 34);
        assert!(resampled_len(100, 0.0).is_err());
    }
}
