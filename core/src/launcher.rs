//! Clip-slot / scene launch model: slot grid, launch quantization, jam-record.
//!
//! Teaching note: the launcher grid ([`crate::timeline::launcher_slots`]) is
//! a *view* — it borrows clip ids, it owns nothing. This module adds the
//! *performance* side: pressing a slot/scene does not play instantly, it
//! starts at the next quantization boundary, and a jam (a sequence of scene
//! launches) can be written back to the linear timeline as ordinary clips.
//!
//! Frozen-schema stance (same as `timeline.rs` / `record.rs`): no new
//! `OpKind`, no project-schema change, no IPC surface. Slot launch plans
//! are pure computations; jam-record emits frozen `ClipAdded` ops
//! (`seq: 0` placeholders the engine replaces), so the whole workflow is
//! undoable for free and the typegen drift gate is unaffected.

use serde::{Deserialize, Serialize};

use crate::model::{Clip, Op, OpKind};
use crate::timeline::{TimelineDoc, clip_end, launcher_slots, section_end};

/// Launch quantization grid: the launch starts at the next grid boundary
/// at or after the press beat. `Bar` is 4 beats in 4/4; callers in another
/// meter pass an explicit [`LaunchQuant::Beats`] grid.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LaunchQuant {
    /// Start immediately (no quantization).
    None,
    /// Start at the next whole beat.
    Beat,
    /// Start at the next bar line (4 beats).
    Bar,
    /// Start at the next section (scene) boundary: the pressed section's
    /// own start, or one full section length later when already past it.
    Scene,
    /// Start at the next multiple of `0` (must be finite and > 0).
    Beats(f64),
}

impl LaunchQuant {
    /// Grid size in beats. `Scene` has no fixed grid — see
    /// [`quantize_launch_for_scene`] — so this returns `None` for it.
    pub fn grid_beats(&self) -> Option<f64> {
        match self {
            LaunchQuant::None => Some(0.0),
            LaunchQuant::Beat => Some(1.0),
            LaunchQuant::Bar => Some(4.0),
            LaunchQuant::Scene => None,
            LaunchQuant::Beats(g) => Some(*g),
        }
    }

    /// Validate the grid: `Scene`/`None`/`Beat`/`Bar` always hold;
    /// `Beats(g)` needs finite `g > 0`.
    pub fn validate(&self) -> Result<()> {
        if let LaunchQuant::Beats(g) = self {
            if !g.is_finite() || *g <= 0.0 {
                return Err(LaunchError::BadQuant(format!(
                    "quant grid {g} must be finite and > 0"
                )));
            }
        }
        Ok(())
    }
}

/// One pressed pad: a `(section, track)` cell in the slot grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotRef {
    pub section_id: String,
    pub track_id: String,
}

/// One scene/slot launch plan: what the press triggers and when it starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchPlan {
    /// Pressed section (scene) id.
    pub section_id: String,
    /// Pressed track, or `None` for a whole-scene (row) launch.
    pub track_id: Option<String>,
    /// Transport beat when the press happened.
    pub requested_beat: f64,
    /// Transport beat when playback starts (quantized).
    pub launch_beat: f64,
    /// Clip ids the launch triggers (slot cell, or whole row for scenes).
    pub clip_ids: Vec<String>,
}

/// One jam event: a scene launch that already happened (quantized).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JamEvent {
    pub section_id: String,
    pub launch_beat: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LaunchError {
    UnknownSection(String),
    UnknownSlot(String),
    BadBeat(String),
    BadQuant(String),
    NoEvents,
    BadJam(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSection(s) => write!(f, "unknown section `{s}`"),
            Self::UnknownSlot(s) => write!(f, "unknown slot `{s}`"),
            Self::BadBeat(m) => write!(f, "bad beat: {m}"),
            Self::BadJam(m) => write!(f, "bad jam: {m}"),
            Self::BadQuant(m) => write!(f, "bad quant: {m}"),
            Self::NoEvents => write!(f, "jam has no events"),
        }
    }
}

impl std::error::Error for LaunchError {}

pub type Result<T> = std::result::Result<T, LaunchError>;

fn check_beat(beat: f64, what: &str) -> Result<()> {
    if !beat.is_finite() || beat < 0.0 {
        return Err(LaunchError::BadBeat(format!(
            "{what} beat {beat} must be finite and >= 0"
        )));
    }
    Ok(())
}

/// Next grid boundary at or after `requested` (`requested` itself when
/// already on the grid, within 1e-9). `grid` must be finite and > 0.
pub fn quantize_beat(requested: f64, grid: f64) -> Result<f64> {
    check_beat(requested, "requested")?;
    if !grid.is_finite() || grid <= 0.0 {
        return Err(LaunchError::BadQuant(format!(
            "quant grid {grid} must be finite and > 0"
        )));
    }
    let n = (requested / grid).ceil() - requested / grid;
    let snapped = if n < 1e-9 { requested } else { (requested / grid).ceil() * grid };
    // Avoid 7.9999996-style float dust on exact multiples.
    let rounded = (snapped / grid).round() * grid;
    if (rounded - snapped).abs() < 1e-9 {
        Ok(rounded)
    } else {
        Ok(snapped)
    }
}

/// Scene quantization: the pressed section's own start when the press is
/// before it, else one full section length after the press position's
/// section-phase. Keeps a jam in section-sized phrases.
pub fn quantize_launch_for_scene(requested: f64, section_start: f64, section_len: f64) -> Result<f64> {
    check_beat(requested, "requested")?;
    if !section_start.is_finite() || section_start < 0.0 {
        return Err(LaunchError::BadBeat(format!(
            "section start {section_start} must be finite and >= 0"
        )));
    }
    if !section_len.is_finite() || section_len <= 0.0 {
        return Err(LaunchError::BadQuant(format!(
            "section length {section_len} must be finite and > 0"
        )));
    }
    if requested <= section_start {
        return Ok(section_start);
    }
    let phase = (requested - section_start) / section_len;
    Ok(section_start + phase.ceil() * section_len)
}

/// Resolve the quantized launch beat for a press under `quant`.
pub fn launch_beat_for(
    requested: f64,
    quant: LaunchQuant,
    doc: &TimelineDoc,
    section_id: &str,
) -> Result<f64> {
    check_beat(requested, "requested")?;
    quant.validate()?;
    match quant {
        LaunchQuant::None => Ok(requested),
        LaunchQuant::Beat | LaunchQuant::Bar | LaunchQuant::Beats(_) => {
            let grid = match quant {
                LaunchQuant::Beats(g) => g,
                _ => quant.grid_beats().expect("fixed grid"),
            };
            quantize_beat(requested, grid)
        }
        LaunchQuant::Scene => {
            let section = doc
                .section(section_id)
                .ok_or_else(|| LaunchError::UnknownSection(section_id.to_string()))?;
            quantize_launch_for_scene(requested, section.start_beats, section.length_beats)
        }
    }
}

/// Plan launching one slot cell: the clips the `(section, track)` cell
/// holds, starting at the quantized beat. Unknown sections/slots are
/// errors (a dangling pad must fail loudly, never trigger silence).
pub fn plan_slot_launch(
    project: &crate::model::Project,
    doc: &TimelineDoc,
    slot: &SlotRef,
    requested_beat: f64,
    quant: LaunchQuant,
) -> Result<LaunchPlan> {
    let section = doc
        .section(&slot.section_id)
        .ok_or_else(|| LaunchError::UnknownSection(slot.section_id.clone()))?;
    if !project.tracks.iter().any(|t| t.id == slot.track_id) {
        return Err(LaunchError::UnknownSlot(format!(
            "{}:{}",
            slot.section_id, slot.track_id
        )));
    }
    let _ = section;
    let launch_beat = launch_beat_for(requested_beat, quant, doc, &slot.section_id)?;
    let end = section_end(doc.section(&slot.section_id).expect("checked"));
    let mut clips: Vec<&Clip> = project
        .clips
        .iter()
        .filter(|c| {
            c.track_id == slot.track_id && c.start_beats < end && clip_end(c) > section.start_beats
        })
        .collect();
    clips.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(LaunchPlan {
        section_id: slot.section_id.clone(),
        track_id: Some(slot.track_id.clone()),
        requested_beat,
        launch_beat,
        clip_ids: clips.into_iter().map(|c| c.id.clone()).collect(),
    })
}

/// Plan launching a whole scene (one section row across all tracks).
pub fn plan_scene_launch(
    project: &crate::model::Project,
    doc: &TimelineDoc,
    section_id: &str,
    requested_beat: f64,
    quant: LaunchQuant,
) -> Result<LaunchPlan> {
    if doc.section(section_id).is_none() {
        return Err(LaunchError::UnknownSection(section_id.to_string()));
    }
    let launch_beat = launch_beat_for(requested_beat, quant, doc, section_id)?;
    let slots = launcher_slots(project, doc);
    let mut clip_ids: Vec<String> = slots
        .iter()
        .filter(|s| s.section_id == section_id)
        .flat_map(|s| s.clip_ids.iter().cloned())
        .collect();
    clip_ids.sort();
    clip_ids.dedup();
    Ok(LaunchPlan {
        section_id: section_id.to_string(),
        track_id: None,
        requested_beat,
        launch_beat,
        clip_ids,
    })
}

/// Validate a jam: non-empty, known sections, finite non-negative launch
/// beats in non-decreasing order.
pub fn validate_jam(doc: &TimelineDoc, events: &[JamEvent]) -> Result<()> {
    if events.is_empty() {
        return Err(LaunchError::NoEvents);
    }
    let mut prev = -f64::INFINITY;
    for (i, ev) in events.iter().enumerate() {
        let section = doc
            .section(&ev.section_id)
            .ok_or_else(|| LaunchError::UnknownSection(ev.section_id.clone()))?;
        let _ = section;
        if !ev.launch_beat.is_finite() || ev.launch_beat < 0.0 {
            return Err(LaunchError::BadJam(format!(
                "event {i} launch beat {} must be finite and >= 0",
                ev.launch_beat
            )));
        }
        if ev.launch_beat < prev {
            return Err(LaunchError::BadJam(format!(
                "event {i} launch beat {} is before event {} ({prev})",
                ev.launch_beat,
                i.saturating_sub(1)
            )));
        }
        prev = ev.launch_beat;
    }
    Ok(())
}

/// Record a jam into the arrangement: for each jam event, copy every clip
/// overlapping the launched section to `launch_beat + (clip.start -
/// section.start)` as a frozen `ClipAdded` op. New ids are
/// `{clip_id}__jam{i}`; collisions with existing clips are errors (the
/// caller re-jams with fresh ids rather than silently overwriting takes).
///
/// `actor` is the op actor (`ui`, `mcp`, ...); `seq: 0` placeholders are
/// replaced by the engine.
pub fn jam_record_ops(
    actor: &str,
    project: &crate::model::Project,
    doc: &TimelineDoc,
    events: &[JamEvent],
) -> Result<Vec<Op>> {
    validate_jam(doc, events)?;
    let mut ops = Vec::new();
    for (i, ev) in events.iter().enumerate() {
        let section = doc.section(&ev.section_id).expect("validated");
        let end = section_end(section);
        let mut member: Vec<&Clip> = project
            .clips
            .iter()
            .filter(|c| c.start_beats < end && clip_end(c) > section.start_beats)
            .collect();
        member.sort_by(|a, b| {
            a.start_beats
                .partial_cmp(&b.start_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        for clip in member {
            let offset = clip.start_beats - section.start_beats;
            let new_id = format!("{}__jam{i}", clip.id);
            if project.clips.iter().any(|c| c.id == new_id)
                || ops.iter().any(|o: &Op| {
                    o.kind == OpKind::ClipAdded
                        && serde_json::from_str::<Clip>(&o.value_json)
                            .map(|c| c.id == new_id)
                            .unwrap_or(false)
                })
            {
                return Err(LaunchError::BadJam(format!("clip id `{new_id}` already exists")));
            }
            let recorded = Clip {
                id: new_id,
                track_id: clip.track_id.clone(),
                name: format!("{} (jam {i})", clip.name),
                start_beats: ev.launch_beat + offset,
                length_beats: clip.length_beats,
                kind: clip.kind.clone(),
                source: clip.source.clone(),
            };
            ops.push(Op {
                seq: 0,
                actor: actor.to_string(),
                kind: OpKind::ClipAdded,
                target: recorded.track_id.clone(),
                value_json: serde_json::to_string(&recorded).expect("clip serializes"),
            });
        }
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::timeline::{sample_timeline_doc, sample_timeline_project};

    #[test]
    fn quant_grid_snaps_up_to_next_boundary() {
        assert_eq!(quantize_beat(0.0, 1.0).unwrap(), 0.0);
        assert_eq!(quantize_beat(3.0, 4.0).unwrap(), 4.0);
        assert_eq!(quantize_beat(4.0, 4.0).unwrap(), 4.0);
        assert_eq!(quantize_beat(4.5, 1.0).unwrap(), 5.0);
        assert_eq!(quantize_beat(2.0, 0.5).unwrap(), 2.0);
        assert!(quantize_beat(1.0, 0.0).is_err());
        assert!(quantize_beat(-1.0, 1.0).is_err());
        assert!(quantize_beat(f64::NAN, 1.0).is_err());
    }

    #[test]
    fn scene_quant_holds_section_phrases() {
        // Chorus is 16..32: a press before it waits for 16, a press inside
        // re-triggers at the next 16-beat phrase.
        assert_eq!(quantize_launch_for_scene(0.0, 16.0, 16.0).unwrap(), 16.0);
        assert_eq!(quantize_launch_for_scene(17.0, 16.0, 16.0).unwrap(), 32.0);
        assert_eq!(quantize_launch_for_scene(16.0, 16.0, 16.0).unwrap(), 16.0);
    }

    #[test]
    fn slot_and_scene_plans_carry_quantized_beats() {
        let project = sample_timeline_project();
        let doc = sample_timeline_doc();
        let slot = SlotRef {
            section_id: "sec_chorus".to_string(),
            track_id: "trk_drums".to_string(),
        };
        let plan = plan_slot_launch(&project, &doc, &slot, 17.2, LaunchQuant::Beat).unwrap();
        assert_eq!(plan.launch_beat, 18.0);
        assert_eq!(plan.clip_ids, vec!["clip_chorus_1".to_string()]);
        let scene = plan_scene_launch(&project, &doc, "sec_chorus", 3.0, LaunchQuant::Bar).unwrap();
        assert_eq!(scene.launch_beat, 4.0);
        assert_eq!(scene.track_id, None);
        assert_eq!(
            scene.clip_ids,
            vec!["clip_chorus_1".to_string(), "clip_chorus_2".to_string()]
        );
        // Scene quant waits for the section start when pressed early.
        let early =
            plan_scene_launch(&project, &doc, "sec_chorus", 2.0, LaunchQuant::Scene).unwrap();
        assert_eq!(early.launch_beat, 16.0);
        // Dangling pads fail loudly.
        assert!(plan_scene_launch(&project, &doc, "sec_nope", 0.0, LaunchQuant::Beat).is_err());
        assert!(
            plan_slot_launch(
                &project,
                &doc,
                &SlotRef {
                    section_id: "sec_chorus".to_string(),
                    track_id: "trk_nope".to_string(),
                },
                0.0,
                LaunchQuant::Beat,
            )
            .is_err()
        );
    }

    #[test]
    fn jam_record_writes_frozen_clip_added_ops() {
        let dir = std::env::temp_dir().join(format!("ccez-launch-{}-jam", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = sample_timeline_project();
        let doc = sample_timeline_doc();
        // Jam verse at 0 then chorus at 16: copies land phrase-aligned.
        let events = vec![
            JamEvent {
                section_id: "sec_verse".to_string(),
                launch_beat: 0.0,
            },
            JamEvent {
                section_id: "sec_chorus".to_string(),
                launch_beat: 16.0,
            },
        ];
        let ops = jam_record_ops("ui", &project, &doc, &events).unwrap();
        // 2 verse clips + 2 chorus clips, all frozen ClipAdded.
        assert_eq!(ops.len(), 4);
        for op in &ops {
            assert_eq!(op.kind, OpKind::ClipAdded);
            assert_eq!(op.seq, 0);
        }
        // Apply through the engine: undoable, arrangement holds the jam.
        let mut engine = Engine::create(&dir, project).unwrap();
        for op in &ops {
            engine
                .apply(&op.actor, op.kind.clone(), &op.target, &op.value_json)
                .unwrap();
        }
        let jammed: Vec<f64> = engine
            .project()
            .clips
            .iter()
            .filter(|c| c.id.ends_with("__jam1"))
            .map(|c| c.start_beats)
            .collect();
        assert_eq!(jammed, vec![16.0, 24.0]);
        engine.undo().unwrap();
        assert_eq!(engine.project().clips.len(), 4 + 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jam_rejects_empty_unknown_and_unordered_events() {
        let doc = sample_timeline_doc();
        let project = sample_timeline_project();
        assert_eq!(
            jam_record_ops("ui", &project, &doc, &[]),
            Err(LaunchError::NoEvents)
        );
        assert!(matches!(
            jam_record_ops(
                "ui",
                &project,
                &doc,
                &[JamEvent {
                    section_id: "sec_nope".to_string(),
                    launch_beat: 0.0
                }]
            ),
            Err(LaunchError::UnknownSection(_))
        ));
        // Time-travelling second event is rejected.
        assert!(matches!(
            jam_record_ops(
                "ui",
                &project,
                &doc,
                &[
                    JamEvent {
                        section_id: "sec_chorus".to_string(),
                        launch_beat: 16.0
                    },
                    JamEvent {
                        section_id: "sec_verse".to_string(),
                        launch_beat: 4.0
                    },
                ]
            ),
            Err(LaunchError::BadJam(_))
        ));
    }
}
