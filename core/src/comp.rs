//! Track E (agent 2): comping + retrospective capture.
//!
//! Two recording-workflow helpers, both pure functions over the frozen v0
//! model — no new `OpKind`, no schema change, so the typegen drift gate
//! (`bun run typegen -- --check`) is unaffected (like `branch.rs`).
//!
//! **Comping** is how a DAW turns loop-recorded passes into one keeper
//! performance. Each pass records a full-region *take* clip; the player then
//! picks the best moment from each pass as *sections* ("bar 1 from take 2,
//! bar 2 from take 1"), and [`build_comp`] stitches those sections into one
//! composite [`Clip`](crate::model::Clip) whose `source` names the takes it
//! was cut from (`comp:<take-a>+<take-b>` — opaque, like every `source`).
//! The composite is an ordinary clip: it enters the project through the
//! frozen `ClipAdded` op, so it is undoable and survives restart for free.
//!
//! **Retrospective capture** is the "I wasn't recording!" button. A
//! [`RetroBuffer`] keeps the last N played events (stamped in beats by the
//! caller — this module owns no clock) so a good improvisation played while
//! the transport was merely *playing* can be materialized into a clip with
//! [`RetroBuffer::to_clip`]. Empty window = `None`, never an empty clip.

use crate::model::{Clip, ClipKind};

/// Float tolerance for section-boundary comparisons, in beats.
const EPS: f64 = 1e-9;

#[derive(Debug)]
pub enum CompError {
    EmptySections,
    UnknownTake(String),
    WrongTrack { take: String, want: String },
    BadRange(String),
    GapOrOverlap { at_beats: f64 },
    Uncovered { start_beats: f64, end_beats: f64 },
    MixedKinds,
}

impl std::fmt::Display for CompError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySections => write!(f, "comp needs at least one section"),
            Self::UnknownTake(t) => write!(f, "unknown comp take `{t}`"),
            Self::WrongTrack { take, want } => {
                write!(f, "take `{take}` is not on track `{want}`")
            }
            Self::BadRange(m) => write!(f, "bad comp range: {m}"),
            Self::GapOrOverlap { at_beats } => {
                write!(f, "comp sections gap or overlap at beat {at_beats}")
            }
            Self::Uncovered { start_beats, end_beats } => write!(
                f,
                "take does not cover comp section [{start_beats}, {end_beats})"
            ),
            Self::MixedKinds => write!(f, "comp takes must all be the same clip kind"),
        }
    }
}

impl std::error::Error for CompError {}

pub type Result<T> = std::result::Result<T, CompError>;

/// One slice of the composite: beats `[start_beats, end_beats)` come from
/// the take clip named by `take_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompSection {
    pub take_id: String,
    pub start_beats: f64,
    pub end_beats: f64,
}

impl CompSection {
    pub fn new(take_id: &str, start_beats: f64, end_beats: f64) -> Self {
        Self {
            take_id: take_id.to_string(),
            start_beats,
            end_beats,
        }
    }
}

/// All take clips on `track_id` overlapping `[start_beats, end_beats)`,
/// sorted by start beat. Takes are ordinary clips whose `source` is a
/// `take:<n>` reference; loop recording just stacks them.
pub fn takes_for_region<'a>(
    clips: &'a [Clip],
    track_id: &str,
    start_beats: f64,
    end_beats: f64,
) -> Vec<&'a Clip> {
    let mut takes: Vec<&Clip> = clips
        .iter()
        .filter(|c| {
            c.track_id == track_id
                && c.start_beats < end_beats
                && c.start_beats + c.length_beats > start_beats
        })
        .collect();
    takes.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    takes
}

/// Stitch `sections` into one composite clip on `track_id`.
///
/// Rules (all checked, in order):
/// - at least one section; every section has `start < end`;
/// - sections are contiguous: each starts where the previous ended
///   (within `EPS`) — gaps and overlaps are rejected;
/// - every named take exists, sits on `track_id`, covers its section, and
///   all takes share one [`ClipKind`].
///
/// The composite spans the sections, inherits the takes' kind, and carries
/// `source = "comp:<take-1>+<take-2>..."` in section order, so the edit
/// stays traceable without a schema change.
pub fn build_comp(
    id: &str,
    track_id: &str,
    name: &str,
    sections: &[CompSection],
    takes: &[Clip],
) -> Result<Clip> {
    if sections.is_empty() {
        return Err(CompError::EmptySections);
    }
    for s in sections {
        if !(s.start_beats < s.end_beats) {
            return Err(CompError::BadRange(format!(
                "[{}, {})",
                s.start_beats, s.end_beats
            )));
        }
    }
    for pair in sections.windows(2) {
        if (pair[1].start_beats - pair[0].end_beats).abs() > EPS {
            return Err(CompError::GapOrOverlap {
                at_beats: pair[0].end_beats,
            });
        }
    }
    let mut kind: Option<ClipKind> = None;
    for s in sections {
        let take = takes
            .iter()
            .find(|c| c.id == s.take_id)
            .ok_or_else(|| CompError::UnknownTake(s.take_id.clone()))?;
        if take.track_id != track_id {
            return Err(CompError::WrongTrack {
                take: take.id.clone(),
                want: track_id.to_string(),
            });
        }
        let take_end = take.start_beats + take.length_beats;
        if take.start_beats - s.start_beats > EPS || s.end_beats - take_end > EPS {
            return Err(CompError::Uncovered {
                start_beats: s.start_beats,
                end_beats: s.end_beats,
            });
        }
        match &kind {
            None => kind = Some(take.kind.clone()),
            Some(k) if *k == take.kind => {}
            Some(_) => return Err(CompError::MixedKinds),
        }
    }
    let start_beats = sections.first().expect("checked").start_beats;
    let end_beats = sections.last().expect("checked").end_beats;
    let source = format!(
        "comp:{}",
        sections
            .iter()
            .map(|s| s.take_id.as_str())
            .collect::<Vec<_>>()
            .join("+")
    );
    Ok(Clip {
        id: id.to_string(),
        track_id: track_id.to_string(),
        name: name.to_string(),
        start_beats,
        length_beats: end_beats - start_beats,
        kind: kind.expect("checked"),
        source,
    })
}

/// One retrospectively captured event: opaque payload `data` (note JSON,
/// MIDI blob key, ...) played at `beat`. Beats are stamped by the caller;
/// this buffer owns storage, not time.
#[derive(Debug, Clone, PartialEq)]
pub struct RetroEvent {
    pub beat: f64,
    pub data: String,
}

/// Always-on capture ring: `push` every played event whether or not the
/// transport is recording; `to_clip` rescues the window afterwards.
/// Capacity-bounded (oldest events fall off); zero capacity keeps nothing.
#[derive(Debug, Clone, Default)]
pub struct RetroBuffer {
    capacity: usize,
    events: std::collections::VecDeque<RetroEvent>,
}

impl RetroBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: std::collections::VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn push(&mut self, beat: f64, data: &str) {
        if self.capacity == 0 {
            return;
        }
        if self.events.len() >= self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(RetroEvent {
            beat,
            data: data.to_string(),
        });
    }

    /// Events with `beat >= since_beats`, in push order.
    pub fn retrieve(&self, since_beats: f64) -> Vec<&RetroEvent> {
        self.events
            .iter()
            .filter(|e| e.beat >= since_beats)
            .collect()
    }

    /// Materialize the window `[since_beats, ...)` as a clip on `track_id`,
    /// spanning first to last captured event. `None` when the window is
    /// empty — retrospective capture never invents an empty clip. The clip
    /// `source` records the event count (`retro:<n>-events`) and stays
    /// opaque per the frozen schema.
    pub fn to_clip(
        &self,
        id: &str,
        track_id: &str,
        name: &str,
        kind: ClipKind,
        since_beats: f64,
    ) -> Option<Clip> {
        let window: Vec<&RetroEvent> = self.retrieve(since_beats);
        if window.is_empty() {
            return None;
        }
        let first = window.first().expect("checked").beat;
        let last = window.last().expect("checked").beat;
        Some(Clip {
            id: id.to_string(),
            track_id: track_id.to_string(),
            name: name.to_string(),
            start_beats: first,
            length_beats: (last - first).max(EPS),
            kind,
            source: format!("retro:{}-events", window.len()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::apply_op_to_project;
    use crate::model::{ClipKind, Project, Track};

    fn take(id: &str, start: f64, len: f64) -> Clip {
        Clip {
            id: id.to_string(),
            track_id: "trk_vox".to_string(),
            name: id.to_string(),
            start_beats: start,
            length_beats: len,
            kind: ClipKind::Audio,
            source: format!("take:{id}"),
        }
    }

    fn vox_project(takes: Vec<Clip>) -> Project {
        let mut p = Project::new("proj_comp", "Comp test");
        p.tracks.push(Track {
            id: "trk_vox".to_string(),
            name: "Vox".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: takes.iter().map(|c| c.id.clone()).collect(),
            device_ids: vec![],
        });
        p.clips = takes;
        p
    }

    #[test]
    fn comp_take_builds_composite_and_applies_as_clip_added() {
        // Three loop passes over bars 0-4; keep bar 0-2 from take 2 and
        // bar 2-4 from take 1.
        let takes = vec![take("take_1", 0.0, 4.0), take("take_2", 0.0, 4.0)];
        let sections = vec![
            CompSection::new("take_2", 0.0, 2.0),
            CompSection::new("take_1", 2.0, 4.0),
        ];
        let comp = build_comp("clip_comp", "trk_vox", "Vox comp", &sections, &takes)
            .expect("valid comp");
        assert_eq!(comp.start_beats, 0.0);
        assert_eq!(comp.length_beats, 4.0);
        assert_eq!(comp.kind, ClipKind::Audio);
        assert_eq!(comp.source, "comp:take_2+take_1");

        // The composite enters the project as an ordinary ClipAdded op:
        // undoable, logged, restart-safe via the Track A engine.
        let mut project = vox_project(takes);
        let op = crate::model::Op {
            seq: 1,
            actor: "ui".to_string(),
            kind: crate::model::OpKind::ClipAdded,
            target: comp.id.clone(),
            value_json: serde_json::to_string(&comp).expect("serialize"),
        };
        apply_op_to_project(&mut project, &op).expect("apply comp clip");
        assert!(project.clips.iter().any(|c| c.id == "clip_comp"));
        assert!(project.tracks[0].clip_ids.contains(&"clip_comp".to_string()));
    }

    #[test]
    fn comp_take_rejects_gaps_overlaps_and_unknown_takes() {
        let takes = vec![take("take_1", 0.0, 4.0), take("take_2", 0.0, 4.0)];
        // Gap between sections.
        assert!(matches!(
            build_comp(
                "c",
                "trk_vox",
                "x",
                &[CompSection::new("take_1", 0.0, 1.0), CompSection::new("take_2", 2.0, 4.0)],
                &takes
            ),
            Err(CompError::GapOrOverlap { .. })
        ));
        // Overlap between sections.
        assert!(matches!(
            build_comp(
                "c",
                "trk_vox",
                "x",
                &[CompSection::new("take_1", 0.0, 3.0), CompSection::new("take_2", 2.0, 4.0)],
                &takes
            ),
            Err(CompError::GapOrOverlap { .. })
        ));
        // Unknown take.
        assert!(matches!(
            build_comp(
                "c",
                "trk_vox",
                "x",
                &[CompSection::new("take_9", 0.0, 4.0)],
                &takes
            ),
            Err(CompError::UnknownTake(_))
        ));
        // Section outside the take's coverage.
        assert!(matches!(
            build_comp(
                "c",
                "trk_vox",
                "x",
                &[CompSection::new("take_1", 2.0, 8.0)],
                &takes
            ),
            Err(CompError::Uncovered { .. })
        ));
        // Empty sections.
        assert!(matches!(
            build_comp("c", "trk_vox", "x", &[], &takes),
            Err(CompError::EmptySections)
        ));
    }

    #[test]
    fn takes_for_region_lists_overlapping_takes_in_order() {
        let takes = vec![take("take_2", 4.0, 4.0), take("take_1", 0.0, 4.0)];
        let found = takes_for_region(&takes, "trk_vox", 0.0, 8.0);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, "take_1");
        let found = takes_for_region(&takes, "trk_vox", 0.0, 4.0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "take_1");
        assert!(takes_for_region(&takes, "trk_other", 0.0, 8.0).is_empty());
    }

    #[test]
    fn retrospective_capture_materializes_played_window_as_clip() {
        let mut buf = RetroBuffer::new(128);
        // Played while merely playing, never recording.
        buf.push(4.0, r#"{"note":60}"#);
        buf.push(4.5, r#"{"note":64}"#);
        buf.push(5.0, r#"{"note":67}"#);
        let heard = buf.retrieve(4.0);
        assert_eq!(heard.len(), 3);
        assert!(buf.retrieve(6.0).is_empty());

        let clip = buf
            .to_clip("clip_retro", "trk_vox", "Rescued idea", ClipKind::Midi, 4.0)
            .expect("window nonempty");
        assert_eq!(clip.start_beats, 4.0);
        assert_eq!(clip.length_beats, 1.0);
        assert_eq!(clip.source, "retro:3-events");

        // Empty window rescues nothing — never an empty clip.
        assert!(buf
            .to_clip("clip_none", "trk_vox", "Nothing", ClipKind::Midi, 6.0)
            .is_none());

        // Capacity bounds memory: oldest falls off.
        let mut small = RetroBuffer::new(2);
        small.push(0.0, "a");
        small.push(1.0, "b");
        small.push(2.0, "c");
        assert_eq!(small.len(), 2);
        assert_eq!(small.retrieve(0.0)[0].data, "b");
    }
}
