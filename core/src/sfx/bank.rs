//! v1 SFX bank build + validation (GA-2, agent 2).
//!
//! The trigger/voice path lives in `super::runtime` (`SfxRuntime`) and the
//! offline preview in `super::audition` (`render_voice`); both read the
//! frozen bank shapes as data. This module owns the *bank* side: building a
//! [`SfxBank`] from a v0 [`Project`]'s clip inventory, statically validating
//! it, and round-tripping it through JSON (the verbatim serialization the
//! export package ships in `contracts/export-package.md`).
//!
//! Rule mapping (validator rule 2 in `contracts/export-package.md`):
//! empty pools and duplicate event ids fail here without a project;
//! dangling clip ids fail in [`resolve_against_project`], which needs the
//! v0 clip list. Unknown game params in `rtpc` are *not* a build error —
//! they are ignored per trigger (quiet tolerance, same as snapshots).

use std::collections::HashSet;

use super::super::game_audio::{SfxBank, SfxEvent, GAME_AUDIO_SCHEMA_VERSION};
use super::super::model::Project;

/// One authoring-time event definition: the bank builder resolves its
/// `clip_ids` against the project's clip inventory.
#[derive(Debug, Clone, PartialEq)]
pub struct BankEventDraft {
    pub id: String,
    pub name: String,
    pub clip_ids: Vec<String>,
    pub volume: f64,
    pub volume_random: f64,
    pub pitch_random: f64,
    pub cooldown_ms: u64,
    pub max_polyphony: u32,
    pub rtpc: Vec<super::super::game_audio::RtpcBinding>,
}

impl BankEventDraft {
    pub fn new(id: &str, name: &str, clip_ids: Vec<String>) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            clip_ids,
            volume: 0.8,
            volume_random: 0.0,
            pitch_random: 0.0,
            cooldown_ms: 0,
            max_polyphony: 4,
            rtpc: Vec::new(),
        }
    }

    fn into_event(self) -> SfxEvent {
        SfxEvent {
            id: self.id,
            name: self.name,
            clip_ids: self.clip_ids,
            volume: self.volume,
            volume_random: self.volume_random,
            pitch_random: self.pitch_random,
            cooldown_ms: self.cooldown_ms,
            max_polyphony: self.max_polyphony,
            rtpc: self.rtpc,
        }
    }
}

/// Build a bank from the project's clip inventory.
///
/// Every draft pool must be nonempty, every event id unique within the
/// bank, and every clip id must resolve to a real v0 clip — otherwise the
/// returned `Err` lists one message per violation (validator rule 2) and
/// no bank is produced. Returns the bank on success; use
/// [`validate_bank`] to re-check a bank received from elsewhere.
pub fn build_bank(
    bank_id: &str,
    bank_name: &str,
    project: &Project,
    drafts: Vec<BankEventDraft>,
) -> Result<SfxBank, Vec<String>> {
    let bank = SfxBank {
        schema_version: GAME_AUDIO_SCHEMA_VERSION,
        id: bank_id.to_string(),
        name: bank_name.to_string(),
        events: drafts.into_iter().map(BankEventDraft::into_event).collect(),
    };
    let mut errors = validate_bank(&bank);
    errors.extend(resolve_against_project(&bank, project));
    if errors.is_empty() {
        Ok(bank)
    } else {
        Err(errors)
    }
}

/// Static bank validation that needs no project: empty pools and duplicate
/// event ids (validator rule 2, project-independent half).
pub fn validate_bank(bank: &SfxBank) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    for event in &bank.events {
        if !seen.insert(event.id.as_str()) {
            errors.push(format!("duplicate event id '{}'", event.id));
        }
        if event.clip_ids.is_empty() {
            errors.push(format!("event '{}' has an empty clip pool", event.id));
        }
    }
    errors
}

/// Project-dependent validation: every pooled clip id must resolve to a
/// real v0 clip. Returns one message per dangling id.
pub fn resolve_against_project(bank: &SfxBank, project: &Project) -> Vec<String> {
    let known: HashSet<&str> = project.clips.iter().map(|c| c.id.as_str()).collect();
    let mut errors = Vec::new();
    for event in &bank.events {
        for clip in &event.clip_ids {
            if !known.contains(clip.as_str()) {
                errors.push(format!(
                    "event '{}' references unknown clip '{}'",
                    event.id, clip
                ));
            }
        }
    }
    errors
}

/// Serialize a bank exactly as the export package ships it
/// (`bank_<id>.json`, verbatim JSON).
pub fn bank_to_json(bank: &SfxBank) -> String {
    serde_json::to_string_pretty(bank).expect("SfxBank is always serializable")
}

/// Parse a bank back; mis-shaped JSON is an authoring-time error string,
// never a panic.
pub fn bank_from_json(json: &str) -> Result<SfxBank, String> {
    serde_json::from_str(json).map_err(|e| format!("invalid bank JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::super::super::game_audio::RtpcBinding;
    use super::super::super::model::{Clip, ClipKind};
    use super::*;

    fn project_with_clips(ids: &[&str]) -> Project {
        let mut project = Project::new("proj_demo", "Demo");
        for (i, id) in ids.iter().enumerate() {
            project.clips.push(Clip {
                id: id.to_string(),
                track_id: "trk_1".to_string(),
                name: format!("clip {i}"),
                start_beats: 0.0,
                length_beats: 1.0,
                kind: ClipKind::Audio,
                source: format!("samples/{id}.wav"),
            });
        }
        project
    }

    fn footstep_draft() -> BankEventDraft {
        let mut draft = BankEventDraft::new(
            "player.footstep",
            "Footstep",
            vec!["clip_step_a".to_string(), "clip_step_b".to_string()],
        );
        draft.volume_random = 0.1;
        draft.pitch_random = 2.0;
        draft.cooldown_ms = 90;
        draft.rtpc.push(RtpcBinding {
            param: "speed".to_string(),
            target_node: "bus_sfx".to_string(),
            target_param: "volume".to_string(),
            min: 0.5,
            max: 1.0,
        });
        draft
    }

    #[test]
    fn bank_builds_from_project_clips_and_round_trips_verbatim() {
        let project = project_with_clips(&["clip_step_a", "clip_step_b"]);
        let bank = build_bank("bank_demo", "Demo", &project, vec![footstep_draft()])
            .expect("valid bank builds");
        assert_eq!(bank.schema_version, GAME_AUDIO_SCHEMA_VERSION);
        assert_eq!(bank.events.len(), 1);
        let json = bank_to_json(&bank);
        assert!(json.contains(&GAME_AUDIO_SCHEMA_VERSION.to_string()));
        let back = bank_from_json(&json).expect("round trip");
        assert_eq!(bank, back);
        // Re-serializing the parsed bank is byte-stable (verbatim ship rule).
        assert_eq!(bank_to_json(&back), json);
    }

    #[test]
    fn build_rejects_empty_pool_duplicate_ids_and_dangling_clips() {
        let project = project_with_clips(&["clip_step_a"]);
        let mut dupe = footstep_draft();
        dupe.id = "player.footstep".to_string();
        let err = build_bank(
            "bank_demo",
            "Demo",
            &project,
            vec![
                footstep_draft(),
                dupe,
                BankEventDraft::new("silent", "Silent", vec![]),
                BankEventDraft::new("ghost", "Ghost", vec!["clip_missing".to_string()]),
            ],
        )
        .expect_err("invalid bank must not build");
        assert!(err.iter().any(|e| e.contains("duplicate")), "{err:?}");
        assert!(err.iter().any(|e| e.contains("empty")), "{err:?}");
        assert!(
            err.iter().any(|e| e.contains("clip_missing")),
            "{err:?}"
        );
    }

    #[test]
    fn validate_bank_needs_no_project_and_resolve_needs_clips() {
        let project = Project::new("proj_empty", "Empty");
        let bank = SfxBank {
            schema_version: GAME_AUDIO_SCHEMA_VERSION,
            id: "bank_demo".to_string(),
            name: "Demo".to_string(),
            events: vec![footstep_draft().into_event()],
        };
        assert!(validate_bank(&bank).is_empty());
        let dangling = resolve_against_project(&bank, &project);
        assert_eq!(dangling.len(), 2, "{dangling:?}");
    }

    #[test]
    fn malformed_bank_json_is_an_error_not_a_panic() {
        assert!(bank_from_json("{not json").is_err());
        assert!(bank_from_json(r#"{"schema_version":1}"#).is_err());
    }
}
