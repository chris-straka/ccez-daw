//! Track I: in-timeline repair — sample cleanup, spectral edits, MIDI tidy.
//!
//! Teaching note: repair is *editing*, not mixing. Every function here is a
//! pure, deterministic transform over data the timeline already owns, so
//! each repair previews exactly and undoes through the ordinary op log
//! (audio/MIDI bytes live as engine assets beside the project, like Track F
//! MIDI blobs; the project itself never stores samples).
//!
//! Two halves:
//!
//! - **Sample repairs** over mono `f32` frames: [`remove_dc`] (DC offset),
//!   [`declick`] (spike interpolation), [`trim_silence`] (leading/trailing
//!   hush), [`detect_transients`] (energy-flux onsets — the same onsets the
//!   spectral editor snaps to).
//! - **Spectral edits** over magnitude frames (`&mut [Vec<f32>]`, one frame
//!   per row, one band per column): [`gate_bands`] (hush below a floor) and
//!   [`notch_band`] (cut one humming band, e.g. 50/60 Hz mains).
//! - **MIDI cleanup** over [`MidiClip`](crate::midi::MidiClip):
//!   [`quantize_clip`] (snap starts to a grid, offsets keep the humanize
//!   remainder), [`remove_muted`] (drop muted notes), and
//!   [`fix_same_pitch_overlaps`] (truncate an earlier note where a later
//!   same-pitch note starts inside it — the classic stuck-note fix).
//!
//! Adds no IPC or project-schema surface, so the typegen drift gate is
//! unaffected.

use crate::midi::MidiClip;

/// Subtract the mean: removes DC offset so silence is really zero.
pub fn remove_dc(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    if mean != 0.0 {
        for s in samples.iter_mut() {
            *s -= mean;
        }
    }
}

/// Repair click spikes: a frame is a click when `|x| > threshold` *and* it
/// exceeds `ratio` times the louder neighbor (a lone spike, not loud
/// music). Clicks are replaced by linear interpolation across `radius`
/// frames each side. Returns the repaired count.
pub fn declick(samples: &mut [f32], threshold: f32, radius: usize, ratio: f32) -> usize {
    if samples.len() < 3 || !(threshold > 0.0) || radius == 0 {
        return 0;
    }
    // Detect first (read-only), repair after — a repair must never seed a
    // neighboring detection in the same pass.
    let mut clicks = Vec::new();
    for i in 1..samples.len() - 1 {
        let x = samples[i].abs();
        if x > threshold {
            let beside = samples[i - 1].abs().max(samples[i + 1].abs());
            if beside > 0.0 && x > ratio * beside {
                clicks.push(i);
            } else if beside == 0.0 {
                clicks.push(i);
            }
        }
    }
    for i in clicks.iter() {
        let i = *i;
        let lo = i.saturating_sub(radius);
        let hi = (i + radius).min(samples.len() - 1);
        let (a, b) = (samples[lo], samples[hi]);
        let span = (hi - lo).max(1) as f32;
        for k in lo..=hi {
            let t = (k - lo) as f32 / span;
            samples[k] = a + t * (b - a);
        }
    }
    clicks.len()
}

/// First/last frame indexes holding audio above `threshold` (inclusive).
/// Returns `None` when the whole buffer is silence.
pub fn trim_silence(samples: &[f32], threshold: f32) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;
    for (i, &s) in samples.iter().enumerate() {
        if s.abs() > threshold {
            if first.is_none() {
                first = Some(i);
            }
            last = Some(i);
        }
    }
    first.map(|f| (f, last.unwrap_or(f)))
}

/// Energy-flux onset detection: frame indexes where the mean-square energy
/// of the `window` ahead exceeds `factor` times the window behind. Pure and
/// deterministic — the spectral editor snaps selections to these.
pub fn detect_transients(samples: &[f32], window: usize, factor: f32) -> Vec<usize> {
    if window == 0 || samples.len() < 2 * window {
        return Vec::new();
    }
    let energy = |from: usize, to: usize| -> f32 {
        samples[from..to].iter().map(|s| s * s).sum::<f32>() / (to - from) as f32
    };
    let mut out = Vec::new();
    let mut i = window;
    while i + window <= samples.len() {
        let before = energy(i - window, i);
        let after = energy(i, i + window);
        if after > factor * before.max(f32::EPSILON) && after > f32::EPSILON {
            out.push(i);
            i += window; // one hit per window: no double-trigger on a edge
        } else {
            i += 1;
        }
    }
    out
}

/// Spectral gate: magnitudes below `floor` go to zero (hush per band).
/// Returns the zeroed bin count.
pub fn gate_bands(frames: &mut [Vec<f32>], floor: f32) -> usize {
    let mut n = 0;
    for frame in frames.iter_mut() {
        for bin in frame.iter_mut() {
            if *bin < floor {
                if *bin != 0.0 {
                    n += 1;
                }
                *bin = 0.0;
            }
        }
    }
    n
}

/// Notch one band (plus `width` neighbors each side) by `cut` (0 = full cut,
/// 1 = untouched). Out-of-range band centers are errors, never silent.
pub fn notch_band(
    frames: &mut [Vec<f32>],
    band: usize,
    width: usize,
    cut: f32,
) -> Result<(), String> {
    if !(0.0..=1.0).contains(&cut) {
        return Err(format!("notch cut {cut} must be in 0..=1"));
    }
    for (fi, frame) in frames.iter_mut().enumerate() {
        if band >= frame.len() {
            return Err(format!(
                "frame {fi}: band {band} out of range ({} bands)",
                frame.len()
            ));
        }
        let lo = band.saturating_sub(width);
        let hi = (band + width).min(frame.len() - 1);
        for b in lo..=hi {
            frame[b] *= cut;
        }
    }
    Ok(())
}

/// Snap note starts to `grid` beats (grid > 0): the grid part moves into
/// `start_beats`, the leftover becomes `timing_offset_beats` (clamped to the
/// ±0.25 microtiming lane; remainders beyond the lane fold back — the grid
/// wins, loudly documented, never silently kept).
pub fn quantize_clip(clip: &MidiClip, grid: f64) -> Result<MidiClip, String> {
    if !(grid > 0.0 && grid.is_finite()) {
        return Err(format!("quantize grid {grid} must be finite and > 0"));
    }
    let mut out = clip.clone();
    for note in &mut out.notes {
        let onset = note.start_beats + note.timing_offset_beats;
        let snapped = (onset / grid).round() * grid;
        let mut remainder = onset - snapped;
        if remainder.abs() > 0.25 {
            // Beyond the microtiming lane: keep the grid, drop the feel.
            remainder = 0.0;
        }
        note.start_beats = snapped.max(0.0);
        note.timing_offset_beats = if note.start_beats == 0.0 && remainder < 0.0 {
            0.0
        } else {
            remainder
        };
    }
    out.notes.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
            .then(a.note_id.cmp(&b.note_id))
    });
    Ok(out)
}

/// Drop muted notes. Returns the removed count.
pub fn remove_muted(clip: &mut MidiClip) -> usize {
    let before = clip.notes.len();
    clip.notes.retain(|n| !n.muted);
    before - clip.notes.len()
}

/// Truncate same-(pitch, channel) overlaps: when a later note starts inside
/// an earlier one, the earlier note ends where the later begins (1-tick
/// floor keeps lengths positive). Returns the fixed count.
pub fn fix_same_pitch_overlaps(clip: &mut MidiClip) -> usize {
    const TICK: f64 = 1.0 / 480.0;
    let mut fixed = 0;
    // Notes are sorted by (start, pitch, id); compare consecutive runs of
    // the same (pitch, channel).
    clip.notes.sort_by(|a, b| {
        a.pitch
            .cmp(&b.pitch)
            .then(a.channel.cmp(&b.channel))
            .then(
                a.start_beats
                    .partial_cmp(&b.start_beats)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    for i in 1..clip.notes.len() {
        let (prev_end, key_prev, key_cur, cur_start) = {
            let (prev, cur) = (&clip.notes[i - 1], &clip.notes[i]);
            (
                prev.start_beats + prev.len_beats,
                (prev.pitch, prev.channel),
                (cur.pitch, cur.channel),
                cur.start_beats,
            )
        };
        if key_prev == key_cur && cur_start < prev_end {
            let prev = &mut clip.notes[i - 1];
            prev.len_beats = (cur_start - prev.start_beats).max(TICK);
            fixed += 1;
        }
    }
    // Restore canonical clip order.
    clip.notes.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
            .then(a.note_id.cmp(&b.note_id))
    });
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiNote;

    #[test]
    fn dc_removal_zeroes_silence() {
        let mut buf = vec![0.5; 64];
        remove_dc(&mut buf);
        assert!(buf.iter().all(|s| s.abs() < 1e-6));
        let mut empty: Vec<f32> = vec![];
        remove_dc(&mut empty); // no panic
    }

    #[test]
    fn declick_repairs_lone_spike_and_ignores_music() {
        let mut buf = vec![0.1; 32];
        buf[16] = 0.9; // lone spike
        let n = declick(&mut buf, 0.5, 2, 4.0);
        assert_eq!(n, 1);
        assert!(buf[16].abs() < 0.2, "spike interpolated, got {}", buf[16]);
        // Loud plateau: neighbors equally loud, not a click.
        let mut music = vec![0.8; 32];
        assert_eq!(declick(&mut music, 0.5, 2, 4.0), 0);
    }

    #[test]
    fn trim_and_transients_agree() {
        let mut buf = vec![0.0; 64];
        for s in buf[16..48].iter_mut() {
            *s = 0.5;
        }
        assert_eq!(trim_silence(&buf, 0.01), Some((16, 47)));
        assert_eq!(trim_silence(&vec![0.0; 8], 0.01), None);
        let hits = detect_transients(&buf, 4, 2.0);
        assert!(hits.iter().any(|&h| (12..=20).contains(&h)), "hits={hits:?}");
    }

    #[test]
    fn spectral_edits_cut_and_gate() {
        let mut spec = vec![vec![0.01, 0.9, 0.02], vec![0.03, 0.8, 0.005]];
        let zeroed = gate_bands(&mut spec, 0.05);
        assert_eq!(zeroed, 4);
        assert_eq!(spec[0][1], 0.9);
        notch_band(&mut spec, 1, 0, 0.0).unwrap();
        assert_eq!(spec[0][1], 0.0);
        assert!(notch_band(&mut spec, 9, 0, 0.0).is_err());
        assert!(notch_band(&mut spec, 0, 0, 2.0).is_err());
    }

    #[test]
    fn midi_cleanup_quantize_muted_overlaps() {
        use crate::midi::MidiClip;
        let mut clip = MidiClip::new(4.0);
        let mut sloppy = MidiNote::new(1, 60, 100, 0.51, 1.0);
        sloppy.timing_offset_beats = 0.0;
        clip.add_note(sloppy).unwrap();
        let mut muted = MidiNote::new(2, 64, 100, 1.0, 0.5);
        muted.muted = true;
        clip.add_note(muted).unwrap();
        let q = quantize_clip(&clip, 0.5).unwrap();
        let first = q.notes.iter().find(|n| n.note_id == 1).unwrap();
        assert_eq!(first.start_beats, 0.5);
        assert!(quantize_clip(&clip, 0.0).is_err());

        let mut clip2 = clip.clone();
        assert_eq!(remove_muted(&mut clip2), 1);

        let mut clip3 = MidiClip::new(4.0);
        clip3.add_note(MidiNote::new(1, 60, 100, 0.0, 2.0)).unwrap();
        clip3.add_note(MidiNote::new(2, 60, 100, 1.0, 1.0)).unwrap();
        assert_eq!(fix_same_pitch_overlaps(&mut clip3), 1);
        assert_eq!(clip3.notes[0].len_beats, 1.0);
    }
}
