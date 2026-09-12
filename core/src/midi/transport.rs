//! Playback expand + record round-trip.
//!
//! Playback and record are inverse views over the same beat timeline:
//!
//! - [`expand`] turns a [`MidiClip`] into sorted [`ScheduledNote`]s for the
//!   renderer: muted notes vanish, `probability` gates deterministically,
//!   `ratchets` subdivide, `timing_offset_beats` shifts onsets (clamped at
//!   0 — a clip cannot start before its own beat 0).
//! - [`Recorder`] turns timestamped note-on/off pairs back into a
//!   [`MidiClip`]. Feeding `expand` output through a `Recorder` returns an
//!   equivalent clip (the `midi_record_playback_round_trip` test proves it).
//!
//! Probability uses a hash gate, not an RNG: `hash(note_id, seed)` decides,
//! so the same `(clip, seed)` always renders the same take and no RNG
//! dependency enters `core`.

use std::collections::HashMap;

use super::{MidiClip, MidiNote};

/// One sounding note after performance expansion: absolute (clip-local)
/// onset/offset in beats, carrying its MPE expression through.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledNote {
    pub note_id: u32,
    pub pitch: u8,
    pub velocity: u8,
    pub on_beats: f64,
    pub off_beats: f64,
    pub channel: u8,
    pub pitch_bend: f64,
    pub pressure: f64,
    pub timbre: f64,
}

/// Deterministic 64-bit mix (splitmix64 finalizer over `(note_id, seed)`).
/// Uniform in `[0, 1)`: the note sounds when `gate < probability`.
fn gate(note_id: u32, seed: u64) -> f64 {
    let mut z = (note_id as u64).wrapping_add(0x9E3779B97F4A7C15).wrapping_add(seed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    // 53-bit mantissa → [0,1).
    ((z >> 11) as f64) / ((1u64 << 53) as f64)
}

/// Expand a clip to its sounding notes for one playback pass.
///
/// - Muted notes and `probability`-gated notes (gate >= probability) are
///   skipped. `probability <= 0` never sounds; `>= 1` always sounds.
/// - `ratchets = N` splits the span into N equal sub-notes (each `len / N`
///   long, onsets at `start + i * len / N`); sub-notes share `note_id`
///   (they are one performance gesture, not N identities).
/// - Onsets shift by `timing_offset_beats` and clamp at 0; offsets preserve
///   the sub-note length.
/// - Output is sorted by `(on_beats, pitch, note_id, off_beats)`.
pub fn expand(clip: &MidiClip, seed: u64) -> Vec<ScheduledNote> {
    let mut out = Vec::new();
    for note in &clip.notes {
        if note.muted {
            continue;
        }
        if note.probability <= 0.0 {
            continue;
        }
        if note.probability < 1.0 && gate(note.note_id, seed) >= note.probability {
            continue;
        }
        let n = note.ratchets.max(1) as usize;
        let sub = note.len_beats / n as f64;
        for i in 0..n {
            let on = (note.start_beats + i as f64 * sub + note.timing_offset_beats).max(0.0);
            out.push(ScheduledNote {
                note_id: note.note_id,
                pitch: note.pitch,
                velocity: note.velocity,
                on_beats: on,
                off_beats: on + sub,
                channel: note.channel,
                pitch_bend: note.pitch_bend,
                pressure: note.pressure,
                timbre: note.timbre,
            });
        }
    }
    out.sort_by(|a, b| {
        a.on_beats
            .partial_cmp(&b.on_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
            .then(a.note_id.cmp(&b.note_id))
            .then(
                a.off_beats
                    .partial_cmp(&b.off_beats)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    out
}

/// Live-record collector: `note_on` opens a voice, `note_off` closes it
/// into a [`MidiNote`]. Voices key on `(pitch, channel)` — the MPE-correct
/// key, since one channel carries at most one sounding note.
#[derive(Debug, Default)]
pub struct Recorder {
    length_beats: f64,
    next_id: u32,
    open: HashMap<(u8, u8), RecorderVoice>,
    notes: Vec<MidiNote>,
}

#[derive(Debug, Clone)]
struct RecorderVoice {
    on_beats: f64,
    velocity: u8,
    pitch_bend: f64,
    pressure: f64,
    timbre: f64,
}

impl Recorder {
    pub fn new(length_beats: f64) -> Self {
        Self {
            length_beats,
            next_id: 1,
            open: HashMap::new(),
            notes: Vec::new(),
        }
    }

    /// Open a voice at `at_beats`. A second on without an off re-triggers:
    /// the stale voice is closed at the new onset (never silently dropped).
    pub fn note_on(
        &mut self,
        pitch: u8,
        velocity: u8,
        channel: u8,
        at_beats: f64,
        pitch_bend: f64,
        pressure: f64,
        timbre: f64,
    ) {
        if let Some(stale) = self.open.remove(&(pitch, channel)) {
            self.close_voice(pitch, channel, stale, at_beats);
        }
        self.open.insert(
            (pitch, channel),
            RecorderVoice {
                on_beats: at_beats,
                velocity: velocity.clamp(1, 127),
                pitch_bend: pitch_bend.clamp(-48.0, 48.0),
                pressure: pressure.clamp(0.0, 1.0),
                timbre: timbre.clamp(0.0, 1.0),
            },
        );
    }

    /// Close a voice; unknown pairs (stray offs) are ignored.
    pub fn note_off(&mut self, pitch: u8, channel: u8, at_beats: f64) {
        if let Some(voice) = self.open.remove(&(pitch, channel)) {
            self.close_voice(pitch, channel, voice, at_beats);
        }
    }

    fn close_voice(&mut self, pitch: u8, channel: u8, voice: RecorderVoice, at_beats: f64) {
        let len = (at_beats - voice.on_beats).max(1.0 / 480.0); // 1 tick floor
        let id = self.next_id;
        self.next_id += 1;
        self.notes.push(MidiNote {
            note_id: id,
            pitch: pitch.min(127),
            velocity: voice.velocity,
            start_beats: voice.on_beats.max(0.0),
            len_beats: len,
            channel: channel.min(15),
            probability: 1.0,
            ratchets: 1,
            timing_offset_beats: 0.0,
            pitch_bend: voice.pitch_bend,
            pressure: voice.pressure,
            timbre: voice.timbre,
            muted: false,
        });
    }

    /// Close all still-open voices at `at_beats` and return the clip
    /// (sorted, like every [`MidiClip`]).
    pub fn finish(mut self, at_beats: f64) -> MidiClip {
        let open: Vec<((u8, u8), RecorderVoice)> = self.open.drain().collect();
        for ((pitch, channel), voice) in open {
            self.close_voice(pitch, channel, voice, at_beats);
        }
        let mut clip = MidiClip::new(self.length_beats);
        for n in self.notes {
            // Recorder output is valid by construction; skip duplicates only.
            let _ = clip.add_note(n);
        }
        clip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probability_gate_is_deterministic_and_bounded() {
        let mut clip = MidiClip::new(4.0);
        clip.add_note(MidiNote::new(7, 60, 100, 0.0, 1.0)).unwrap();
        clip.notes[0].probability = 0.5;
        let a = expand(&clip, 42);
        let b = expand(&clip, 42);
        assert_eq!(a, b, "same seed must render the same take");
        // A zero-probability note never sounds; full probability always does.
        clip.notes[0].probability = 0.0;
        assert!(expand(&clip, 42).is_empty());
        clip.notes[0].probability = 1.0;
        assert_eq!(expand(&clip, 42).len(), 1);
    }

    #[test]
    fn stray_note_off_is_ignored() {
        let mut rec = Recorder::new(4.0);
        rec.note_off(60, 0, 1.0);
        let clip = rec.finish(2.0);
        assert!(clip.notes.is_empty());
    }
}
