//! Agent 7 (record workflow): punch in/out, count-in/metronome integration,
//! software direct monitoring with latency compensation, and take lanes
//! feeding the comping model.
//!
//! Teaching note: recording is the one workflow where time has three
//! simultaneous meanings, and this module keeps them separate on purpose:
//!
//! - **Transport beats** are where the playhead is (`beat` arguments below).
//! - **Wall-clock latency** is what the audio interface adds on the way in
//!   and out (measured in samples, converted to beats for compensation).
//! - **Take identity** is which pass a piece of audio came from (plain
//!   [`Clip`]s whose `source` is a `take:<id>` reference, stacked as lanes).
//!
//! Everything here is pure functions over the frozen v0 model — no new
//! `OpKind`, no schema change, no IPC surface — so the typegen drift gate
//! (`bun run typegen -- --check`) is unaffected (same stance as `comp.rs`).
//! Punch takes enter the project through the frozen `ClipAdded` op and comp
//! through [`crate::comp::build_comp`], so the whole workflow is undoable
//! for free.
//!
//! **Hardware inserts are explicitly out of scope.** There is no code path
//! here that addresses outboard gear: monitoring is software-only (input
//! through the DAW, compensated for interface latency). If you need a
//! hardware send/return loop, that is a different track, not a flag on this
//! one.

use crate::model::{Clip, ClipKind, EngineState};

/// Float tolerance for beat comparisons, in beats (same as `comp::EPS`).
const EPS: f64 = 1e-9;

/// A validated auto-punch region: beats `[start_beats, end_beats)`.
#[derive(Debug, Clone, PartialEq)]
pub struct PunchRange {
    pub start_beats: f64,
    pub end_beats: f64,
}

/// How recording engages: the player punches by hand, or the transport
/// punches automatically inside a range.
#[derive(Debug, Clone, PartialEq)]
pub enum PunchMode {
    /// Capture whenever the transport is recording (player's footswitch /
    /// key command decides).
    Manual,
    /// Capture only while the playhead is inside the range.
    Auto(PunchRange),
}

/// One count-in click: how many beats before the record start it sounds
/// (`offset_beats` is negative; `0` would be the downbeat itself, which is
/// never a click), and whether it is a bar-downbeat accent.
#[derive(Debug, Clone, PartialEq)]
pub struct CountInClick {
    pub offset_beats: f64,
    pub accent: bool,
}

/// Count-in length, in musical units. Beats per bar comes from the project
/// time signature so the click accents land on real bar lines.
#[derive(Debug, Clone, PartialEq)]
pub struct CountIn {
    pub bars: u32,
    pub beats_per_bar: u32,
}

/// Software monitoring switch, per armed track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorMode {
    /// Never route the live input to the output.
    Off,
    /// Route live input when the track is armed and the transport is *not*
    /// playing back takes (stopped or recording): during playback you hear
    /// the recorded lanes, otherwise you hear yourself.
    Auto,
    /// Route live input whenever the track is armed.
    On,
}

/// One take lane: the take clip plus its stacking position. Lane 0 is the
/// earliest-recorded (or earliest-starting) take; later passes stack above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeLane {
    pub take_id: String,
    pub lane: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordError {
    BadPunchRange(String),
    BadCountIn(String),
    BadTempo(String),
    BadLatency(String),
    NoTakes,
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPunchRange(m) => write!(f, "bad punch range: {m}"),
            Self::BadCountIn(m) => write!(f, "bad count-in: {m}"),
            Self::BadTempo(m) => write!(f, "bad tempo: {m}"),
            Self::BadLatency(m) => write!(f, "bad latency: {m}"),
            Self::NoTakes => write!(f, "no takes to lane"),
        }
    }
}

impl std::error::Error for RecordError {}

pub type Result<T> = std::result::Result<T, RecordError>;

/// Validate a punch range: finite, `start >= 0`, `start < end`.
pub fn validate_punch(start_beats: f64, end_beats: f64) -> Result<PunchRange> {
    if !start_beats.is_finite() || !end_beats.is_finite() {
        return Err(RecordError::BadPunchRange(
            "bounds must be finite".to_string(),
        ));
    }
    if start_beats < 0.0 {
        return Err(RecordError::BadPunchRange(
            "punch cannot start before beat 0".to_string(),
        ));
    }
    if !(start_beats < end_beats) {
        return Err(RecordError::BadPunchRange(format!(
            "need start < end, got [{start_beats}, {end_beats})"
        )));
    }
    Ok(PunchRange {
        start_beats,
        end_beats,
    })
}

/// Should audio at `beat` be captured? Manual punches whenever the
/// transport records; auto punches only inside its range (end-exclusive,
/// with `EPS` tolerance so a playhead sitting exactly on the out-point
/// counts as out).
pub fn is_punching(beat: f64, mode: &PunchMode, transport_recording: bool) -> bool {
    if !transport_recording {
        return false;
    }
    match mode {
        PunchMode::Manual => true,
        PunchMode::Auto(range) => {
            beat + EPS >= range.start_beats && beat + EPS < range.end_beats
        }
    }
}

/// Total count-in length in beats: `bars * beats_per_bar`.
pub fn count_in_beats(count_in: &CountIn) -> Result<f64> {
    if count_in.bars == 0 {
        return Err(RecordError::BadCountIn(
            "count-in needs at least one bar".to_string(),
        ));
    }
    if count_in.beats_per_bar == 0 {
        return Err(RecordError::BadCountIn(
            "beats per bar must be >= 1".to_string(),
        ));
    }
    Ok(count_in.bars as f64 * count_in.beats_per_bar as f64)
}

/// Count-in length in seconds at `tempo_bpm`.
pub fn count_in_seconds(count_in: &CountIn, tempo_bpm: f64) -> Result<f64> {
    if !(tempo_bpm > 0.0) || !tempo_bpm.is_finite() {
        return Err(RecordError::BadTempo(format!(
            "tempo must be a positive finite BPM, got {tempo_bpm}"
        )));
    }
    Ok(count_in_beats(count_in)? * 60.0 / tempo_bpm)
}

/// The click schedule for a count-in: one click per beat, at negative
/// offsets counting up to (but excluding) the record downbeat. The first
/// click of each bar is accented. A 1-bar 4/4 count-in therefore clicks at
/// `-4, -3, -2, -1` with the accent on `-4`.
pub fn count_in_clicks(count_in: &CountIn) -> Result<Vec<CountInClick>> {
    let total = count_in_beats(count_in)? as u64;
    let per_bar = count_in.beats_per_bar as u64;
    let mut clicks = Vec::with_capacity(total as usize);
    for i in 0..total {
        let offset_beats = i as f64 - total as f64;
        clicks.push(CountInClick {
            offset_beats,
            accent: i % per_bar == 0,
        });
    }
    Ok(clicks)
}

/// Should the live input be audible? `Off` never, `On` whenever armed,
/// `Auto` when armed and the transport is not playing back takes.
pub fn should_monitor(
    mode: &MonitorMode,
    track_armed: bool,
    transport: &EngineState,
) -> bool {
    match mode {
        MonitorMode::Off => false,
        MonitorMode::On => track_armed,
        MonitorMode::Auto => track_armed && *transport != EngineState::Playing,
    }
}

/// Round-trip interface latency expressed in beats: the samples spent
/// getting in *plus* getting out, converted at `sample_rate_hz` and
/// `tempo_bpm`. Software monitoring hears the input this late, so a
/// performance captured against the click lands this late on the timeline.
pub fn latency_beats(
    input_latency_samples: u64,
    output_latency_samples: u64,
    sample_rate_hz: f64,
    tempo_bpm: f64,
) -> Result<f64> {
    if !(sample_rate_hz > 0.0) || !sample_rate_hz.is_finite() {
        return Err(RecordError::BadLatency(format!(
            "sample rate must be positive finite, got {sample_rate_hz}"
        )));
    }
    if !(tempo_bpm > 0.0) || !tempo_bpm.is_finite() {
        return Err(RecordError::BadTempo(format!(
            "tempo must be a positive finite BPM, got {tempo_bpm}"
        )));
    }
    let total = input_latency_samples as f64 + output_latency_samples as f64;
    Ok(total / sample_rate_hz * tempo_bpm / 60.0)
}

/// Nudge a captured beat earlier by the measured latency so the take lines
/// up with what the player heard. Inverse of the interface delay: the
/// monitor path delays hearing, so the recording is shifted back by the
/// same amount.
pub fn compensate_capture(captured_beat: f64, latency_beats: f64) -> f64 {
    captured_beat - latency_beats
}

/// Stack takes into lanes: sorted by `(start_beats, id)` — the same order
/// [`crate::comp::takes_for_region`] returns — with lane index = position.
/// Empty input is an error: lanes describe recorded passes, and with no
/// passes there is nothing to comp.
pub fn assign_take_lanes(takes: &[Clip]) -> Result<Vec<TakeLane>> {
    if takes.is_empty() {
        return Err(RecordError::NoTakes);
    }
    let mut sorted: Vec<&Clip> = takes.iter().collect();
    sorted.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(sorted
        .iter()
        .enumerate()
        .map(|(lane, clip)| TakeLane {
            take_id: clip.id.clone(),
            lane,
        })
        .collect())
}

/// Build the take clip for one punched pass: it spans exactly the punch
/// range and carries `source = "take:<id>"`, the convention
/// [`crate::comp::build_comp`] consumes as comp input. The caller assigns
/// the id (e.g. `take_vox_p1`); the pass index only documents pass order.
pub fn punch_take_clip(
    id: &str,
    track_id: &str,
    name: &str,
    kind: ClipKind,
    punch: &PunchRange,
) -> Clip {
    Clip {
        id: id.to_string(),
        track_id: track_id.to_string(),
        name: name.to_string(),
        start_beats: punch.start_beats,
        length_beats: punch.end_beats - punch.start_beats,
        kind,
        source: format!("take:{id}"),
    }
}

/// The captured portion of a manual-punch pass inside an auto-punch-style
/// window: `None` when the pass misses the window entirely, otherwise the
/// `[start, end)` overlap in beats. Auto-punch passes always overlap fully;
/// a hand-punched pass may start early or punch out late, and only the
/// in-window slice becomes take material.
pub fn punch_overlap(
    pass_start_beats: f64,
    pass_end_beats: f64,
    window: &PunchRange,
) -> Option<(f64, f64)> {
    let start = pass_start_beats.max(window.start_beats);
    let end = pass_end_beats.min(window.end_beats);
    if start + EPS < end {
        Some((start, end))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comp::{build_comp, takes_for_region, CompSection};

    #[test]
    fn punch_take_two_passes_comp_into_keeper() {
        // The validation path: two auto-punched passes on one track become
        // take lanes, then comp sections stitch a keeper across both takes.
        let punch = validate_punch(0.0, 4.0).unwrap();
        let take_1 = punch_take_clip("take_vox_p1", "trk_vox", "Vox p1", ClipKind::Audio, &punch);
        let take_2 = punch_take_clip("take_vox_p2", "trk_vox", "Vox p2", ClipKind::Audio, &punch);
        assert_eq!(take_1.source, "take:take_vox_p1");
        assert_eq!(take_1.length_beats, 4.0);

        let clips = vec![take_1.clone(), take_2.clone()];
        let takes: Vec<Clip> = takes_for_region(&clips, "trk_vox", 0.0, 4.0)
            .into_iter()
            .cloned()
            .collect();
        assert_eq!(takes.len(), 2);

        let lanes = assign_take_lanes(&takes).unwrap();
        assert_eq!(
            lanes,
            vec![
                TakeLane { take_id: "take_vox_p1".to_string(), lane: 0 },
                TakeLane { take_id: "take_vox_p2".to_string(), lane: 1 },
            ]
        );

        let keeper = build_comp(
            "clip_vox_keeper",
            "trk_vox",
            "Vox keeper",
            &[
                CompSection::new("take_vox_p2", 0.0, 2.0),
                CompSection::new("take_vox_p1", 2.0, 4.0),
            ],
            &takes,
        )
        .unwrap();
        assert_eq!(keeper.length_beats, 4.0);
        assert_eq!(keeper.source, "comp:take_vox_p2+take_vox_p1");
    }

    #[test]
    fn auto_punch_gates_on_range_edges() {
        let mode = PunchMode::Auto(validate_punch(8.0, 12.0).unwrap());
        assert!(!is_punching(7.5, &mode, true));
        assert!(is_punching(8.0, &mode, true));
        assert!(is_punching(11.9, &mode, true));
        assert!(!is_punching(12.0, &mode, true));
        assert!(!is_punching(10.0, &mode, false));
        assert!(is_punching(99.0, &PunchMode::Manual, true));
        assert!(!is_punching(99.0, &PunchMode::Manual, false));
    }

    #[test]
    fn count_in_clicks_accent_bar_starts() {
        let clicks = count_in_clicks(&CountIn { bars: 1, beats_per_bar: 4 }).unwrap();
        assert_eq!(clicks.len(), 4);
        assert_eq!(clicks[0], CountInClick { offset_beats: -4.0, accent: true });
        assert_eq!(clicks[3], CountInClick { offset_beats: -1.0, accent: false });
        let secs = count_in_seconds(&CountIn { bars: 1, beats_per_bar: 4 }, 120.0).unwrap();
        assert!((secs - 2.0).abs() < 1e-12);
        assert!(count_in_clicks(&CountIn { bars: 0, beats_per_bar: 4 }).is_err());
    }

    #[test]
    fn monitor_auto_mutes_input_during_playback() {
        assert!(should_monitor(&MonitorMode::Auto, true, &EngineState::Stopped));
        assert!(should_monitor(&MonitorMode::Auto, true, &EngineState::Recording));
        assert!(!should_monitor(&MonitorMode::Auto, true, &EngineState::Playing));
        assert!(!should_monitor(&MonitorMode::Auto, false, &EngineState::Recording));
        assert!(should_monitor(&MonitorMode::On, true, &EngineState::Playing));
        assert!(!should_monitor(&MonitorMode::Off, true, &EngineState::Recording));
    }

    #[test]
    fn latency_compensation_round_trip() {
        // 256 in + 256 out samples @ 48kHz, 120bpm: 512/48000*2 beats.
        let beats = latency_beats(256, 256, 48_000.0, 120.0).unwrap();
        assert!((beats - 512.0 / 48_000.0 * 2.0).abs() < 1e-12);
        assert_eq!(compensate_capture(10.0 + beats, beats), 10.0);
        assert!(latency_beats(0, 0, 0.0, 120.0).is_err());
        assert!(latency_beats(0, 0, 48_000.0, 0.0).is_err());
    }

    #[test]
    fn punch_overlap_trims_manual_passes() {
        let window = validate_punch(8.0, 12.0).unwrap();
        assert_eq!(punch_overlap(6.0, 14.0, &window), Some((8.0, 12.0)));
        assert_eq!(punch_overlap(9.0, 11.0, &window), Some((9.0, 11.0)));
        assert_eq!(punch_overlap(0.0, 8.0, &window), None);
        assert_eq!(punch_overlap(12.0, 16.0, &window), None);
        assert!(validate_punch(4.0, 4.0).is_err());
        assert!(validate_punch(-1.0, 4.0).is_err());
        assert!(assign_take_lanes(&[]).is_err());
    }
}
