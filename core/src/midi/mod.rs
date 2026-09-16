//! Track F: MIDI engine + MPE-ready model.
//!
//! MIDI lives *beside* the frozen project schema, not inside it. The v0
//! `Clip { kind: Midi, source }` shape stays untouched: a MIDI clip's
//! `source` names an opaque asset key (`take:<n>` / `<key>.mid.json`), and
//! the bytes behind that key are the JSON encoding of a [`MidiClip`]
//! defined here. That keeps the typegen drift gate green (`model.rs`,
//! `ipc.rs`, `emit.rs` untouched) while giving MIDI a real model.
//!
//! Layout:
//!
//! - [`MidiNote`](crate::midi::MidiNote): one note with performance fields
//!   (`probability`, `ratchets`, `timing_offset_beats`) plus per-note MPE
//!   expression (`pitch_bend`, `pressure`, `timbre`, per-note `channel`).
//! - [`clip`]: [`MidiClip`](crate::midi::MidiClip): sorted note list +
//!   JSON asset encode/decode + [`Engine`](crate::engine::Engine) save/load
//!   helpers (opaque `midi`-kind blobs).
//! - [`groove`]: [`GrooveTemplate`](crate::midi::GrooveTemplate) extract +
//!   transfer (amount-blended).
//! - [`transport`]: deterministic [`expand`](crate::midi::expand) for
//!   playback and [`Recorder`](crate::midi::Recorder) for record, so
//!   record(play(clip)) == clip round-trips.
//!
//! Reads the frozen `ClipKind::Midi` marker only; adds no IPC or
//! project-schema surface.

pub mod clip;
pub mod groove;
pub mod transport;

pub use clip::MidiClip;
pub use groove::{ApplyParams, GroovePool, GrooveTemplate, apply_full, extract};
pub use transport::{Recorder, ScheduledNote, expand};

use serde::{Deserialize, Serialize};

/// Per-note MPE expression, kept as plain `f64` fields on the note so the
/// model is MPE-ready without requiring an MPE deviceotland today:
///
/// - `pitch_bend`: semitones, ±48 (MPE pitch-bend range).
/// - `pressure`: 0..=1 (MPE pressure / aftertouch dimension).
/// - `timbre`: 0..=1 (MPE timbre / CC74 brightness dimension).
///
/// Channel 0 is the global (non-MPE / master) channel; member notes live on
/// channels 1..=15. [`assign_mpe_channels`] gives overlapping notes
/// distinct member channels, which is the one hard MPE requirement
/// (one channel per sounding note).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiNote {
    /// Stable per-note identity. Seeds the deterministic probability gate
    /// and survives [`assign_mpe_channels`].
    pub note_id: u32,
    /// MIDI pitch 0..=127 (60 = middle C).
    pub pitch: u8,
    /// MIDI velocity 1..=127 (0 is reserved for note-off; use `muted`).
    pub velocity: u8,
    /// Start in clip-local beats. May be negative by up to
    /// `timing_offset_beats` after groove/humanize shifts (clamped at render).
    pub start_beats: f64,
    /// Length in beats, must be > 0.
    pub len_beats: f64,
    /// MIDI channel 0..=15. MPE member notes use 1..=15.
    pub channel: u8,
    /// Playback probability 0..=1. 1 = always, 0 = never (audition only).
    /// Evaluated deterministically in [`expand`](crate::midi::expand).
    pub probability: f64,
    /// Ratchet subdivisions >= 1. 1 = plain note; N > 1 re-triggers the
    /// note N times evenly across its span.
    pub ratchets: u8,
    /// Microtiming offset in beats, ±0.25 (a 16th at 4/4). Applied after
    /// groove transfer; positive = late.
    pub timing_offset_beats: f64,
    /// Per-note MPE pitch bend in semitones, ±48.
    pub pitch_bend: f64,
    /// Per-note MPE pressure 0..=1.
    pub pressure: f64,
    /// Per-note MPE timbre (CC74) 0..=1.
    pub timbre: f64,
    /// Muted notes are kept for editing but never scheduled.
    pub muted: bool,
}

impl MidiNote {
    /// Minimal note: full probability, no ratchet, no offset, flat MPE.
    pub fn new(note_id: u32, pitch: u8, velocity: u8, start_beats: f64, len_beats: f64) -> Self {
        Self {
            note_id,
            pitch,
            velocity,
            start_beats,
            len_beats,
            channel: 0,
            probability: 1.0,
            ratchets: 1,
            timing_offset_beats: 0.0,
            pitch_bend: 0.0,
            pressure: 0.0,
            timbre: 0.0,
            muted: false,
        }
    }

    /// Field validation. Returns a human message on the first failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.pitch > 127 {
            return Err(format!("pitch {} out of range 0..=127", self.pitch));
        }
        if self.velocity == 0 || self.velocity > 127 {
            return Err(format!("velocity {} out of range 1..=127", self.velocity));
        }
        if !self.len_beats.is_finite() || self.len_beats <= 0.0 {
            return Err(format!("len_beats {} must be > 0", self.len_beats));
        }
        if !self.start_beats.is_finite() {
            return Err("start_beats must be finite".to_string());
        }
        if self.channel > 15 {
            return Err(format!("channel {} out of range 0..=15", self.channel));
        }
        if !(0.0..=1.0).contains(&self.probability) {
            return Err(format!("probability {} out of range 0..=1", self.probability));
        }
        if self.ratchets < 1 {
            return Err("ratchets must be >= 1".to_string());
        }
        if self.timing_offset_beats.abs() > 0.25 || !self.timing_offset_beats.is_finite() {
            return Err(format!(
                "timing_offset_beats {} out of range ±0.25",
                self.timing_offset_beats
            ));
        }
        if self.pitch_bend.abs() > 48.0 || !self.pitch_bend.is_finite() {
            return Err(format!("pitch_bend {} out of range ±48 semitones", self.pitch_bend));
        }
        if !(0.0..=1.0).contains(&self.pressure) {
            return Err(format!("pressure {} out of range 0..=1", self.pressure));
        }
        if !(0.0..=1.0).contains(&self.timbre) {
            return Err(format!("timbre {} out of range 0..=1", self.timbre));
        }
        Ok(())
    }
}

/// Give every note that overlaps in time its own MPE member channel.
///
/// MPE's one hard rule: one channel per sounding note, so per-note pitch
/// bend / pressure / timbre stay independent. `members` lists the member
/// channels to deal from (default `1..=7` lower zone; pass `9..=15` for an
/// upper zone; channel 0 is always left as the master). Notes are dealt in
/// start order; overlapping notes never share a channel while enough
/// members exist. When overlaps outnumber members, channels are reused by
/// earliest-finishing note (documented overflow, not an error).
/// Non-overlapping notes reuse channel 1 implicitly via the same rule.
pub fn assign_mpe_channels(clip: &mut MidiClip, members: &[u8]) {
    let members: Vec<u8> = if members.is_empty() {
        (1..=7).collect()
    } else {
        members.to_vec()
    };
    let mut order: Vec<usize> = (0..clip.notes.len()).collect();
    order.sort_by(|&a, &b| {
        clip.notes[a]
            .start_beats
            .partial_cmp(&clip.notes[b].start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Active voice per member channel: when it ends (beats).
    let mut busy_until = vec![f64::NEG_INFINITY; members.len()];
    for idx in order {
        let (start, end) = {
            let n = &clip.notes[idx];
            (n.start_beats + n.timing_offset_beats, n.start_beats + n.len_beats)
        };
        // First free channel wins; else steal the earliest-finishing one.
        let mut pick = 0;
        let mut free = false;
        for (i, busy) in busy_until.iter().enumerate() {
            if *busy <= start {
                pick = i;
                free = true;
                break;
            }
        }
        if !free {
            let mut earliest = 0;
            for i in 1..busy_until.len() {
                if busy_until[i] < busy_until[earliest] {
                    earliest = i;
                }
            }
            pick = earliest;
        }
        busy_until[pick] = end;
        clip.notes[idx].channel = members[pick];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_validation_rejects_bad_fields() {
        let mut n = MidiNote::new(1, 60, 100, 0.0, 1.0);
        assert!(n.validate().is_ok());
        n.velocity = 0;
        assert!(n.validate().is_err());
        n.velocity = 100;
        n.ratchets = 0;
        assert!(n.validate().is_err());
        n.ratchets = 1;
        n.probability = 2.0;
        assert!(n.validate().is_err());
        n.probability = 1.0;
        n.timing_offset_beats = 1.0;
        assert!(n.validate().is_err());
        n.timing_offset_beats = 0.0;
        n.pitch_bend = 49.0;
        assert!(n.validate().is_err());
    }

    #[test]
    fn mpe_assignment_gives_overlaps_distinct_channels() {
        let mut clip = MidiClip::new(4.0);
        clip.add_note(MidiNote::new(1, 60, 100, 0.0, 2.0)).unwrap();
        clip.add_note(MidiNote::new(2, 64, 100, 0.5, 2.0)).unwrap();
        clip.add_note(MidiNote::new(3, 67, 100, 3.0, 0.5)).unwrap();
        assign_mpe_channels(&mut clip, &[1, 2]);
        let ch: Vec<u8> = clip.notes.iter().map(|n| n.channel).collect();
        assert_ne!(ch[0], ch[1], "overlapping notes share channel {ch:?}");
        assert!(ch.iter().all(|c| *c == 1 || *c == 2));
    }
}
