//! S-1 fixture builder: authors the demo TP-like GA-4 export package.
//!
//! Teaching note: this binary contains no audio math and no contract shapes.
//! It only assembles authoring-side data (a v0 `Project`, v1 `AdaptiveCue`s,
//! one v1 `SfxBank`, v1 `GameStateParam`s) and hands them to the frozen GA-4
//! path (`gaexport::build_package` + `write_package`). Rendering, the
//! manifest, and all five validator rules stay owned by `core/src/gaexport.rs`.
//! Same inputs + same seed = byte-identical package (validator rule 5), so
//! the template track can treat `godot-template/demo-package/` as fixed data.
//!
//! ```sh
//! cargo run --manifest-path ga-fixture/Cargo.toml -- \
//!   --out godot-template/demo-package --context ga-fixture/context.json
//! scripts/validate-export.sh godot-template/demo-package ga-fixture/context.json
//! ```

use ccez_core::gaexport::{self, ExportOptions, ExportRequest};
use ccez_core::game_audio::{
    AdaptiveCue, CueLayer, GameStateParam, RtpcBinding, SfxBank, SfxEvent, TransitionRule,
    GAME_AUDIO_SCHEMA_VERSION,
};
use ccez_core::model::{Clip, ClipKind, Node, NodeKind, Param, Project, Track};

/// Game states the demo covers: LoZ:TP-like field/combat/boss plus the
/// village-calm and night beds the template's state buttons exercise.
pub const FIXTURE_STATES: &[&str] = &["field", "combat", "boss", "village", "night"];

fn clip(id: &str, length_beats: f64) -> Clip {
    Clip {
        id: id.to_string(),
        track_id: "trk_music".to_string(),
        name: id.to_string(),
        start_beats: 0.0,
        length_beats,
        kind: ClipKind::Audio,
        source: format!("samples/{id}.wav"),
    }
}

fn bus(id: &str, name: &str) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::Bus,
        name: name.to_string(),
        params: vec![Param {
            id: "volume".to_string(),
            label: "Volume".to_string(),
            value: 0.8,
            min: 0.0,
            max: 1.0,
            default: 0.8,
            unit: String::new(),
        }],
    }
}

fn track(id: &str, name: &str) -> Track {
    Track {
        id: id.to_string(),
        name: name.to_string(),
        volume: 0.8,
        pan: 0.0,
        muted: false,
        solo: false,
        clip_ids: Vec::new(),
        device_ids: Vec::new(),
    }
}

/// The v0 project every fixture id resolves against: clips (loop length =
/// longest clip per layer), two mix tracks, two mix buses. RTPC targets must
/// name a real `target_node:target_param` here or the export fails rule 3.
pub fn fixture_project() -> Project {
    let mut p = Project::new("proj_tp_demo", "TP Demo");
    p.tempo = 120.0;
    p.tracks.push(track("trk_music", "Music"));
    p.tracks.push(track("trk_sfx", "SFX"));
    for (id, beats) in [
        ("clip_field_bed", 8.0),
        ("clip_field_strings", 8.0),
        ("clip_field_drums", 4.0),
        ("clip_field_night", 8.0),
        ("clip_boss_bed", 8.0),
        ("clip_boss_brass", 8.0),
        ("clip_boss_choir", 4.0),
        ("clip_stinger_hit", 2.0),
        ("clip_step_a", 1.0),
        ("clip_step_b", 1.0),
        ("clip_swing_a", 1.0),
        ("clip_swing_b", 1.0),
        ("clip_ui_a", 1.0),
        ("clip_gal_a", 1.0),
        ("clip_gal_b", 1.0),
    ] {
        p.clips.push(clip(id, beats));
    }
    p.devices.push(bus("bus_sfx", "SFX bus"));
    p.devices.push(bus("bus_music", "Music bus"));
    p
}

/// The four game params the template wires to sliders: `threat` drives SFX
/// RTPC, `mounted` drives the gallop/horseback blend, `time_of_day` and
/// `health_low` ship declared so the game can post them before any binding
/// reads them (missing values read as `default`, never an error).
pub fn fixture_params() -> Vec<GameStateParam> {
    vec![
        GameStateParam {
            id: "threat".to_string(),
            label: "Threat".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: String::new(),
        },
        GameStateParam {
            id: "time_of_day".to_string(),
            label: "Time of day".to_string(),
            min: 0.0,
            max: 24.0,
            default: 12.0,
            unit: "h".to_string(),
        },
        GameStateParam {
            id: "health_low".to_string(),
            label: "Health low".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: String::new(),
        },
        GameStateParam {
            id: "mounted".to_string(),
            label: "Mounted".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: String::new(),
        },
    ]
}

fn layer(id: &str, name: &str, clips: &[&str], states: &[&str], volume: f64) -> CueLayer {
    CueLayer {
        id: id.to_string(),
        name: name.to_string(),
        clip_ids: clips.iter().map(|s| s.to_string()).collect(),
        states: states.iter().map(|s| s.to_string()).collect(),
        volume,
    }
}

fn rule(
    id: &str,
    from: &str,
    to: &str,
    kind: ccez_core::game_audio::TransitionKind,
    fade_beats: f64,
    stinger: &str,
) -> TransitionRule {
    TransitionRule {
        id: id.to_string(),
        from_state: from.to_string(),
        to_state: to.to_string(),
        kind,
        fade_beats,
        stinger_cue_id: stinger.to_string(),
    }
}

use ccez_core::game_audio::TransitionKind;

/// Field/combat cue (104 BPM): an always-on bed, a field+village string
/// layer, a combat drum layer, a night pad. The field->boss hit goes through
/// a stinger so the template can demo the `Stinger` kind on a real rule.
pub fn cue_field() -> AdaptiveCue {
    let mut cue = AdaptiveCue::new("cue_field", "Field");
    cue.tempo = 104.0;
    cue.default_state = "field".to_string();
    cue.layers.push(layer("bed", "Field Bed", &["clip_field_bed"], &[], 0.8));
    cue.layers.push(layer(
        "strings",
        "Field Strings",
        &["clip_field_strings"],
        &["field", "village"],
        0.75,
    ));
    cue.layers.push(layer(
        "drums",
        "Combat Drums",
        &["clip_field_drums"],
        &["combat"],
        0.9,
    ));
    cue.layers.push(layer(
        "nightpad",
        "Night Pad",
        &["clip_field_night"],
        &["night"],
        0.7,
    ));
    cue.transitions.push(rule(
        "t_field_combat",
        "field",
        "combat",
        TransitionKind::Fade,
        2.0,
        "",
    ));
    cue.transitions.push(rule(
        "t_combat_field",
        "combat",
        "field",
        TransitionKind::Fade,
        4.0,
        "",
    ));
    cue.transitions.push(rule(
        "t_night_field",
        "night",
        "field",
        TransitionKind::Fade,
        2.0,
        "",
    ));
    cue.transitions.push(rule(
        "t_field_boss",
        "field",
        "boss",
        TransitionKind::Stinger,
        0.0,
        "cue_stinger_boss",
    ));
    cue
}

/// Boss cue (132 BPM): bed plus brass + choir stacks that only sound on
/// `boss`. The template holds these layers while the boss state is active.
pub fn cue_boss() -> AdaptiveCue {
    let mut cue = AdaptiveCue::new("cue_boss", "Boss");
    cue.tempo = 132.0;
    cue.default_state = "boss".to_string();
    cue.layers.push(layer("bed", "Boss Bed", &["clip_boss_bed"], &[], 0.8));
    cue.layers.push(layer(
        "brass",
        "Boss Brass",
        &["clip_boss_brass"],
        &["boss"],
        0.85,
    ));
    cue.layers.push(layer(
        "choir",
        "Boss Choir",
        &["clip_boss_choir"],
        &["boss"],
        0.7,
    ));
    cue.transitions.push(rule(
        "t_boss_field",
        "boss",
        "field",
        TransitionKind::Fade,
        4.0,
        "",
    ));
    cue
}

/// The one-shot hit the field->boss `Stinger` rule fires. It is a real cue
/// (validator rule 3 needs the id to resolve) and ships its own stem so the
/// engine can trigger it by id.
pub fn cue_stinger_boss() -> AdaptiveCue {
    let mut cue = AdaptiveCue::new("cue_stinger_boss", "Boss Stinger");
    cue.tempo = 132.0;
    cue.default_state = "boss".to_string();
    cue.layers.push(layer(
        "hit",
        "Stinger Hit",
        &["clip_stinger_hit"],
        &["boss"],
        0.9,
    ));
    cue
}

pub fn fixture_cues() -> Vec<AdaptiveCue> {
    vec![cue_field(), cue_boss(), cue_stinger_boss()]
}

fn event(
    id: &str,
    name: &str,
    clips: &[&str],
    volume: f64,
    volume_random: f64,
    pitch_random: f64,
    cooldown_ms: u64,
    max_polyphony: u32,
    rtpc: Vec<RtpcBinding>,
) -> SfxEvent {
    SfxEvent {
        id: id.to_string(),
        name: name.to_string(),
        clip_ids: clips.iter().map(|s| s.to_string()).collect(),
        volume,
        volume_random,
        pitch_random,
        cooldown_ms,
        max_polyphony,
        rtpc,
    }
}

fn rtpc(param: &str, node: &str, target: &str, min: f64, max: f64) -> RtpcBinding {
    RtpcBinding {
        param: param.to_string(),
        target_node: node.to_string(),
        target_param: target.to_string(),
        min,
        max,
    }
}

/// The demo SFX bank: footsteps bent by `threat`, sword swings beside them,
/// a UI confirm, and a gallop bent by `mounted` for the horseback state.
/// Every RTPC target names a real mix address (see `fixture_project`).
pub fn fixture_bank() -> SfxBank {
    SfxBank {
        schema_version: GAME_AUDIO_SCHEMA_VERSION,
        id: "bank_tp".to_string(),
        name: "TP Demo Bank".to_string(),
        events: vec![
            event(
                "player.footstep",
                "Footstep",
                &["clip_step_a", "clip_step_b"],
                0.7,
                0.05,
                1.0,
                90,
                4,
                vec![rtpc("threat", "bus_sfx", "volume", 0.5, 1.0)],
            ),
            event(
                "sword.swing",
                "Sword Swing",
                &["clip_swing_a", "clip_swing_b"],
                0.85,
                0.08,
                2.0,
                120,
                3,
                vec![rtpc("threat", "bus_sfx", "volume", 0.6, 1.0)],
            ),
            event(
                "ui.confirm",
                "UI Confirm",
                &["clip_ui_a"],
                0.6,
                0.03,
                0.5,
                50,
                4,
                Vec::new(),
            ),
            event(
                "horse.gallop",
                "Horse Gallop",
                &["clip_gal_a", "clip_gal_b"],
                0.75,
                0.06,
                1.5,
                140,
                2,
                vec![rtpc("mounted", "trk_sfx", "volume", 0.4, 1.0)],
            ),
        ],
    }
}

pub fn fixture_request() -> ExportRequest {
    ExportRequest::new(
        "demo-tp-v1",
        vec![
            "cue_field".to_string(),
            "cue_boss".to_string(),
            "cue_stinger_boss".to_string(),
        ],
        vec!["bank_tp".to_string()],
    )
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
}

fn usage() -> ! {
    eprintln!("usage: ccez-fixture --out <package-dir> --context <context.json>");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = flag_value(&args, "--out").unwrap_or_else(|| usage());
    let context_path = flag_value(&args, "--context").unwrap_or_else(|| usage());

    let project = fixture_project();
    let cues = fixture_cues();
    let banks = vec![fixture_bank()];
    let params = fixture_params();
    let request = fixture_request();
    let options = ExportOptions::default();

    match gaexport::build_package(&project, &cues, &banks, &params, &request, options) {
        Ok(built) => {
            let dir = std::path::PathBuf::from(&out);
            gaexport::write_package(&dir, &built).expect("write package");
            let context = serde_json::json!({
                "project": project,
                "cues": cues,
                "banks": banks,
                "params": params,
            });
            std::fs::write(
                &context_path,
                serde_json::to_string_pretty(&context).expect("context serializes"),
            )
            .expect("write context");
            println!(
                "fixture: wrote {} stems + {} bank file(s) to {out} (context: {context_path})",
                built.stems.len(),
                built.banks.len(),
            );
            for stem in &built.package.stems {
                println!(
                    "  {} {:?} loop {}..{}",
                    stem.path, stem.kind, stem.loop_start_beats, stem.loop_end_beats
                );
            }
        }
        Err(errors) => {
            eprintln!("fixture rejected ({} error{}):", errors.len(), if errors.len() == 1 { "" } else { "s" });
            for e in &errors {
                eprintln!("- {e}");
            }
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_builds_and_validates_clean() {
        let project = fixture_project();
        let cues = fixture_cues();
        let banks = vec![fixture_bank()];
        let options = ExportOptions {
            sample_rate: 8000,
            seed: gaexport::DEFAULT_EXPORT_SEED,
        };
        let built = gaexport::build_package(
            &project,
            &cues,
            &banks,
            &fixture_params(),
            &fixture_request(),
            options,
        )
        .expect("fixture builds");
        // field/combat/boss coverage: bed + state layers on every cue.
        assert_eq!(built.package.cue_ids.len(), 3);
        assert_eq!(built.package.bank_ids, vec!["bank_tp".to_string()]);
        // 4 field layers + 3 boss layers + 1 stinger layer + 7 pooled SFX clips.
        assert_eq!(built.package.stems.len(), 8 + 7);
        let dir = std::env::temp_dir().join(format!(
            "ccez-fixture-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        gaexport::write_package(&dir, &built).expect("write");
        let report =
            gaexport::validate_package(&dir, &project, &cues, &banks, &fixture_params(), options);
        assert!(report.is_ok(), "fixture validates: {:?}", report.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fixture_covers_the_tp_states_and_params() {
        let cues = fixture_cues();
        let field = cues.iter().find(|c| c.id == "cue_field").expect("field cue");
        assert_eq!(field.layers_for_state("field").len(), 2); // bed + strings
        assert_eq!(field.layers_for_state("village").len(), 2); // bed + strings
        assert_eq!(field.layers_for_state("combat").len(), 2); // bed + drums
        assert_eq!(field.layers_for_state("night").len(), 2); // bed + nightpad
        let stinger = field.transition_for("field", "boss").expect("stinger rule");
        assert_eq!(stinger.kind, TransitionKind::Stinger);
        assert_eq!(stinger.stinger_cue_id, "cue_stinger_boss");
        let params = fixture_params();
        let ids: Vec<&str> = params.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["threat", "time_of_day", "health_low", "mounted"]);
    }
}
