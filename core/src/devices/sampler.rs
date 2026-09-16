//! Simpler-style sampler model: sample buffers plus the drum-pad map.
//!
//! Teaching note: a frozen [`Node`](crate::model::Node) carries numeric
//! params only, so it cannot hold audio. The sampler splits the same way
//! MIDI does: the `Node` holds the play params (`transpose`, `gain`,
//! `attack`, `release`, `cutoff` — see [`crate::devices::class`]) while
//! the audio lives *beside* the project. [`SampleBuffer`] is the JSON
//! encoding behind one `audio`-kind engine asset; [`SampleBank`] maps
//! sampler device ids to their buffers (the [`Rack`](super::rack::Rack)
//! sidecar precedent: structure riding alongside the project, never in
//! it). [`DrumRack`] maps 16 drum pads to `(track, note)` targets with
//! per-pad trim, so one pad strike knows which voice to trigger.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{ASSET_KIND_AUDIO, Engine};

/// Hard ceiling for sample buffers (10 M frames ≈ 3.8 min at 44.1 kHz
/// mono): decode refuses anything larger rather than ballooning memory
/// on a hand-edited asset.
pub const MAX_SAMPLE_FRAMES: usize = 10_000_000;

/// Number of drum pads in a [`DrumRack`]. Pads are `0..16` (classic
/// 4x4); MIDI notes stay full-range `0..=127`.
pub const DRUM_PAD_COUNT: u8 = 16;

/// One mono sample: owning rate plus frames. The renderer resamples from
/// this at the device's `transpose` pitch (see
/// [`crate::devices::kernel::sampler_process`]); a rate mismatch with the
/// engine is just playback-speed truth, not an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleBuffer {
    pub sample_rate: f64,
    pub frames: Vec<f32>,
}

impl SampleBuffer {
    pub fn new(sample_rate: f64, frames: Vec<f32>) -> Result<Self, String> {
        let buf = Self { sample_rate, frames };
        buf.validate()?;
        Ok(buf)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            return Err(format!("sample_rate {} must be positive", self.sample_rate));
        }
        if self.frames.len() > MAX_SAMPLE_FRAMES {
            return Err(format!(
                "sample has {} frames, limit is {MAX_SAMPLE_FRAMES}",
                self.frames.len()
            ));
        }
        if self.frames.iter().any(|f| !f.is_finite()) {
            return Err("sample frames must be finite".to_string());
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Serialize to the bytes stored behind an `audio`-kind engine asset.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| format!("sample encode: {e}"))
    }

    /// Parse bytes previously produced by [`SampleBuffer::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let buf: SampleBuffer =
            serde_json::from_slice(bytes).map_err(|e| format!("sample decode: {e}"))?;
        buf.validate()?;
        Ok(buf)
    }

    /// Persist behind `key` as an `audio`-kind engine asset.
    pub fn save_to_engine(&self, engine: &mut Engine, key: &str) -> Result<(), String> {
        let bytes = self.encode()?;
        engine
            .store_asset(key, ASSET_KIND_AUDIO, &bytes)
            .map_err(|e| format!("sample store_asset: {e}"))
    }

    /// Load bytes stored by [`SampleBuffer::save_to_engine`].
    pub fn load_from_engine(engine: &Engine, key: &str) -> Result<Self, String> {
        let bytes = engine
            .load_asset(key)
            .map_err(|e| format!("sample load_asset: {e}"))?;
        Self::decode(&bytes)
    }
}

/// Device id → sample buffer. Missing ids render silence (a setup gap,
/// not a corrupt block); empty buffers do too.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SampleBank {
    pub samples: BTreeMap<String, SampleBuffer>,
}

impl SampleBank {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, device_id: &str, sample: SampleBuffer) {
        self.samples.insert(device_id.to_string(), sample);
    }

    pub fn get(&self, device_id: &str) -> Option<&SampleBuffer> {
        self.samples.get(device_id)
    }

    pub fn remove(&mut self, device_id: &str) -> bool {
        self.samples.remove(device_id).is_some()
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("bank encode: {e}"))
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let bank: SampleBank =
            serde_json::from_str(json).map_err(|e| format!("bank decode: {e}"))?;
        for buf in bank.samples.values() {
            buf.validate()?;
        }
        Ok(bank)
    }
}

/// One drum pad: which MIDI `note` on which `track_id` it strikes, plus a
/// per-pad `gain` trim and `transpose` nudge (the pad's tuning against
/// the voice's own params — both apply).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrumPad {
    pub pad: u8,
    pub track_id: String,
    pub note: u8,
    pub gain: f64,
    pub transpose: f64,
}

impl DrumPad {
    pub fn new(
        pad: u8,
        track_id: &str,
        note: u8,
        gain: f64,
        transpose: f64,
    ) -> Result<Self, String> {
        let p = Self {
            pad,
            track_id: track_id.to_string(),
            note,
            gain,
            transpose,
        };
        p.validate()?;
        Ok(p)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.pad >= DRUM_PAD_COUNT {
            return Err(format!("pad {} out of range 0..16", self.pad));
        }
        // MIDI notes are 0..=127 (the field stays `u8` to match `MidiNote`).
        if self.note > 127 {
            return Err(format!("note {} out of range 0..=127", self.note));
        }
        if self.track_id.is_empty() {
            return Err("drum pad track_id must be non-empty".to_string());
        }
        if !self.gain.is_finite() || !(0.0..=4.0).contains(&self.gain) {
            return Err(format!("pad gain {} out of range 0..=4", self.gain));
        }
        if !self.transpose.is_finite() || self.transpose.abs() > 48.0 {
            return Err(format!("pad transpose {} out of range ±48", self.transpose));
        }
        Ok(())
    }
}

/// The 16-pad map: pad index → [`DrumPad`]. Unmapped pads are silent
/// (like missing sampler buffers — setup gaps, never errors). Two pads
/// may share a note (layered strikes); [`DrumRack::pads_for_note`]
/// returns every pad striking it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DrumRack {
    pub pads: BTreeMap<u8, DrumPad>,
}

impl DrumRack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Map (or remap) one pad. The key always equals `pad.pad`.
    pub fn set_pad(&mut self, pad: DrumPad) -> Result<(), String> {
        pad.validate()?;
        self.pads.insert(pad.pad, pad);
        Ok(())
    }

    /// Unmap one pad. Returns `true` when something was removed.
    pub fn clear_pad(&mut self, pad: u8) -> bool {
        self.pads.remove(&pad).is_some()
    }

    /// What does this pad strike? `None` = unmapped = silent.
    pub fn strike(&self, pad: u8) -> Option<&DrumPad> {
        self.pads.get(&pad)
    }

    /// Every mapped pad striking `note` (layered voices), in pad order.
    pub fn pads_for_note(&self, note: u8) -> Vec<&DrumPad> {
        let mut out: Vec<&DrumPad> =
            self.pads.values().filter(|p| p.note == note).collect();
        out.sort_by_key(|p| p.pad);
        out
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("drum rack encode: {e}"))
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let rack: DrumRack =
            serde_json::from_str(json).map_err(|e| format!("drum rack decode: {e}"))?;
        for pad in rack.pads.values() {
            pad.validate()?;
        }
        // Keys must match their pads (hand-edited JSON still loads sound).
        for (key, pad) in &rack.pads {
            if *key != pad.pad {
                return Err(format!("pad key {key} != pad {}", pad.pad));
            }
        }
        Ok(rack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_buffer_validates_and_round_trips() {
        let buf = SampleBuffer::new(44100.0, vec![0.0, 0.5, -0.5]).expect("buffer");
        assert_eq!(buf.len(), 3);
        assert!(!buf.is_empty());
        let bytes = buf.encode().expect("encode");
        assert_eq!(SampleBuffer::decode(&bytes).expect("decode"), buf);
        assert!(SampleBuffer::new(0.0, vec![1.0]).is_err());
        assert!(SampleBuffer::new(44100.0, vec![f32::NAN]).is_err());
        assert!(SampleBuffer::decode(b"not json").is_err());
    }

    #[test]
    fn sample_bank_crud_and_json() {
        let mut bank = SampleBank::new();
        assert!(bank.get("s1").is_none());
        bank.insert("s1", SampleBuffer::new(48000.0, vec![1.0]).expect("buf"));
        assert_eq!(bank.get("s1").expect("hit").sample_rate, 48000.0);
        let json = bank.to_json().expect("json");
        assert_eq!(SampleBank::from_json(&json).expect("parse"), bank);
        assert!(bank.remove("s1"));
        assert!(!bank.remove("s1"));
        assert!(SampleBank::from_json("{bad").is_err());
    }

    #[test]
    fn drum_pad_validation_rejects_bad_fields() {
        assert!(DrumPad::new(0, "trk", 36, 1.0, 0.0).is_ok());
        assert!(DrumPad::new(16, "trk", 36, 1.0, 0.0).is_err());
        // 200 fits a u8 but is no MIDI note: parsing succeeds and
        // validation rejects it (same split as `MidiNote::validate`).
        let bad: DrumPad = serde_json::from_str(
            r#"{"pad":0,"track_id":"trk","note":200,"gain":1.0,"transpose":0.0}"#,
        )
        .expect("parse");
        assert!(bad.validate().is_err());
        assert!(DrumPad::new(0, "", 36, 1.0, 0.0).is_err());
        assert!(DrumPad::new(0, "trk", 36, 5.0, 0.0).is_err());
        assert!(DrumPad::new(0, "trk", 36, 1.0, 49.0).is_err());
    }

    #[test]
    fn drum_rack_maps_pads_to_notes_and_back() {
        let mut rack = DrumRack::new();
        assert!(rack.strike(0).is_none());
        rack.set_pad(DrumPad::new(0, "drums", 36, 1.0, 0.0).expect("pad")).expect("set");
        rack.set_pad(DrumPad::new(1, "drums", 36, 0.5, 12.0).expect("pad")).expect("set");
        rack.set_pad(DrumPad::new(15, "perc", 42, 1.0, 0.0).expect("pad")).expect("set");
        // Pad -> (track, note).
        let hit = rack.strike(1).expect("hit");
        assert_eq!((hit.track_id.as_str(), hit.note), ("drums", 36));
        // Note -> pads (layered kick on 0 and 1, pad order).
        let layers: Vec<u8> = rack.pads_for_note(36).iter().map(|p| p.pad).collect();
        assert_eq!(layers, vec![0, 1]);
        assert!(rack.pads_for_note(99).is_empty());
        // Remap and clear.
        rack.set_pad(DrumPad::new(0, "bass", 40, 1.0, 0.0).expect("pad")).expect("remap");
        assert_eq!(rack.strike(0).expect("hit").track_id, "bass");
        assert!(rack.clear_pad(0));
        assert!(!rack.clear_pad(0));
        // JSON round-trip; bad payloads rejected.
        let json = rack.to_json().expect("json");
        assert_eq!(DrumRack::from_json(&json).expect("parse"), rack);
        assert!(DrumRack::from_json("{bad").is_err());
        // Mismatched key rejected.
        let mut raw = serde_json::to_value(&rack).expect("value");
        raw["pads"]["7"] = raw["pads"]["1"].clone();
        assert!(DrumRack::from_json(&serde_json::to_string(&raw).expect("s")).is_err());
    }
}
