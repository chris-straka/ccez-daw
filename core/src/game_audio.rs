//! v1 game-audio model: interactive/adaptive game audio (additive; v0 untouched).
//!
//! These types are separate top-level documents with their own schema version
//! (`GAME_AUDIO_SCHEMA_VERSION = 1`). They reference v0 project data only by
//! string id (`clip_ids`, `target_node`), so the frozen v0 `Project` shape,
//! op log, IPC table, action registry, and MCP tool list are unchanged.
//! TypeScript + Zod v4 mirrors are emitted by `emit` (appended last, so every
//! previously emitted line renders byte-identically); human-readable tables
//! live in `contracts/game-state.md`, `contracts/adaptive-cue-schema.md`,
//! `contracts/sfx-bank-schema.md`, and `contracts/export-package.md`.

use serde::{Deserialize, Serialize};

/// Schema version stamped on every saved game-audio document.
/// Breaking change = new version + migration note.
pub const GAME_AUDIO_SCHEMA_VERSION: u32 = 1;

/// One named, ranged game-state parameter (the RTPC-style input).
/// Mirrors `Param`'s range metadata without sharing its type, so v0 never moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameStateParam {
    pub id: String,
    pub label: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: String,
}

/// One live value for a named game-state parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameStateValue {
    pub param: String,
    pub value: f64,
}

/// A named game state plus its continuous parameter values, e.g.
/// `state: "combat"` with `[{ param: "threat", value: 0.8 }]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameStateSnapshot {
    pub state: String,
    pub values: Vec<GameStateValue>,
}

/// How an adaptive cue moves from one game state to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionKind {
    Cut,
    Fade,
    BarWait,
    Stinger,
}

/// One vertical layer of an adaptive cue: a set of clips that sound together
/// while any of `states` is active (empty `states` = always active bed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueLayer {
    pub id: String,
    pub name: String,
    pub clip_ids: Vec<String>,
    pub states: Vec<String>,
    pub volume: f64,
}

/// One transition rule: when the game state moves `from_state` -> `to_state`,
/// apply `kind` over `fade_beats`; `stinger_cue_id` names the stinger clip
/// (empty unless `kind` is `Stinger`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionRule {
    pub id: String,
    pub from_state: String,
    pub to_state: String,
    pub kind: TransitionKind,
    pub fade_beats: f64,
    pub stinger_cue_id: String,
}

/// One adaptive music cue: vertical layers plus horizontal transition rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdaptiveCue {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub tempo: f64,
    pub default_state: String,
    pub layers: Vec<CueLayer>,
    pub transitions: Vec<TransitionRule>,
}

impl AdaptiveCue {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            schema_version: GAME_AUDIO_SCHEMA_VERSION,
            id: id.to_string(),
            name: name.to_string(),
            tempo: 120.0,
            default_state: "explore".to_string(),
            layers: Vec::new(),
            transitions: Vec::new(),
        }
    }

    /// Layers audible in `state` (always-on bed layers included).
    pub fn layers_for_state(&self, state: &str) -> Vec<&CueLayer> {
        self.layers
            .iter()
            .filter(|l| l.states.is_empty() || l.states.iter().any(|s| s == state))
            .collect()
    }

    /// First rule matching a state change, if any.
    pub fn transition_for(&self, from: &str, to: &str) -> Option<&TransitionRule> {
        self.transitions
            .iter()
            .find(|t| t.from_state == from && t.to_state == to)
    }
}

/// One RTPC-style binding: game parameter `param` drives
/// `target_node:target_param` across the `[min, max]` output range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpcBinding {
    pub param: String,
    pub target_node: String,
    pub target_param: String,
    pub min: f64,
    pub max: f64,
}

/// One playable SFX event: pick a clip, humanize, throttle, modulate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SfxEvent {
    pub id: String,
    pub name: String,
    pub clip_ids: Vec<String>,
    pub volume: f64,
    /// +/- range applied per trigger, in dB-equivalent linear units.
    pub volume_random: f64,
    /// +/- range applied per trigger, in semitones.
    pub pitch_random: f64,
    pub cooldown_ms: u64,
    pub max_polyphony: u32,
    pub rtpc: Vec<RtpcBinding>,
}

/// One named bank of SFX events shipped to the engine as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SfxBank {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub events: Vec<SfxEvent>,
}

impl SfxBank {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            schema_version: GAME_AUDIO_SCHEMA_VERSION,
            id: id.to_string(),
            name: name.to_string(),
            events: Vec::new(),
        }
    }
}

/// What one exported audio file came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StemKind {
    MusicLayer,
    SfxClip,
}

/// One rendered file inside an engine export package. Loop points are in
/// beats so the engine can convert at the cue tempo; empty loop (`0, 0`) =
/// one-shot, not a loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportStem {
    pub path: String,
    pub source_id: String,
    pub source_layer_id: String,
    pub kind: StemKind,
    pub loop_start_beats: f64,
    pub loop_end_beats: f64,
}

/// The engine deliverable: rendered stems plus the JSON event bank plus the
/// validator version that approved it. Paths are package-relative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPackage {
    pub schema_version: u32,
    pub name: String,
    pub cue_ids: Vec<String>,
    pub bank_ids: Vec<String>,
    pub stems: Vec<ExportStem>,
    pub event_bank_path: String,
    pub validator_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_cue_layers_and_transitions_resolve() {
        let mut cue = AdaptiveCue::new("cue_fight", "Fight");
        cue.layers.push(CueLayer {
            id: "bed".to_string(),
            name: "Bed".to_string(),
            clip_ids: vec!["clip_a".to_string()],
            states: vec![],
            volume: 0.8,
        });
        cue.layers.push(CueLayer {
            id: "drums".to_string(),
            name: "Drums".to_string(),
            clip_ids: vec!["clip_b".to_string()],
            states: vec!["combat".to_string()],
            volume: 0.9,
        });
        cue.transitions.push(TransitionRule {
            id: "t1".to_string(),
            from_state: "explore".to_string(),
            to_state: "combat".to_string(),
            kind: TransitionKind::Fade,
            fade_beats: 4.0,
            stinger_cue_id: String::new(),
        });
        assert_eq!(cue.layers_for_state("explore").len(), 1);
        assert_eq!(cue.layers_for_state("combat").len(), 2);
        let t = cue.transition_for("explore", "combat").expect("rule");
        assert_eq!(t.kind, TransitionKind::Fade);
        assert!(cue.transition_for("combat", "explore").is_none());
        let json = serde_json::to_string(&cue).expect("serialize");
        let back: AdaptiveCue = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cue, back);
    }

    #[test]
    fn sfx_bank_and_export_package_round_trip_through_json() {
        let bank = SfxBank {
            schema_version: GAME_AUDIO_SCHEMA_VERSION,
            id: "bank_ui".to_string(),
            name: "UI".to_string(),
            events: vec![SfxEvent {
                id: "ev_click".to_string(),
                name: "Click".to_string(),
                clip_ids: vec!["clip_c".to_string()],
                volume: 0.7,
                volume_random: 0.05,
                pitch_random: 1.0,
                cooldown_ms: 50,
                max_polyphony: 4,
                rtpc: vec![RtpcBinding {
                    param: "ui_speed".to_string(),
                    target_node: "bus_sfx".to_string(),
                    target_param: "volume".to_string(),
                    min: 0.5,
                    max: 1.0,
                }],
            }],
        };
        let pkg = ExportPackage {
            schema_version: GAME_AUDIO_SCHEMA_VERSION,
            name: "demo-v1".to_string(),
            cue_ids: vec!["cue_fight".to_string()],
            bank_ids: vec![bank.id.clone()],
            stems: vec![ExportStem {
                path: "stems/cue_fight_bed.wav".to_string(),
                source_id: "cue_fight".to_string(),
                source_layer_id: "bed".to_string(),
                kind: StemKind::MusicLayer,
                loop_start_beats: 0.0,
                loop_end_beats: 16.0,
            }],
            event_bank_path: "bank_ui.json".to_string(),
            validator_version: "1".to_string(),
        };
        for v in [
            serde_json::to_string(&bank).expect("serialize"),
            serde_json::to_string(&pkg).expect("serialize"),
        ] {
            assert!(v.contains(&GAME_AUDIO_SCHEMA_VERSION.to_string()));
        }
        let back: SfxBank =
            serde_json::from_str(&serde_json::to_string(&bank).expect("serialize"))
                .expect("deserialize");
        assert_eq!(bank, back);
    }

    #[test]
    fn game_state_snapshot_round_trips_through_json() {
        let snap = GameStateSnapshot {
            state: "combat".to_string(),
            values: vec![GameStateValue {
                param: "threat".to_string(),
                value: 0.8,
            }],
        };
        let json = serde_json::to_string(&snap).expect("serialize");
        let back: GameStateSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(snap, back);
    }
}
