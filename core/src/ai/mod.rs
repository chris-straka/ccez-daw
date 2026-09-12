//! Track M: AI sidecars — transcription (drums / melody / chords) plus
//! separation + cleanup + groove-transfer (`separation`).
//!
//! The DAW runs fully without these: they are pure functions plus an
//! optional background [`job::Job`], with no new dependencies, no new
//! [`OpKind`](crate::model::OpKind), and no changes to `model.rs`,
//! `ipc.rs`, or `emit.rs` (the typegen drift gate stays green).
//!
//! Sidecar pattern (shared with the NL layer in `mcp/src/nl/sidecar.ts`):
//!
//! - Each transcription kind has a stable sidecar id
//!   ([`SIDECAR_DRUMS`] / [`SIDECAR_MELODY`] / [`SIDECAR_CHORDS`]). Output
//!   ops are applied with actor `ai:<id>` (see [`ai_actor`]), so every AI
//!   clip is ordinary undoable history — never flattened audio.
//! - Models are unpinned: the deterministic baselines here
//!   (`drums` / `melody` / `chords`) are the offline floor. A future HTTP
//!   sidecar may serve any model; swapping it changes plans, never op
//!   shapes or UI. The model id is a plain string, nothing pins a version.
//! - Heavy bytes stay lazy: a transcription produces a [`MidiClip`]; the
//!   caller persists it with [`MidiClip::save_to_engine`] (a `midi`-kind
//!   asset, bundle-portable) and commits one frozen `ClipAdded` op whose
//!   `source` names the asset key. Editing the notes before or after the
//!   commit is ordinary MIDI editing.
//!
//! [`MidiClip`]: crate::midi::MidiClip

pub mod chords;
pub mod drums;
pub mod job;
pub mod melody;
pub mod models;
pub mod separation;

pub use chords::{ChordLabel, ChordTranscription, submit_chords, transcribe_chords};
pub use drums::{DrumOnset, DrumVoice, detect_drums, submit_drums};
pub use job::{Job, JobControl, JobStatus};
pub use melody::{PitchFrame, submit_melody, transcribe_melody};
pub use models::{
    DEFAULT_SIDECAR_ENDPOINT, PINS, ModelPin, ModelResolution, ModelVerdict, cached_weight_path,
    model_cache_dir, pin_for, resolve,
};
pub use separation::{
    Cleanup, Separation, SeparationDest, SIDECAR_CLEANUP, SIDECAR_GROOVE, SIDECAR_SEPARATION,
    STEM_NAMES, apply_cleanup_to_engine, apply_groove_to_engine, apply_separation_to_engine,
    cleanup_audio, cleanup_asset_key, groove_asset_key, separate_mix, separation_asset_key,
    separation_drafts, submit_cleanup, submit_groove_transfer, submit_separation, transfer_groove,
};

use crate::model::{Clip, ClipKind, OpKind};

/// Stable sidecar ids. These become op actors via [`ai_actor`].
pub const SIDECAR_DRUMS: &str = "transcribe-drums";
pub const SIDECAR_MELODY: &str = "transcribe-melody";
pub const SIDECAR_CHORDS: &str = "transcribe-chords";

/// Transcription kind: which baseline (or neural sidecar) produced a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionKind {
    Drums,
    Melody,
    Chords,
}

impl TranscriptionKind {
    pub fn sidecar_id(self) -> &'static str {
        match self {
            Self::Drums => SIDECAR_DRUMS,
            Self::Melody => SIDECAR_MELODY,
            Self::Chords => SIDECAR_CHORDS,
        }
    }
}

/// Actor for AI output: `ai:<sidecar>` (mirrors `engine::valid_actor`).
/// Returns an error on an empty sidecar name instead of minting `ai:`.
pub fn ai_actor(sidecar: &str) -> Result<String, String> {
    if sidecar.is_empty() {
        return Err("AI sidecar name must be non-empty".to_string());
    }
    Ok(format!("ai:{sidecar}"))
}

/// One op-log entry minus `seq` (assigned by `Engine::apply`, never here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpDraft {
    pub kind: OpKind,
    pub target: String,
    pub value_json: String,
}

/// Result of one transcription: human summary + ordinary ops + warnings.
/// The caller applies `ops` with [`ai_actor`] through `Engine::apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionPlan {
    pub summary: String,
    pub ops: Vec<OpDraft>,
    pub warnings: Vec<String>,
    pub kind: TranscriptionKind,
}

/// Build the single `ClipAdded` draft committing `clip` (a frozen v0
/// [`Clip`] whose `source` names the stored MIDI asset key).
pub fn draft_clip_added(clip: &Clip) -> OpDraft {
    OpDraft {
        kind: OpKind::ClipAdded,
        target: clip.id.clone(),
        value_json: serde_json::to_string(clip).expect("Clip serializes"),
    }
}

/// Convenience: draft a MIDI clip shell for a transcription result.
pub fn draft_midi_clip(
    clip_id: &str,
    track_id: &str,
    name: &str,
    start_beats: f64,
    length_beats: f64,
    asset_key: &str,
) -> OpDraft {
    draft_clip_added(&Clip {
        id: clip_id.to_string(),
        track_id: track_id.to_string(),
        name: name.to_string(),
        start_beats,
        length_beats,
        kind: ClipKind::Midi,
        source: asset_key.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, valid_actor};

    #[test]
    fn sidecar_actors_satisfy_engine_actor_rule() {
        for kind in [
            TranscriptionKind::Drums,
            TranscriptionKind::Melody,
            TranscriptionKind::Chords,
        ] {
            let actor = ai_actor(kind.sidecar_id()).expect("actor");
            assert!(valid_actor(&actor), "engine rejects {actor}");
        }
        assert!(ai_actor("").is_err());
    }

    #[test]
    fn plan_draft_round_trips_through_engine_with_undo() {
        let dir = std::env::temp_dir().join(format!(
            "ccez-ai-plan-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut project = crate::model::Project::new("proj_ai", "ai-test");
        project.tracks.push(crate::model::Track {
            id: "trk_ai".to_string(),
            name: "AI".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        let mut engine = Engine::create(&dir, project).expect("create");
        let draft = draft_midi_clip("clip_ai_1", "trk_ai", "AI take", 0.0, 4.0, "take:ai-1");
        let plan = TranscriptionPlan {
            summary: "Transcribe test".to_string(),
            ops: vec![draft],
            warnings: vec![],
            kind: TranscriptionKind::Melody,
        };
        let actor = ai_actor(TranscriptionKind::Melody.sidecar_id()).unwrap();
        for op in &plan.ops {
            engine.apply(&actor, op.kind.clone(), &op.target, &op.value_json).expect("apply");
        }
        assert_eq!(engine.project().clips.len(), 1);
        engine.undo().expect("undo");
        assert!(engine.project().clips.is_empty());
        engine.redo().expect("redo");
        assert_eq!(engine.project().clips.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
