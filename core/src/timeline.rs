//! Track E (agent 1): timeline + clips model.
//!
//! Teaching note: a DAW timeline is one model with two views. The **linear**
//! view lays clips on a beat ruler (the classic left-to-right song); the
//! **launcher** view groups the same clips into a grid of slots/scenes (one
//! column per track, one row per section — press a row to play a moment).
//! Both views read the same frozen [`Project`](crate::model::Project); this
//! module only adds *derived* shapes on top, never new persisted project
//! fields.
//!
//! Three ideas, each mapping to one section below:
//!
//! 1. **Sections are objects.** A [`Section`] is a named beat range
//!    (`Verse 0..16`, `Chorus 16..32`). Clips belong to a section by
//!    *overlap*, not by pointer — so moving a section is just moving its
//!    member clips with ordinary frozen `ClipMoved` ops.
//! 2. **Arrangements are orders.** An [`Arrangement`] lists section ids in
//!    play order. Two arrangements over the same sections are two song
//!    structures with zero clip duplication ([`arrangement_layout`]).
//! 3. **Clips carry object-level sound.** [`ClipProps`] is a sidecar map
//!    keyed by clip id: per-clip gain, pitch (semitones), time ratio,
//!    fades, FX device chain, and routing target. It validates ranges and
//!    supplies defaults, and never touches the frozen schema — the mix
//!    engine reads `Project` first and consults these props as overrides.
//!
//! Storage: [`TimelineDoc`] (sections + arrangements + clip props)
//! serializes to JSON beside the project (e.g. `timeline.json`). All clip
//! motion still flows through the frozen op log ([`move_clip_op`],
//! [`move_section_ops`]), so timeline edits stay undoable like every other
//! edit. This module adds no IPC or project-schema surface (like
//! `branch.rs`), so the typegen drift gate is unaffected.

use serde::{Deserialize, Serialize};

use crate::model::{Clip, Op, OpKind, Project};

/// A named beat range on the timeline: `start_beats .. start_beats+length`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub start_beats: f64,
    pub length_beats: f64,
}

/// One play order over shared sections: alternate song structures without
/// duplicating clips. `section_ids` names sections in play order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrangement {
    pub id: String,
    pub name: String,
    pub section_ids: Vec<String>,
}

/// Object-level sound for one clip (sidecar to the frozen [`Clip`]).
///
/// - `gain_db`: +/- dB trim applied at the clip, `[-60, +12]`.
/// - `pitch_semitones`: transpose, `[-24, +24]`.
/// - `time_ratio`: playback-rate multiplier (> 0, 1 = as recorded).
/// - `fade_in_beats` / `fade_out_beats`: non-negative, clamped by the
///   renderer to half the clip length.
/// - `fx`: device-node ids in chain order (object-level FX).
/// - `out`: routing target override (bus/edge endpoint id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipProps {
    pub clip_id: String,
    pub gain_db: f64,
    pub pitch_semitones: f64,
    pub time_ratio: f64,
    pub fade_in_beats: f64,
    pub fade_out_beats: f64,
    pub fx: Vec<String>,
    pub out: Option<String>,
}

/// The timeline sidecar: section objects, arrangement orders, and per-clip
/// props. Stored beside the project, never inside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineDoc {
    pub sections: Vec<Section>,
    pub arrangements: Vec<Arrangement>,
    pub clip_props: Vec<ClipProps>,
}

/// One launcher slot: the clips a `(section, track)` cell would trigger.
/// The launcher is a view — it borrows ids, it owns nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherSlot {
    pub section_id: String,
    pub track_id: String,
    pub clip_ids: Vec<String>,
}

/// One laid-out section inside an arrangement: section content placed at an
/// arrangement-relative offset (sections concatenate in listed order).
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangedSection {
    pub section_id: String,
    pub name: String,
    pub offset_beats: f64,
    pub length_beats: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimelineError {
    UnknownSection(String),
    UnknownArrangement(String),
    DuplicateSection(String),
    BadRange(String),
    BadProps(String),
    UnknownClip(String),
}

impl std::fmt::Display for TimelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSection(s) => write!(f, "unknown section `{s}`"),
            Self::UnknownArrangement(s) => write!(f, "unknown arrangement `{s}`"),
            Self::DuplicateSection(s) => write!(f, "duplicate section `{s}`"),
            Self::BadRange(m) => write!(f, "bad section range: {m}"),
            Self::BadProps(m) => write!(f, "bad clip props: {m}"),
            Self::UnknownClip(s) => write!(f, "unknown clip `{s}`"),
        }
    }
}

impl std::error::Error for TimelineError {}

pub type Result<T> = std::result::Result<T, TimelineError>;

impl Default for ClipProps {
    fn default() -> Self {
        Self {
            clip_id: String::new(),
            gain_db: 0.0,
            pitch_semitones: 0.0,
            time_ratio: 1.0,
            fade_in_beats: 0.0,
            fade_out_beats: 0.0,
            fx: Vec::new(),
            out: None,
        }
    }
}

impl ClipProps {
    /// Flat (unity) props for one clip: audible no-op, routing untouched.
    pub fn flat(clip_id: &str) -> Self {
        Self {
            clip_id: clip_id.to_string(),
            ..Self::default()
        }
    }

    /// Range-check one prop set. Empty clip ids and non-positive time
    /// ratios are errors; out-of-range gain/pitch/fades are errors too
    /// (the renderer never silently reinterprets them).
    pub fn validate(&self) -> Result<()> {
        if self.clip_id.is_empty() {
            return Err(TimelineError::BadProps("clip_id must be non-empty".into()));
        }
        if !(self.gain_db >= -60.0 && self.gain_db <= 12.0) {
            return Err(TimelineError::BadProps(format!(
                "gain_db {} out of [-60, +12]",
                self.gain_db
            )));
        }
        if !(self.pitch_semitones >= -24.0 && self.pitch_semitones <= 24.0) {
            return Err(TimelineError::BadProps(format!(
                "pitch_semitones {} out of [-24, +24]",
                self.pitch_semitones
            )));
        }
        if !(self.time_ratio > 0.0 && self.time_ratio.is_finite()) {
            return Err(TimelineError::BadProps(format!(
                "time_ratio {} must be finite and > 0",
                self.time_ratio
            )));
        }
        if !(self.fade_in_beats >= 0.0 && self.fade_in_beats.is_finite())
            || !(self.fade_out_beats >= 0.0 && self.fade_out_beats.is_finite())
        {
            return Err(TimelineError::BadProps("fades must be finite and >= 0".into()));
        }
        Ok(())
    }

    /// Effective audible length under the time ratio.
    pub fn effective_length_beats(&self, clip: &Clip) -> f64 {
        clip.length_beats / self.time_ratio
    }
}

impl TimelineDoc {
    pub fn empty() -> Self {
        Self {
            sections: Vec::new(),
            arrangements: Vec::new(),
            clip_props: Vec::new(),
        }
    }

    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }

    /// Insert a section object. Rejects duplicates and non-positive or
    /// non-finite ranges; adjacency (end == next start) is legal.
    pub fn add_section(&mut self, section: Section) -> Result<()> {
        if self.sections.iter().any(|s| s.id == section.id) {
            return Err(TimelineError::DuplicateSection(section.id));
        }
        if !(section.start_beats.is_finite() && section.start_beats >= 0.0) {
            return Err(TimelineError::BadRange(format!(
                "start_beats {} must be finite and >= 0",
                section.start_beats
            )));
        }
        if !(section.length_beats.is_finite() && section.length_beats > 0.0) {
            return Err(TimelineError::BadRange(format!(
                "length_beats {} must be finite and > 0",
                section.length_beats
            )));
        }
        self.sections.push(section);
        Ok(())
    }

    /// Insert or replace the prop set for one clip (validated).
    pub fn set_clip_props(&mut self, props: ClipProps) -> Result<()> {
        props.validate()?;
        match self.clip_props.iter_mut().find(|p| p.clip_id == props.clip_id) {
            Some(slot) => *slot = props,
            None => self.clip_props.push(props),
        }
        Ok(())
    }

    /// Props for one clip, or flat defaults when no override exists.
    pub fn props_for(&self, clip_id: &str) -> ClipProps {
        self.clip_props
            .iter()
            .find(|p| p.clip_id == clip_id)
            .cloned()
            .unwrap_or_else(|| ClipProps::flat(clip_id))
    }
}

/// End of a clip in timeline beats.
pub fn clip_end(clip: &Clip) -> f64 {
    clip.start_beats + clip.length_beats
}

/// End of a section in timeline beats.
pub fn section_end(section: &Section) -> f64 {
    section.start_beats + section.length_beats
}

/// Beats -> seconds at one tempo (`tempo` in BPM, must be > 0).
pub fn beats_to_seconds(beats: f64, tempo: f64) -> f64 {
    beats * 60.0 / tempo
}

/// Seconds -> beats at one tempo.
pub fn seconds_to_beats(seconds: f64, tempo: f64) -> f64 {
    seconds * tempo / 60.0
}

/// Linear view: clips on one track, sorted by start (then id for ties).
pub fn clips_sorted_on_track<'a>(project: &'a Project, track_id: &str) -> Vec<&'a Clip> {
    let mut clips: Vec<&Clip> = project
        .clips
        .iter()
        .filter(|c| c.track_id == track_id)
        .collect();
    clips.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    clips
}

/// Overlapping clip pairs on one track (half-open `[start, end)`; touching
/// edges are not overlaps). Sorted by `(a, b)` id pair for determinism.
pub fn find_overlaps(project: &Project, track_id: &str) -> Vec<(String, String)> {
    let clips = clips_sorted_on_track(project, track_id);
    let mut out = Vec::new();
    for i in 0..clips.len() {
        for j in (i + 1)..clips.len() {
            if clips[j].start_beats >= clip_end(clips[i]) {
                break; // sorted: no later clip can overlap clips[i]
            }
            out.push((clips[i].id.clone(), clips[j].id.clone()));
        }
    }
    out
}

/// Clips overlapping a section's beat range (any track; sorted by start).
pub fn section_clips<'a>(project: &'a Project, section: &Section) -> Vec<&'a Clip> {
    let end = section_end(section);
    let mut clips: Vec<&Clip> = project
        .clips
        .iter()
        .filter(|c| c.start_beats < end && clip_end(c) > section.start_beats)
        .collect();
    clips.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    clips
}

/// Launcher view: one slot per `(section, track)` holding the overlapping
/// clip ids. Sections keep doc order; tracks keep project order; clip ids
/// sort by clip start. Empty cells are kept (a silent slot is still a slot).
pub fn launcher_slots(project: &Project, doc: &TimelineDoc) -> Vec<LauncherSlot> {
    let mut slots = Vec::new();
    for section in &doc.sections {
        let end = section_end(section);
        for track in &project.tracks {
            let mut ids: Vec<(&str, f64)> = project
                .clips
                .iter()
                .filter(|c| {
                    c.track_id == track.id && c.start_beats < end && clip_end(c) > section.start_beats
                })
                .map(|c| (c.id.as_str(), c.start_beats))
                .collect();
            ids.sort_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(b.0))
            });
            slots.push(LauncherSlot {
                section_id: section.id.clone(),
                track_id: track.id.clone(),
                clip_ids: ids.into_iter().map(|(id, _)| id.to_string()).collect(),
            });
        }
    }
    slots
}

/// Lay an arrangement out: sections concatenate in listed order, each placed
/// at the running offset. Unknown section ids are errors (a dangling order
/// entry must fail loudly, never render silence).
pub fn arrangement_layout(doc: &TimelineDoc, arrangement_id: &str) -> Result<Vec<ArrangedSection>> {
    let arrangement = doc
        .arrangements
        .iter()
        .find(|a| a.id == arrangement_id)
        .ok_or_else(|| TimelineError::UnknownArrangement(arrangement_id.to_string()))?;
    let mut out = Vec::with_capacity(arrangement.section_ids.len());
    let mut offset = 0.0;
    for id in &arrangement.section_ids {
        let section = doc
            .section(id)
            .ok_or_else(|| TimelineError::UnknownSection(id.clone()))?;
        out.push(ArrangedSection {
            section_id: section.id.clone(),
            name: section.name.clone(),
            offset_beats: offset,
            length_beats: section.length_beats,
        });
        offset += section.length_beats;
    }
    Ok(out)
}

/// Build the frozen `ClipMoved` op moving one clip. The payload uses the
/// frozen `{"startBeats": n}` shape the engine already parses; `seq` is a
/// placeholder (the engine assigns it) and the caller picks the actor.
pub fn move_clip_op(actor: &str, clip_id: &str, new_start_beats: f64) -> Op {
    Op {
        seq: 0,
        actor: actor.to_string(),
        kind: OpKind::ClipMoved,
        target: clip_id.to_string(),
        value_json: format!("{{\"startBeats\": {new_start_beats}}}"),
    }
}

/// Move a whole section by `delta_beats`: one frozen `ClipMoved` op per
/// member clip (section motion is a fan-out, not a new op kind). Errors when
/// the section is unknown; skips nothing — every overlapping clip moves.
pub fn move_section_ops(
    actor: &str,
    project: &Project,
    section: &Section,
    delta_beats: f64,
) -> Vec<Op> {
    section_clips(project, section)
        .into_iter()
        .map(|c| move_clip_op(actor, &c.id, c.start_beats + delta_beats))
        .collect()
}

/// Demo timeline project: two tracks, Verse clips plus a two-clip Chorus.
/// `clip_chorus_2` (bass, Chorus, start 24) is the move-Chorus-2 validation
/// target below and in `ui/tests/timeline.test.ts`.
pub fn sample_timeline_project() -> Project {
    use crate::model::{ClipKind, Track};
    let mut project = Project::new("proj_timeline", "Timeline Demo");
    project.tracks.push(Track {
        id: "trk_drums".to_string(),
        name: "Drums".to_string(),
        volume: 0.8,
        pan: 0.0,
        muted: false,
        solo: false,
        clip_ids: vec!["clip_verse_1".to_string(), "clip_chorus_1".to_string()],
        device_ids: vec![],
    });
    project.tracks.push(Track {
        id: "trk_bass".to_string(),
        name: "Bass".to_string(),
        volume: 0.8,
        pan: 0.0,
        muted: false,
        solo: false,
        clip_ids: vec!["clip_verse_2".to_string(), "clip_chorus_2".to_string()],
        device_ids: vec![],
    });
    let clip = |id: &str, track_id: &str, name: &str, start: f64, len: f64| Clip {
        id: id.to_string(),
        track_id: track_id.to_string(),
        name: name.to_string(),
        start_beats: start,
        length_beats: len,
        kind: ClipKind::Midi,
        source: format!("take:{id}"),
    };
    project.clips.push(clip("clip_verse_1", "trk_drums", "Verse-1", 0.0, 8.0));
    project.clips.push(clip("clip_verse_2", "trk_bass", "Verse-2", 8.0, 8.0));
    project.clips.push(clip("clip_chorus_1", "trk_drums", "Chorus-1", 16.0, 8.0));
    project.clips.push(clip("clip_chorus_2", "trk_bass", "Chorus-2", 24.0, 8.0));
    project
}

/// Sidecar doc for the demo project: Verse `0..16`, Chorus `16..32`, plus a
/// linear arrangement and an alternate (radio) order over the same objects.
pub fn sample_timeline_doc() -> TimelineDoc {
    TimelineDoc {
        sections: vec![
            Section {
                id: "sec_verse".to_string(),
                name: "Verse".to_string(),
                start_beats: 0.0,
                length_beats: 16.0,
            },
            Section {
                id: "sec_chorus".to_string(),
                name: "Chorus".to_string(),
                start_beats: 16.0,
                length_beats: 16.0,
            },
        ],
        arrangements: vec![
            Arrangement {
                id: "arr_linear".to_string(),
                name: "Linear".to_string(),
                section_ids: vec!["sec_verse".to_string(), "sec_chorus".to_string()],
            },
            Arrangement {
                id: "arr_radio".to_string(),
                name: "Radio".to_string(),
                section_ids: vec![
                    "sec_chorus".to_string(),
                    "sec_verse".to_string(),
                    "sec_chorus".to_string(),
                ],
            },
        ],
        clip_props: vec![ClipProps {
            clip_id: "clip_chorus_2".to_string(),
            gain_db: 2.0,
            pitch_semitones: -2.0,
            time_ratio: 1.0,
            fade_in_beats: 0.5,
            fade_out_beats: 1.0,
            fx: vec!["dev_chorus_dub".to_string()],
            out: Some("bus_dub".to_string()),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::model::OpKind;

    fn test_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ccez-timeline-{name}-{}", std::process::id()))
    }

    #[test]
    fn move_chorus_2_via_frozen_clip_moved_op() {
        // Validation test for Track E: moving clip `Chorus-2` is one frozen
        // ClipMoved op through the Track A engine, and undo restores it.
        let dir = test_dir("move-chorus-2");
        let _ = std::fs::remove_dir_all(&dir);
        let project = sample_timeline_project();
        let mut engine = Engine::create(&dir, project).expect("create");
        let before = engine
            .project()
            .clips
            .iter()
            .find(|c| c.id == "clip_chorus_2")
            .expect("seed clip")
            .start_beats;
        assert_eq!(before, 24.0);

        // Op shape the launcher/linear views both emit for a drag.
        let op = move_clip_op("ui", "clip_chorus_2", 32.0);
        assert_eq!(op.kind, OpKind::ClipMoved);
        assert_eq!(op.target, "clip_chorus_2");
        let payload: serde_json::Value =
            serde_json::from_str(&op.value_json).expect("payload is JSON");
        assert_eq!(payload.get("startBeats").and_then(|v| v.as_f64()), Some(32.0));

        let seq = engine
            .apply(&op.actor, op.kind.clone(), &op.target, &op.value_json)
            .expect("apply move");
        assert!(seq >= 1);
        let moved = engine
            .project()
            .clips
            .iter()
            .find(|c| c.id == "clip_chorus_2")
            .expect("moved clip")
            .start_beats;
        assert_eq!(moved, 32.0);

        // Undo is also history: the clip is back at 24.
        engine.undo().expect("undo move");
        let restored = engine
            .project()
            .clips
            .iter()
            .find(|c| c.id == "clip_chorus_2")
            .expect("restored clip")
            .start_beats;
        assert_eq!(restored, 24.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moving_a_section_fans_out_to_member_clips() {
        let project = sample_timeline_project();
        let doc = sample_timeline_doc();
        let chorus = doc.section("sec_chorus").expect("chorus");
        let ops = move_section_ops("ui", &project, chorus, 8.0);
        // Chorus holds exactly the two chorus clips.
        let mut targets: Vec<&str> = ops.iter().map(|op| op.target.as_str()).collect();
        targets.sort_unstable();
        assert_eq!(targets, vec!["clip_chorus_1", "clip_chorus_2"]);
        for op in &ops {
            assert_eq!(op.kind, OpKind::ClipMoved);
        }
        // Verse clips are untouched by a chorus move.
        assert!(ops.iter().all(|op| !op.target.starts_with("clip_verse")));
    }

    #[test]
    fn sections_are_objects_with_beat_ranges() {
        let doc = sample_timeline_doc();
        let chorus = doc.section("sec_chorus").expect("chorus");
        assert_eq!(section_end(chorus), 32.0);
        let project = sample_timeline_project();
        let ids: Vec<&str> = section_clips(&project, chorus)
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, vec!["clip_chorus_1", "clip_chorus_2"]);
        // Adjacent sections share an edge without overlapping membership.
        let verse = doc.section("sec_verse").expect("verse");
        assert_eq!(section_end(verse), chorus.start_beats);
    }

    #[test]
    fn multiple_arrangements_share_section_objects() {
        let doc = sample_timeline_doc();
        let linear = arrangement_layout(&doc, "arr_linear").expect("linear");
        assert_eq!(linear.len(), 2);
        assert_eq!(linear[0].offset_beats, 0.0);
        assert_eq!(linear[1].offset_beats, 16.0);
        assert_eq!(linear[1].section_id, "sec_chorus");
        // The radio order reuses the same chorus object twice, no copies.
        let radio = arrangement_layout(&doc, "arr_radio").expect("radio");
        assert_eq!(radio.len(), 3);
        assert_eq!(radio[2].offset_beats, 32.0);
        assert_eq!(radio[2].section_id, "sec_chorus");
        // Total length follows the order, not the timeline: 16+16+16.
        let total = radio.last().expect("last").offset_beats
            + radio.last().expect("last").length_beats;
        assert_eq!(total, 48.0);
    }

    #[test]
    fn launcher_and_linear_are_views_over_one_model() {
        let project = sample_timeline_project();
        let doc = sample_timeline_doc();
        // Linear: sorted lanes per track.
        let drums = clips_sorted_on_track(&project, "trk_drums");
        assert_eq!(drums.len(), 2);
        assert!(drums[0].start_beats <= drums[1].start_beats);
        // Launcher: same clips, grouped into (section, track) slots.
        let slots = launcher_slots(&project, &doc);
        assert_eq!(slots.len(), 4); // 2 sections x 2 tracks
        let chorus_drums = slots
            .iter()
            .find(|s| s.section_id == "sec_chorus" && s.track_id == "trk_drums")
            .expect("chorus drums slot");
        assert_eq!(chorus_drums.clip_ids, vec!["clip_chorus_1".to_string()]);
        // Every clip appears in exactly one slot per covering section.
        let all: Vec<&str> = slots
            .iter()
            .flat_map(|s| s.clip_ids.iter().map(|id| id.as_str()))
            .collect();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn object_level_clip_props_validate_and_default() {
        let doc = sample_timeline_doc();
        let props = doc.props_for("clip_chorus_2");
        props.validate().expect("seed props valid");
        assert_eq!(props.fx, vec!["dev_chorus_dub".to_string()]);
        assert_eq!(props.out.as_deref(), Some("bus_dub"));
        assert_eq!(props.pitch_semitones, -2.0);
        // Clips without overrides get flat defaults (audible no-op).
        let flat = doc.props_for("clip_verse_1");
        flat.validate().expect("flat props valid");
        assert_eq!(flat.gain_db, 0.0);
        assert_eq!(flat.time_ratio, 1.0);
        assert!(flat.fx.is_empty() && flat.out.is_none());
        // Effective length follows the time ratio.
        let project = sample_timeline_project();
        let clip = project
            .clips
            .iter()
            .find(|c| c.id == "clip_chorus_2")
            .expect("clip");
        assert_eq!(props.effective_length_beats(clip), 8.0);
        // Out-of-range props fail loudly, never silently reinterpreted.
        let mut bad = ClipProps::flat("clip_verse_1");
        bad.pitch_semitones = 48.0;
        assert!(bad.validate().is_err());
        bad = ClipProps::flat("clip_verse_1");
        bad.time_ratio = 0.0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn overlaps_detected_on_a_track() {
        let mut project = sample_timeline_project();
        assert!(find_overlaps(&project, "trk_bass").is_empty());
        project.clips.push(Clip {
            id: "clip_overlap".to_string(),
            track_id: "trk_bass".to_string(),
            name: "Overlap".to_string(),
            start_beats: 26.0,
            length_beats: 4.0,
            kind: crate::model::ClipKind::Audio,
            source: "take:overlap".to_string(),
        });
        let overlaps = find_overlaps(&project, "trk_bass");
        assert_eq!(overlaps.len(), 1);
        assert!(overlaps[0].0 == "clip_chorus_2" || overlaps[0].1 == "clip_chorus_2");
    }

    #[test]
    fn beats_convert_at_tempo() {
        assert_eq!(beats_to_seconds(4.0, 120.0), 2.0);
        assert_eq!(seconds_to_beats(2.0, 120.0), 4.0);
    }

    #[test]
    fn timeline_doc_round_trips_through_json() {
        let doc = sample_timeline_doc();
        let json = serde_json::to_string(&doc).expect("serialize");
        let back: TimelineDoc = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(doc, back);
    }
}
