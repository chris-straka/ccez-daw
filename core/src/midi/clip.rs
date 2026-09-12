//! [`MidiClip`]: the sorted note list behind one MIDI clip asset.
//!
//! A v0 [`Clip`](crate::model::Clip) with `kind: Midi` carries only an
//! opaque `source` string. These bytes are the JSON encoding produced by
//! [`MidiClip::encode`] / [`MidiClip::decode`] (stored via the engine's
//! `midi`-kind asset API, so bundles carry MIDI for free and opening a
//! project never parses a note).

use serde::{Deserialize, Serialize};

use super::MidiNote;
use crate::engine::{ASSET_KIND_MIDI, Engine};

/// One MIDI clip: notes in clip-local beats plus the clip length.
/// `notes` is always kept sorted by `(start_beats, pitch, note_id)` so
/// encode → decode → encode is byte-stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiClip {
    pub length_beats: f64,
    pub notes: Vec<MidiNote>,
}

impl MidiClip {
    pub fn new(length_beats: f64) -> Self {
        Self {
            length_beats,
            notes: Vec::new(),
        }
    }

    /// Validate one note and insert it, keeping sort order.
    pub fn add_note(&mut self, note: MidiNote) -> Result<(), String> {
        note.validate()?;
        if self.notes.iter().any(|n| n.note_id == note.note_id) {
            return Err(format!("duplicate note_id {}", note.note_id));
        }
        self.notes.push(note);
        self.sort();
        Ok(())
    }

    /// Remove by `note_id`. Returns `true` when something was removed.
    pub fn remove_note(&mut self, note_id: u32) -> bool {
        let before = self.notes.len();
        self.notes.retain(|n| n.note_id != note_id);
        self.notes.len() != before
    }

    fn sort(&mut self) {
        self.notes.sort_by(|a, b| {
            a.start_beats
                .partial_cmp(&b.start_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.pitch.cmp(&b.pitch))
                .then(a.note_id.cmp(&b.note_id))
        });
    }

    /// Serialize to the opaque bytes stored behind a clip `source` key.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| format!("midi encode: {e}"))
    }

    /// Parse bytes previously produced by [`MidiClip::encode`], validating
    /// every note and re-sorting (so hand-edited JSON still loads sorted).
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut clip: MidiClip =
            serde_json::from_slice(bytes).map_err(|e| format!("midi decode: {e}"))?;
        if !clip.length_beats.is_finite() || clip.length_beats <= 0.0 {
            return Err(format!("length_beats {} must be > 0", clip.length_beats));
        }
        let mut seen = std::collections::BTreeSet::new();
        for n in &clip.notes {
            n.validate()?;
            if !seen.insert(n.note_id) {
                return Err(format!("duplicate note_id {}", n.note_id));
            }
        }
        clip.sort();
        Ok(clip)
    }

    /// Persist behind `key` as a `midi`-kind engine asset (bundle-portable).
    pub fn save_to_engine(&self, engine: &mut Engine, key: &str) -> Result<(), String> {
        let bytes = self.encode()?;
        engine
            .store_asset(key, ASSET_KIND_MIDI, &bytes)
            .map_err(|e| format!("midi store_asset: {e}"))
    }

    /// Load bytes stored by [`MidiClip::save_to_engine`] (lazy: call only
    /// when the clip is actually opened for edit or render).
    pub fn load_from_engine(engine: &Engine, key: &str) -> Result<Self, String> {
        let bytes = engine
            .load_asset(key)
            .map_err(|e| format!("midi load_asset: {e}"))?;
        Self::decode(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rejects_bad_payloads() {
        assert!(MidiClip::decode(b"not json").is_err());
        let mut clip = MidiClip::new(4.0);
        let mut bad = MidiNote::new(1, 60, 100, 0.0, 1.0);
        bad.probability = 5.0;
        // Bypass add_note validation to test decode-time validation.
        clip.notes.push(bad);
        let bytes = serde_json::to_vec(&clip).unwrap();
        assert!(MidiClip::decode(&bytes).is_err());
    }

    #[test]
    fn remove_note_reports_absence() {
        let mut clip = MidiClip::new(4.0);
        clip.add_note(MidiNote::new(1, 60, 100, 0.0, 1.0)).unwrap();
        assert!(clip.remove_note(1));
        assert!(!clip.remove_note(1));
    }
}
