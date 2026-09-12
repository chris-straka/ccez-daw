//! v0 project model: universal node model + event-sourced op log.
//!
//! Clips, tracks, devices, and modulators share one addressable node model.
//! Audio, MIDI, automation, and modulation are views over it, not separate
//! type systems. v1 implements volume/pan/device params over this addressing;
//! the anything-modulates-anything wishlist generalizes the same addressing.
//!
//! Every type below is mirrored to TypeScript by `emit` from a single
//! field declaration, so the Zod v4 schemas cannot drift from the interfaces.

use serde::{Deserialize, Serialize};

/// Frozen contract version. Breaking change = new version + migration note.
pub const CONTRACT_VERSION: &str = "v0";
/// Integer schema version stamped on every saved project.
pub const SCHEMA_VERSION: u32 = 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Track,
    Clip,
    Device,
    Modulator,
    Bus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Audio,
    Midi,
    Modulation,
    Sidechain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKind {
    Audio,
    Midi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineState {
    Stopped,
    Playing,
    Recording,
}

/// Universal address of one automatable/modulatable parameter.
/// `node` is a node id, `param` is a param id on that node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamAddress {
    pub node: String,
    pub param: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub id: String,
    pub label: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: String,
}

/// One addressable object: a track, clip, device, modulator, or bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub params: Vec<Param>,
}

/// One routing-graph edge. The mixer is a view over these edges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    pub track_id: String,
    pub name: String,
    pub start_beats: f64,
    pub length_beats: f64,
    pub kind: ClipKind,
    /// Opaque source reference (sample path, take id, MIDI blob key).
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub volume: f64,
    pub pan: f64,
    pub muted: bool,
    pub solo: bool,
    pub clip_ids: Vec<String>,
    pub device_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationPoint {
    pub beat: f64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationLane {
    pub id: String,
    pub target: ParamAddress,
    pub points: Vec<AutomationPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub tempo: f64,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub tracks: Vec<Track>,
    pub clips: Vec<Clip>,
    pub devices: Vec<Node>,
    pub routing: Vec<Edge>,
    pub automation: Vec<AutomationLane>,
}

/// One entry of the event-sourced op log. `target` is a node id, track id,
/// clip id, or param address (`node:param`); `value_json` carries the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpKind {
    TrackAdded,
    ClipAdded,
    ClipMoved,
    ParamSet,
    TempoSet,
    UndoMarker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Op {
    pub seq: u64,
    pub actor: String,
    pub kind: OpKind,
    pub target: String,
    pub value_json: String,
}

impl Project {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: id.to_string(),
            name: name.to_string(),
            tempo: 120.0,
            time_sig_num: 4,
            time_sig_den: 4,
            tracks: Vec::new(),
            clips: Vec::new(),
            devices: Vec::new(),
            routing: Vec::new(),
            automation: Vec::new(),
        }
    }

    /// Minimal demoable project: metronome track + two clips (Track B slice).
    pub fn sample() -> Self {
        let mut p = Self::new("proj_sample", "Untitled");
        p.tracks.push(Track {
            id: "trk_click".to_string(),
            name: "Click".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec!["clip_a".to_string()],
            device_ids: vec![],
        });
        p.tracks.push(Track {
            id: "trk_music".to_string(),
            name: "Music".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec!["clip_b".to_string()],
            device_ids: vec![],
        });
        p.clips.push(Clip {
            id: "clip_a".to_string(),
            track_id: "trk_click".to_string(),
            name: "Count-in".to_string(),
            start_beats: 0.0,
            length_beats: 4.0,
            kind: ClipKind::Audio,
            source: "builtin:click".to_string(),
        });
        p.clips.push(Clip {
            id: "clip_b".to_string(),
            track_id: "trk_music".to_string(),
            name: "Sketch".to_string(),
            start_beats: 4.0,
            length_beats: 8.0,
            kind: ClipKind::Midi,
            source: "take:1".to_string(),
        });
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_project_round_trips_through_json() {
        let p = Project::sample();
        let json = serde_json::to_string(&p).expect("serialize");
        let back: Project = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, back);
        assert_eq!(back.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn op_log_entry_round_trips_through_json() {
        let op = Op {
            seq: 1,
            actor: "ui".to_string(),
            kind: OpKind::ParamSet,
            target: "trk_music:volume".to_string(),
            value_json: "0.5".to_string(),
        };
        let json = serde_json::to_string(&op).expect("serialize");
        let back: Op = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(op, back);
    }
}
