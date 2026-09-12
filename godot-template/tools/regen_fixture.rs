//! Temporary S-1 fixture generator: builds the overworld demo package
//! through the real DAW export path (`build_package` + `write_package`)
//! plus the validator context bundle. Deleted after the fixture is built;
//! the checked-in regeneration copy lives at
//! `godot-template/tools/regen_fixture.rs`.

use ccez_core::gaexport::{build_package, write_package, ExportOptions, ExportRequest};
use ccez_core::game_audio::{
    AdaptiveCue, CueLayer, GameStateParam, RtpcBinding, SfxBank, SfxEvent, TransitionKind,
    TransitionRule, GAME_AUDIO_SCHEMA_VERSION,
};
use ccez_core::model::{Clip, ClipKind, Node, NodeKind, Param, Project};
use std::path::Path;

fn clip(id: &str, length_beats: f64) -> Clip {
    Clip {
        id: id.to_string(),
        track_id: "trk_1".to_string(),
        name: id.to_string(),
        start_beats: 0.0,
        length_beats,
        kind: ClipKind::Audio,
        source: format!("samples/{id}.wav"),
    }
}

fn param(id: &str, label: &str, min: f64, max: f64, default: f64, unit: &str) -> GameStateParam {
    GameStateParam {
        id: id.to_string(),
        label: label.to_string(),
        min,
        max,
        default,
        unit: unit.to_string(),
    }
}

fn main() {
    let mut project = Project::new("proj_overworld", "Overworld Demo");
    project.tempo = 100.0;
    for (id, len) in [
        ("clip_bed", 8.0),
        ("clip_drums", 4.0),
        ("clip_drone", 8.0),
        ("clip_brass", 4.0),
        ("clip_pad", 8.0),
        ("clip_bell", 8.0),
        ("clip_step_a", 1.0),
        ("clip_step_b", 1.0),
        ("clip_swing_a", 1.0),
        ("clip_swing_b", 1.0),
        ("clip_click", 1.0),
        ("clip_gallop", 1.0),
    ] {
        project.clips.push(clip(id, len));
    }
    project.devices.push(Node {
        id: "bus_sfx".to_string(),
        kind: NodeKind::Bus,
        name: "SFX bus".to_string(),
        params: vec![Param {
            id: "volume".to_string(),
            label: "Volume".to_string(),
            value: 0.8,
            min: 0.0,
            max: 1.0,
            default: 0.8,
            unit: String::new(),
        }],
    });

    let mut cue = AdaptiveCue::new("cue_overworld", "Overworld");
    cue.tempo = 100.0;
    cue.default_state = "field".to_string();
    let layers = [
        ("bed", "Bed", vec![], "clip_bed", 0.8),
        ("drums", "Drums", vec!["combat", "boss"], "clip_drums", 0.9),
        ("drone", "Dungeon drone", vec!["dungeon"], "clip_drone", 0.7),
        ("brass", "Boss brass", vec!["boss"], "clip_brass", 0.9),
        ("pad", "Calm pad", vec!["village", "night"], "clip_pad", 0.7),
        ("bell", "Night bell", vec!["night"], "clip_bell", 0.6),
    ];
    for (id, name, states, clip_id, volume) in layers {
        cue.layers.push(CueLayer {
            id: id.to_string(),
            name: name.to_string(),
            clip_ids: vec![clip_id.to_string()],
            states: states.iter().map(|s| s.to_string()).collect(),
            volume,
        });
    }
    let transitions = [
        ("t_field_combat", "field", "combat", TransitionKind::Fade, 2.0),
        ("t_combat_field", "combat", "field", TransitionKind::Fade, 2.0),
        ("t_field_dungeon", "field", "dungeon", TransitionKind::Fade, 4.0),
        ("t_dungeon_field", "dungeon", "field", TransitionKind::Fade, 4.0),
        ("t_any_boss", "field", "boss", TransitionKind::Fade, 1.0),
        ("t_field_village", "field", "village", TransitionKind::Fade, 2.0),
        ("t_field_night", "field", "night", TransitionKind::Fade, 4.0),
    ];
    for (id, from, to, kind, fade) in transitions {
        cue.transitions.push(TransitionRule {
            id: id.to_string(),
            from_state: from.to_string(),
            to_state: to.to_string(),
            kind,
            fade_beats: fade,
            stinger_cue_id: String::new(),
        });
    }

    let bank = SfxBank {
        schema_version: GAME_AUDIO_SCHEMA_VERSION,
        id: "bank_adventure".to_string(),
        name: "Adventure".to_string(),
        events: vec![
            SfxEvent {
                id: "player.footstep".to_string(),
                name: "Footstep".to_string(),
                clip_ids: vec!["clip_step_a".to_string(), "clip_step_b".to_string()],
                volume: 0.7,
                volume_random: 0.05,
                pitch_random: 1.0,
                cooldown_ms: 90,
                max_polyphony: 4,
                rtpc: vec![RtpcBinding {
                    param: "threat".to_string(),
                    target_node: "bus_sfx".to_string(),
                    target_param: "volume".to_string(),
                    min: 0.5,
                    max: 1.0,
                }],
            },
            SfxEvent {
                id: "sword.swing".to_string(),
                name: "Sword swing".to_string(),
                clip_ids: vec!["clip_swing_a".to_string(), "clip_swing_b".to_string()],
                volume: 0.8,
                volume_random: 0.05,
                pitch_random: 2.0,
                cooldown_ms: 120,
                max_polyphony: 3,
                rtpc: vec![],
            },
            SfxEvent {
                id: "ui.click".to_string(),
                name: "UI click".to_string(),
                clip_ids: vec!["clip_click".to_string()],
                volume: 0.6,
                volume_random: 0.0,
                pitch_random: 0.0,
                cooldown_ms: 30,
                max_polyphony: 2,
                rtpc: vec![],
            },
            SfxEvent {
                id: "horse.gallop".to_string(),
                name: "Gallop".to_string(),
                clip_ids: vec!["clip_gallop".to_string()],
                volume: 0.75,
                volume_random: 0.05,
                pitch_random: 1.0,
                cooldown_ms: 200,
                max_polyphony: 2,
                rtpc: vec![RtpcBinding {
                    param: "mounted".to_string(),
                    target_node: "bus_sfx".to_string(),
                    target_param: "volume".to_string(),
                    min: 0.3,
                    max: 1.0,
                }],
            },
        ],
    };

    let params = vec![
        param("threat", "Threat", 0.0, 1.0, 0.0, ""),
        param("time_of_day", "Time of day", 0.0, 24.0, 12.0, "h"),
        param("health_low", "Low health", 0.0, 1.0, 0.0, ""),
        param("mounted", "Mounted", 0.0, 1.0, 0.0, ""),
    ];

    let request = ExportRequest::new(
        "overworld-v1",
        vec!["cue_overworld".to_string()],
        vec!["bank_adventure".to_string()],
    );
    let options = ExportOptions::default();
    let cues = vec![cue.clone()];
    let banks = vec![bank.clone()];
    let built = build_package(&project, &cues, &banks, &params, &request, options)
        .unwrap_or_else(|errors| panic!("fixture rejected:\n- {}", errors.join("\n- ")));

    let out = std::env::args()
        .nth(1)
        .expect("usage: s1_fixture_gen <out-dir>");
    let dir = Path::new(&out);
    write_package(dir, &built).expect("write fixture package");
    let context = serde_json::json!({
        "project": project,
        "cues": cues,
        "banks": banks,
        "params": params,
    });
    std::fs::write(
        dir.join("context.json"),
        serde_json::to_string_pretty(&context).unwrap(),
    )
    .expect("write context bundle");
    println!(
        "wrote {} stems + {} bank(s) to {}",
        built.stems.len(),
        built.banks.len(),
        dir.display()
    );
}
