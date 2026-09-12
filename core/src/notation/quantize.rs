//! Meter/key-aware MIDI <-> notation quantization.
//!
//! [`quantize`] turns a performance ([`MidiClip`]: floating-point onsets
//! and lengths) into a written [`Score`]: onsets snap to `grid_beats`,
//! lengths snap to whole grid steps, simultaneous onsets fuse into chords,
//! gaps become rests, notes crossing a bar line split into tied notes, and
//! runs of eighth notes and shorter are beamed per beat group.
//! [`to_midi`] renders a score back to a clip — the *written* part, so a
//! swung performance quantizes to straight eighths and stays straight.

use crate::midi::{MidiClip, MidiNote};

use super::model::{
    BeamGroup, Chord, KeySig, Measure, MeasureEvent, Meter, NotationNote, NotationRest, NoteValue,
    Score, SpelledPitch,
};

/// Snap tolerance: two onsets within half a grid step fuse into a chord.
const EPS: f64 = 1e-9;

/// Quantizer settings. `grid_beats` is the snap step in quarter-note beats:
/// 1.0 = quarter grid, 0.5 = eighths, 0.25 = sixteenths (default), 0.125 =
/// thirty-seconds (the shortest value the model can write).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantizeOptions {
    pub meter: Meter,
    pub key: KeySig,
    pub grid_beats: f64,
}

impl Default for QuantizeOptions {
    fn default() -> Self {
        Self {
            meter: Meter::new(4, 4),
            key: KeySig::major(0),
            grid_beats: 0.25,
        }
    }
}

impl QuantizeOptions {
    pub fn validate(&self) -> Result<f64, String> {
        self.meter.validate()?;
        self.key.validate()?;
        let bar = self.meter.bar_beats().expect("meter validated");
        if !self.grid_beats.is_finite() || self.grid_beats <= 0.0 {
            return Err(format!("grid_beats {} must be finite and > 0", self.grid_beats));
        }
        // The grid must divide the bar, and must not be finer than a 32nd.
        let steps = (bar / self.grid_beats).round();
        if steps < 1.0 || (steps * self.grid_beats - bar).abs() > 1e-6 {
            return Err(format!(
                "grid_beats {} must divide the {}-beat bar",
                self.grid_beats, bar
            ));
        }
        if self.grid_beats < 0.125 - EPS {
            return Err(format!(
                "grid_beats {} is finer than a 32nd (0.125)",
                self.grid_beats
            ));
        }
        Ok(bar)
    }
}

/// One snapped note before bar layout: onset/length in grid steps.
#[derive(Debug, Clone)]
struct Snapped {
    start_step: i64,
    len_steps: i64,
    pitches: Vec<u8>,
}

/// Snap a clip to the grid, fusing simultaneous onsets into chords.
/// Muted notes are skipped (they never sound, like in `midi::expand`);
/// `probability` is ignored — notation writes the part, not the take gate.
fn snap_notes(clip: &MidiClip, grid: f64) -> Vec<Snapped> {
    let mut sounding: Vec<&MidiNote> = clip.notes.iter().filter(|n| !n.muted).collect();
    sounding.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
    });
    // Fuse onsets within half a grid step: they read as one chord.
    let mut fused: Vec<(f64, f64, Vec<u8>)> = Vec::new();
    for n in sounding {
        let start = n.start_beats.max(0.0);
        let end = (n.start_beats + n.len_beats).max(start + grid);
        match fused.last_mut() {
            Some(last) if (start - last.0).abs() <= grid / 2.0 => {
                last.1 = last.1.max(end);
                if !last.2.contains(&n.pitch) {
                    last.2.push(n.pitch);
                    last.2.sort();
                }
            }
            _ => fused.push((start, end, vec![n.pitch])),
        }
    }
    let mut snapped: Vec<Snapped> = fused
        .into_iter()
        .map(|(start, end, pitches)| {
            let start_step = (start / grid).round() as i64;
            let end_step = ((end / grid).round() as i64).max(start_step + 1);
            Snapped { start_step, len_steps: end_step - start_step, pitches }
        })
        .collect();
    // One voice, one staff: a note that is still sounding when the next
    // onset arrives is truncated to that onset (legato overlaps quantize to
    // back-to-back notes; held-against-melody polyphony is out of scope).
    snapped.sort_by_key(|s| (s.start_step, s.pitches.clone()));
    for i in 0..snapped.len() {
        let next_start = snapped.get(i + 1).map(|s| s.start_step);
        if let Some(ns) = next_start {
            let end = snapped[i].start_step + snapped[i].len_steps;
            if end > ns {
                snapped[i].len_steps = (ns - snapped[i].start_step).max(1);
            }
        }
    }
    snapped
}

/// Greedy duration split into (value, dots) with at most one dot each.
/// Covers every multiple of a 32nd up to a whole note; callers split
/// longer spans before calling.
fn split_duration(steps_of_32nd: i64) -> Vec<(NoteValue, u8)> {
    // (32nd-steps, value, dots), longest first. Dotted entries are 1.5x.
    const TABLE: [(i64, NoteValue, u8); 11] = [
        (32, NoteValue::Whole, 0),
        (24, NoteValue::Half, 1),
        (16, NoteValue::Half, 0),
        (12, NoteValue::Quarter, 1),
        (8, NoteValue::Quarter, 0),
        (6, NoteValue::Eighth, 1),
        (4, NoteValue::Eighth, 0),
        (3, NoteValue::Sixteenth, 1),
        (2, NoteValue::Sixteenth, 0),
        (1, NoteValue::ThirtySecond, 0),
        (0, NoteValue::ThirtySecond, 0), // placeholder, never matched
    ];
    let mut rest = steps_of_32nd.max(1);
    let mut out = Vec::new();
    for (steps, value, dots) in TABLE.into_iter().take(10) {
        while rest >= steps {
            out.push((value, dots));
            rest -= steps;
        }
    }
    if rest > 0 {
        // Remainder below a 32nd cannot happen (inputs are grid multiples
        // >= 32nd); fold it into the last event rather than dropping time.
        let _ = rest;
    }
    out
}

/// Build one written note (or tied-note head) for a duration piece.
fn make_note(
    pitch: u8,
    value: NoteValue,
    dots: u8,
    key: KeySig,
    tie_start: bool,
    tie_stop: bool,
) -> NotationNote {
    NotationNote {
        pitch: SpelledPitch::from_midi(pitch, key),
        midi: pitch,
        value,
        dots,
        tie_start,
        tie_stop,
    }
}

/// Quantize a MIDI clip to a written score.
///
/// Steps: validate options, snap onsets/lengths to the grid (fusing
/// near-simultaneous onsets into chords, truncating overlaps to the next
/// onset — one voice, one staff), lay the snapped stream onto bars
/// (rests fill gaps, cross-bar notes split with ties), spell pitches in
/// `opts.key`, and beam eighth-note runs per quarter-beat group.
///
/// An empty (or fully muted) clip yields one bar of rest — notation always
/// has something to show, and the bar count still honors the clip length.
pub fn quantize(clip: &MidiClip, opts: &QuantizeOptions) -> Result<Score, String> {
    let bar = opts.validate()?;
    let grid = opts.grid_beats;
    let steps_per_bar = (bar / grid).round() as i64;

    let snapped = snap_notes(clip, grid);
    let last_end = snapped.iter().map(|s| s.start_step + s.len_steps).max().unwrap_or(0);
    let total_end = (clip.length_beats / grid).ceil() as i64;
    let stream_end = last_end.max(total_end).max(0);
    let n_bars = ((stream_end + steps_per_bar - 1) / steps_per_bar).max(1);

    // Per-bar event streams as (offset_steps, pitches-or-rest, len_steps).
    let mut measures = Vec::with_capacity(n_bars as usize);
    // Cursor per bar: next free step (rests fill up to the next onset).
    for bar_idx in 0..n_bars {
        let bar_start = bar_idx * steps_per_bar;
        // Segments sounding in this bar, including ties carried over from
        // notes that started in an earlier bar.
        #[derive(Clone)]
        struct Seg {
            at: i64,
            len: i64,
            pitches: Vec<u8>,
            tie_stop: bool,
        }
        let mut segs: Vec<Seg> = Vec::new();
        for s in &snapped {
            let s_end = s.start_step + s.len_steps;
            if s_end <= bar_start || s.start_step >= bar_start + steps_per_bar {
                continue;
            }
            // A note starting before this bar is a tie continuation.
            let tie_stop = s.start_step < bar_start;
            let at = s.start_step.max(bar_start);
            // Clip the segment to this bar; a remainder continues tied.
            let len = (s_end - at).min(bar_start + steps_per_bar - at);
            if len > 0 {
                segs.push(Seg { at, len, pitches: s.pitches.clone(), tie_stop });
            }
        }
        segs.sort_by_key(|s| (s.at, s.pitches.clone()));

        let mut events: Vec<MeasureEvent> = Vec::new();
        let mut cursor = 0i64; // steps within this bar
        // 32nd-note steps per grid step (grid >= 32nd, divides evenly).
        let steps32_per_grid = (grid / 0.125).round() as i64;
        let bar_32nds = steps_per_bar * steps32_per_grid;

        // Helper: push rests covering [cursor, target) in 32nd units.
        let push_rest = |events: &mut Vec<MeasureEvent>, from_32: i64, to_32: i64| {
            if to_32 > from_32 {
                for (value, dots) in split_duration(to_32 - from_32) {
                    events.push(MeasureEvent::Rest(NotationRest { value, dots }));
                }
            }
        };

        for seg in &segs {
            let at_in_bar = seg.at - bar_start;
            push_rest(&mut events, cursor * steps32_per_grid, at_in_bar * steps32_per_grid);
            // Split this bar's slice of the note; the head carries tie_stop
            // when it continues an earlier bar, the tail carries tie_start
            // when more bars (or more of this bar) follow.
            let mut left_32 = seg.len * steps32_per_grid;
            let mut first = true;
            // How much of the whole note remains after this bar (for ties).
            let note_end = snapped
                .iter()
                .filter(|s| {
                    s.start_step <= seg.at
                        && seg.at < s.start_step + s.len_steps
                        && s.pitches == seg.pitches
                })
                .map(|s| s.start_step + s.len_steps)
                .max()
                .unwrap_or(seg.at + seg.len);
            while left_32 > 0 {
                // Cap one tied piece at a whole note (the longest value).
                let piece = left_32.min(32);
                let after_this_bar = note_end > bar_start + steps_per_bar;
                let more_pieces = left_32 > piece;
                let parts = split_duration(piece);
                let n = parts.len();
                for (i, (value, dots)) in parts.into_iter().enumerate() {
                    let tie_stop = first && i == 0 && seg.tie_stop;
                    let tie_start = (i + 1 < n) || more_pieces || after_this_bar;
                    let notes = seg
                        .pitches
                        .iter()
                        .map(|&p| make_note(p, value, dots, opts.key, tie_start, tie_stop))
                        .collect::<Vec<_>>();
                    if notes.len() == 1 {
                        events.push(MeasureEvent::Note(notes.into_iter().next().unwrap()));
                    } else {
                        events.push(MeasureEvent::Chord(Chord { notes }));
                    }
                    first = false;
                }
                left_32 -= piece;
            }
            cursor = seg.at - bar_start + seg.len;
        }
        push_rest(&mut events, cursor * steps32_per_grid, bar_32nds);

        // Auto-beam: runs of >= 2 consecutive beamable sounding events that
        // start inside the same quarter-beat group. Offsets are tracked in
        // 32nds alongside the events.
        let mut offsets_32: Vec<i64> = Vec::with_capacity(events.len());
        let mut acc = 0i64;
        for e in &events {
            offsets_32.push(acc);
            acc += (e.beats() / 0.125).round() as i64;
        }
        let beamable_idx = |e: &MeasureEvent| -> Option<NoteValue> {
            match e {
                MeasureEvent::Note(n) if n.value.beamable() => Some(n.value),
                MeasureEvent::Chord(c) if c.notes[0].value.beamable() => Some(c.notes[0].value),
                _ => None,
            }
        };
        let mut beams: Vec<BeamGroup> = Vec::new();
        let mut run: Vec<usize> = Vec::new();
        let flush = |run: &mut Vec<usize>, beams: &mut Vec<BeamGroup>| {
            if run.len() >= 2 {
                beams.push(BeamGroup { events: run.clone() });
            }
            run.clear();
        };
        // Quarter-beat group = 8 thirty-seconds (1 quarter = 8 x 32nd).
        for (idx, e) in events.iter().enumerate() {
            match beamable_idx(e) {
                Some(_) => {
                    let group = offsets_32[idx] / 8;
                    match run.last() {
                        Some(&prev) if offsets_32[prev] / 8 == group => run.push(idx),
                        _ => {
                            flush(&mut run, &mut beams);
                            run.push(idx);
                        }
                    }
                }
                None => flush(&mut run, &mut beams),
            }
        }
        flush(&mut run, &mut beams);
        // Belt-and-braces: keep only consecutive runs of length >= 2
        // (construction above already guarantees this shape).
        let mut clean: Vec<BeamGroup> = Vec::new();
        for g in beams {
            let mut run: Vec<usize> = Vec::new();
            for idx in g.events {
                match run.last() {
                    Some(&p) if idx == p + 1 => run.push(idx),
                    _ => {
                        if run.len() >= 2 {
                            clean.push(BeamGroup { events: run.clone() });
                        }
                        run = vec![idx];
                    }
                }
            }
            if run.len() >= 2 {
                clean.push(BeamGroup { events: run });
            }
        }

        measures.push(Measure { number: (bar_idx + 1) as usize, events, beams: clean });
    }

    let score = Score {
        title: String::new(),
        meter: opts.meter,
        key: opts.key,
        measures,
    };
    score.validate(bar)?;
    Ok(score)
}

/// Render a score back to a MIDI clip: one [`MidiNote`] per written note
/// head (chord members included), tied notes fused back into single notes
/// with the summed length. Velocities default to 80; channels to 0.
pub fn to_midi(score: &Score) -> MidiClip {
    let bar = score.meter.bar_beats().unwrap_or(4.0);
    let total = (score.measures.len() as f64 * bar).max(bar);
    let mut clip = MidiClip::new(total);
    let mut next_id: u32 = 1;
    // Open tie segments keyed by pitch: (segment_start_beats, total_len).
    let mut open: std::collections::HashMap<u8, (f64, f64)> = std::collections::HashMap::new();

    let mut at = 0.0f64;
    for m in &score.measures {
        let mut off = 0.0;
        for e in &m.events {
            let beats = e.beats();
            let notes: Vec<(u8, bool, bool)> = match e {
                MeasureEvent::Note(n) => vec![(n.midi, n.tie_start, n.tie_stop)],
                MeasureEvent::Chord(c) => {
                    c.notes.iter().map(|n| (n.midi, n.tie_start, n.tie_stop)).collect()
                }
                MeasureEvent::Rest(_) => vec![],
            };
            for (pitch, tie_start, tie_stop) in notes {
                pending_add(
                    &mut open,
                    &mut clip,
                    &mut next_id,
                    pitch,
                    at + off,
                    beats,
                    tie_stop,
                    tie_start,
                );
            }
            off += beats;
        }
        at += bar;
    }
    // A tie left open at the score end is still a real note — close it.
    let mut dangling: Vec<(u8, f64, f64)> =
        open.drain().map(|(p, (start, len))| (p, start, len)).collect();
    dangling.sort_by_key(|(p, _, _)| *p);
    for (pitch, start, len) in dangling {
        push_note(&mut clip, &mut next_id, pitch, start, len);
    }
    // Direct pushes skip MidiClip's sorted insert — restore sort order.
    clip.notes.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
            .then(a.note_id.cmp(&b.note_id))
    });
    clip
}

/// Accumulate tied segments: a tie_stop extends the pending segment for
/// that pitch; otherwise a new note opens. A finished (non-tie_start)
/// segment is pushed to the clip.
fn pending_add(
    open: &mut std::collections::HashMap<u8, (f64, f64)>,
    clip: &mut MidiClip,
    next_id: &mut u32,
    pitch: u8,
    onset: f64,
    len: f64,
    tie_stop: bool,
    tie_start: bool,
) {
    if tie_stop {
        if let Some((start, total)) = open.remove(&pitch) {
            let total = total + len;
            if tie_start {
                open.insert(pitch, (start, total));
            } else {
                push_note(clip, next_id, pitch, start, total);
            }
            return;
        }
        // Stray tie_stop with nothing open: treat as a plain note.
    }
    if tie_start {
        // Merge with an existing open segment if the onsets chain
        // (defensive: normally tie_stop accompanies continuation).
        if let Some((start, total)) = open.remove(&pitch) {
            open.insert(pitch, (start, total + len));
        } else {
            open.insert(pitch, (onset, len));
        }
    } else {
        push_note(clip, next_id, pitch, onset, len);
    }
}

fn push_note(clip: &mut MidiClip, next_id: &mut u32, pitch: u8, start: f64, len: f64) {
    let id = *next_id;
    *next_id += 1;
    let mut note = MidiNote::new(id, pitch, 80, start, len.max(1.0 / 480.0));
    note.channel = 0;
    clip.notes.push(note);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip_with(notes: Vec<(u32, u8, f64, f64)>, len: f64) -> MidiClip {
        let mut clip = MidiClip::new(len);
        for (id, pitch, start, dur) in notes {
            clip.add_note(MidiNote::new(id, pitch, 90, start, dur)).unwrap();
        }
        clip
    }

    #[test]
    fn quantize_snaps_sloopy_timing_to_grid() {
        // Sloppy eighths around beats 0/0.5/1.0 on a 16th grid.
        let clip = clip_with(
            vec![(1, 60, 0.06, 0.44), (2, 62, 0.52, 0.46), (3, 64, 1.03, 0.9)],
            4.0,
        );
        let score = quantize(&clip, &QuantizeOptions::default()).unwrap();
        assert_eq!(score.measures.len(), 1);
        let m = &score.measures[0];
        // First two notes snapped to 0.0 and 0.5; third to 1.0, len 1.0.
        let sounding: Vec<&MeasureEvent> =
            m.events.iter().filter(|e| !e.is_rest()).collect();
        assert_eq!(sounding.len(), 3);
        // Onsets: 0, 0.5, 1.0 — verify via cumulative beats.
        let mut onsets = Vec::new();
        let mut acc = 0.0;
        for e in &m.events {
            if !e.is_rest() {
                onsets.push(acc);
            }
            acc += e.beats();
        }
        assert!((onsets[0] - 0.0).abs() < 1e-6, "onsets {onsets:?}");
        assert!((onsets[1] - 0.5).abs() < 1e-6, "onsets {onsets:?}");
        assert!((onsets[2] - 1.0).abs() < 1e-6, "onsets {onsets:?}");
        // Bar still sums to 4 beats (rests filled).
        assert!((m.beats() - 4.0).abs() < 1e-6);
    }

    #[test]
    fn quantize_is_meter_aware() {
        // One quarter note at beat 0 in 3/4: bar is 3 beats, rest fills 2.
        let clip = clip_with(vec![(1, 60, 0.0, 1.0)], 3.0);
        let opts = QuantizeOptions {
            meter: Meter::new(3, 4),
            ..QuantizeOptions::default()
        };
        let score = quantize(&clip, &opts).unwrap();
        assert_eq!(score.measures.len(), 1);
        assert!((score.measures[0].beats() - 3.0).abs() < 1e-6);
        // 6/8 bars are 3 quarter-beats too.
        let opts68 = QuantizeOptions {
            meter: Meter::new(6, 8),
            ..QuantizeOptions::default()
        };
        let score68 = quantize(&clip, &opts68).unwrap();
        assert!((score68.measures[0].beats() - 3.0).abs() < 1e-6);
        // A 4-beat clip in 3/4 spans two bars.
        let long = clip_with(vec![(1, 60, 0.0, 4.0)], 4.0);
        let score2 = quantize(&long, &opts).unwrap();
        assert_eq!(score2.measures.len(), 2);
        // The cross-bar note is tied.
        let first = match &score2.measures[0].events[0] {
            MeasureEvent::Note(n) => n,
            other => panic!("want note, got {other:?}"),
        };
        assert!(first.tie_start && !first.tie_stop);
    }

    #[test]
    fn quantize_is_key_aware_and_fuses_chords() {
        // C-E-G triad slightly smeared in time + an F#.
        let clip = clip_with(
            vec![
                (1, 60, 0.0, 1.0),
                (2, 64, 0.03, 1.0),
                (3, 67, 0.05, 1.0),
                (4, 66, 1.0, 0.5),
            ],
            4.0,
        );
        let opts = QuantizeOptions {
            key: KeySig::major(1), // G major: F#
            ..QuantizeOptions::default()
        };
        let score = quantize(&clip, &opts).unwrap();
        match &score.measures[0].events[0] {
            MeasureEvent::Chord(c) => {
                assert_eq!(c.notes.len(), 3);
                assert_eq!(c.notes.iter().map(|n| n.midi).collect::<Vec<_>>(), vec![60, 64, 67]);
            }
            other => panic!("want chord, got {other:?}"),
        }
        // F# spelled sharp-side in G major...
        let sharp = find_midi(&score, 66).unwrap();
        assert_eq!((sharp.pitch.step, sharp.pitch.alter), (super::super::model::Step::F, 1));
        // ...and flat-side (Gb) in F major.
        let opts_flat = QuantizeOptions {
            key: KeySig::major(-1),
            ..QuantizeOptions::default()
        };
        let flat_score = quantize(&clip, &opts_flat).unwrap();
        let flat = find_midi(&flat_score, 66).unwrap();
        assert_eq!((flat.pitch.step, flat.pitch.alter), (super::super::model::Step::G, -1));
    }

    fn find_midi(score: &Score, midi: u8) -> Option<NotationNote> {
        for m in &score.measures {
            for e in &m.events {
                match e {
                    MeasureEvent::Note(n) if n.midi == midi => return Some(n.clone()),
                    MeasureEvent::Chord(c) => {
                        if let Some(n) = c.notes.iter().find(|n| n.midi == midi) {
                            return Some(n.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    #[test]
    fn quantize_beams_eighth_runs_per_beat() {
        // Four straight eighths in beat 0..2, then quarters.
        let clip = clip_with(
            vec![
                (1, 60, 0.0, 0.5),
                (2, 62, 0.5, 0.5),
                (3, 64, 1.0, 0.5),
                (4, 65, 1.5, 0.5),
                (5, 67, 2.0, 1.0),
            ],
            4.0,
        );
        let score = quantize(&clip, &QuantizeOptions::default()).unwrap();
        let beams = &score.measures[0].beams;
        assert!(!beams.is_empty(), "eighth runs must beam");
        // Every beamed event is an eighth (or shorter) sounding event.
        for g in beams {
            assert!(g.events.len() >= 2);
            for &i in &g.events {
                assert!(!score.measures[0].events[i].is_rest());
            }
        }
        score.validate(4.0).unwrap();
    }

    #[test]
    fn quantize_rejects_bad_options() {
        let clip = clip_with(vec![(1, 60, 0.0, 1.0)], 4.0);
        let bad_grid = QuantizeOptions { grid_beats: 0.03, ..QuantizeOptions::default() };
        assert!(quantize(&clip, &bad_grid).is_err());
        let bad_meter = QuantizeOptions {
            meter: Meter::new(4, 3),
            ..QuantizeOptions::default()
        };
        assert!(quantize(&clip, &bad_meter).is_err());
    }

    #[test]
    fn midi_round_trip_preserves_written_part() {
        // Grid-aligned clip: quantize -> to_midi -> quantize is stable,
        // and to_midi pitches/onsets match the written notes.
        let clip = clip_with(
            vec![(1, 60, 0.0, 1.0), (2, 64, 1.0, 0.5), (3, 67, 1.5, 0.5), (4, 72, 2.0, 2.0)],
            4.0,
        );
        let opts = QuantizeOptions::default();
        let score = quantize(&clip, &opts).unwrap();
        let back = to_midi(&score);
        assert_eq!(back.notes.len(), 4);
        let pitches: Vec<u8> = back.notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 64, 67, 72]);
        assert!((back.notes[0].start_beats - 0.0).abs() < 1e-6);
        assert!((back.notes[3].len_beats - 2.0).abs() < 1e-6);
        // Re-quantizing the rendered clip gives the same score back.
        let score2 = quantize(&back, &opts).unwrap();
        assert_eq!(score, score2);
    }

    #[test]
    fn overlapping_notes_truncate_to_next_onset() {
        // Legato overlap: note 1 (0..2) is still held when note 2 starts.
        let clip = clip_with(vec![(1, 60, 0.0, 2.0), (2, 62, 1.0, 1.0)], 4.0);
        let score = quantize(&clip, &QuantizeOptions::default()).unwrap();
        let back = to_midi(&score);
        assert_eq!(back.notes.len(), 2);
        assert!((back.notes[0].len_beats - 1.0).abs() < 1e-6);
        assert!((back.notes[1].start_beats - 1.0).abs() < 1e-6);
    }

    #[test]
    fn empty_clip_yields_bar_of_rest() {
        let clip = MidiClip::new(4.0);
        let score = quantize(&clip, &QuantizeOptions::default()).unwrap();
        assert_eq!(score.measures.len(), 1);
        assert!(score.measures[0].events.iter().all(|e| e.is_rest()));
    }
}
