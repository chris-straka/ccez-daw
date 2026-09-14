//! Video scoring engine: picture against the beat grid.
//!
//! Teaching note: scoring to picture is a *clock-mapping* problem, not a
//! playback problem. The DAW owns one transport (beats at a tempo); the
//! video file owns another (frames at a frame rate); the audio hardware owns
//! a third (samples at a sample rate). This module maps between the three
//! without owning any of them:
//!
//! 1. **The model is a sidecar.** [`VideoTrack`] pins one media file to the
//!    timeline by `start_beats` + `offset_beats` (slip), exactly like the
//!    UI-local `VideoClip` in `ui/src/video/model.ts`, plus engine concerns
//!    the UI cannot express: a rational [`FrameRate`] (so 29.97 is really
//!    30000/1001, never the float 29.97), a [`Timecode`] house address for
//!    media time 0, and [`VideoTrack::follow_transport`]. [`VideoDoc`]
//!    serializes to JSON beside the project (e.g. `video.json`), the same
//!    sidecar pattern as [`crate::timeline::TimelineDoc`] — the frozen
//!    project schema, IPC table, and typegen output are untouched.
//! 2. **SMPTE timecode is exact integer math.** [`timecode_from_frame`] /
//!    [`timecode_to_frame`] round-trip on frame counts, including
//!    drop-frame 29.97/59.94 (frame numbers 00/01 are skipped at the start
//!    of every minute except every tenth — see the function docs).
//! 3. **Drift is measured in samples, not floats.**
//!    [`drift_in_samples`] computes `video_frame - audio_position` with
//!    `i128` rational arithmetic, so an hour-long 48 kHz / 29.97 df session
//!    reports exactly what the clocks say. [`DriftMonitor`] turns that
//!    number into a decision ([`DriftAction::InSync`] vs `Resync`).
//! 4. **Thumbnails are a background job that cannot fail.**
//!    [`spawn_strip_job`] extracts a filmstrip with `ffmpeg` when one is
//!    usable and returns a [`ThumbStrip::placeholder`] strip otherwise —
//!    missing binary, missing file, per-frame errors all degrade to
//!    placeholders, never to an `Err`.
//!
//! Out of scope (like surround/Atmos and hardware surfaces): decoding video
//! in-process, genlock, and any Tauri/IPC wiring. The `<video>` element
//! renders picture; the engine renders audio; this module is the ruler.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

/// Rational frame rate: `num / den` frames per second.
///
/// Rational on purpose: NTSC rates are exact fractions (30000/1001), and the
/// `f64` 29.97 drifts ~3.6 ms/hour against the real clock — audible lip-sync
/// drift over a feature. [`FrameRate::fps`] is provided for display only;
//  clock math uses [`drift_in_samples`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRate {
    pub num: u32,
    pub den: u32,
}

impl FrameRate {
    pub const R24: Self = Self { num: 24, den: 1 };
    pub const R25: Self = Self { num: 25, den: 1 };
    pub const R30: Self = Self { num: 30, den: 1 };
    pub const R48: Self = Self { num: 48, den: 1 };
    pub const R50: Self = Self { num: 50, den: 1 };
    pub const R60: Self = Self { num: 60, den: 1 };
    /// 23.976 fps film-over-NTSC.
    pub const R23976: Self = Self {
        num: 24000,
        den: 1001,
    };
    /// 29.97 fps drop-frame NTSC.
    pub const R2997: Self = Self {
        num: 30000,
        den: 1001,
    };
    /// 59.94 fps drop-frame NTSC.
    pub const R5994: Self = Self {
        num: 60000,
        den: 1001,
    };

    pub fn new(num: u32, den: u32) -> Result<Self> {
        if num == 0 || den == 0 {
            return Err(VideoError::BadRange(format!(
                "frame rate {num}/{den} must have nonzero num and den"
            )));
        }
        if num > 240 * den {
            return Err(VideoError::BadRange(format!(
                "frame rate {num}/{den} exceeds 240 fps"
            )));
        }
        Ok(Self { num, den })
    }

    /// Snap an `f64` fps to the nearest known rate (within 0.1%), else to a
    /// nearby integer (within 0.01), else error. Display/import convenience —
    /// stored rates stay exact.
    pub fn from_fps(fps: f64) -> Result<Self> {
        if !(fps > 0.0 && fps.is_finite() && fps <= 240.0) {
            return Err(VideoError::BadRange(format!(
                "fps {fps} must be finite and in (0, 240]"
            )));
        }
        const KNOWN: [FrameRate; 9] = [
            FrameRate::R24,
            FrameRate::R25,
            FrameRate::R30,
            FrameRate::R48,
            FrameRate::R50,
            FrameRate::R60,
            FrameRate::R23976,
            FrameRate::R2997,
            FrameRate::R5994,
        ];
        if let Some(hit) = KNOWN
            .iter()
            .find(|r| (r.fps() - fps).abs() / r.fps() < 0.001)
        {
            return Ok(*hit);
        }
        let near = fps.round();
        if (near - fps).abs() < 0.01 {
            return Ok(Self {
                num: near as u32,
                den: 1,
            });
        }
        Err(VideoError::UnsupportedRate(format!(
            "fps {fps} matches no known rate"
        )))
    }

    /// Display fps. Never use this for clock math.
    pub fn fps(&self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Nominal whole-number rate used for timecode fields (24/25/30/48/50/60).
    pub fn nominal_fps(&self) -> u32 {
        (self.num as f64 / self.den as f64).round() as u32
    }

    /// Frames dropped per drop-minute, if this rate supports drop-frame
    /// timecode: 2 for 30000/1001, 4 for 60000/1001, else `None`.
    pub fn drop_step(&self) -> Option<i64> {
        match (self.num, self.den) {
            (30000, 1001) => Some(2),
            (60000, 1001) => Some(4),
            _ => None,
        }
    }
}

/// One SMPTE address: `HH:MM:SS:FF`, or `HH:MM:SS;FF` when `drop_frame`.
///
/// Hours are unbounded (no 24-hour wrap): a 25-hour timeline formats as
/// `25:00:00:00`. This deviates from on-wire SMPTE, which wraps — the
/// string here is a DAW address, not a broadcast signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timecode {
    pub hours: u32,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u32,
    pub drop_frame: bool,
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours, self.minutes, self.seconds, sep, self.frames
        )
    }
}

impl Timecode {
    pub fn new(hours: u32, minutes: u8, seconds: u8, frames: u32, drop_frame: bool) -> Self {
        Self {
            hours,
            minutes,
            seconds,
            frames,
            drop_frame,
        }
    }
}

/// Parse `HH:MM:SS:FF` (non-drop) or `HH:MM:SS;FF` (drop). The separator
/// decides drop-frame; `rate` bounds the frame field and — for drop rates —
/// rejects the skipped frame numbers (`:00;00`/`:00;01` at the top of a
/// drop minute). Hours accept any width; minutes/seconds must be 0-59.
pub fn parse_timecode(s: &str, rate: &FrameRate) -> Result<Timecode> {
    let s = s.trim();
    let (head, ff, drop_frame) = match (s.rfind(':'), s.rfind(';')) {
        (Some(ci), Some(si)) if si > ci => (&s[..si], &s[si + 1..], true),
        (Some(ci), _) => (&s[..ci], &s[ci + 1..], false),
        (None, Some(si)) => (&s[..si], &s[si + 1..], true),
        (None, None) => {
            return Err(VideoError::BadTimecode(format!(
                "bad timecode `{s}` (want HH:MM:SS:FF or HH:MM:SS;FF)"
            )))
        }
    };
    let parts: Vec<&str> = head.split(':').collect();
    if parts.len() != 3 {
        return Err(VideoError::BadTimecode(format!(
            "bad timecode `{s}` (want HH:MM:SS:FF or HH:MM:SS;FF)"
        )));
    }
    let parse = |p: &str, what: &str| {
        p.parse::<u32>().map_err(|_| {
            VideoError::BadTimecode(format!("bad timecode `{s}` ({what} `{p}` not a number)"))
        })
    };
    let hh = parse(parts[0], "hours")?;
    let mm = parse(parts[1], "minutes")?;
    let ss = parse(parts[2], "seconds")?;
    let ff = parse(ff, "frames")?;
    if mm >= 60 || ss >= 60 {
        return Err(VideoError::BadTimecode(format!(
            "bad timecode `{s}` (minutes/seconds must be < 60)"
        )));
    }
    let nominal = rate.nominal_fps();
    if ff >= nominal {
        return Err(VideoError::BadTimecode(format!(
            "bad timecode `{s}` (frame {ff} out of range for {}fps nominal)",
            rate.fps()
        )));
    }
    if drop_frame {
        let step = rate.drop_step().ok_or_else(|| {
            VideoError::DropUnsupported(format!(
                "drop-frame timecode needs 30000/1001 or 60000/1001, got {}/{}",
                rate.num, rate.den
            ))
        })?;
        if mm % 10 != 0 && ss == 0 && (ff as i64) < step {
            return Err(VideoError::BadTimecode(format!(
                "bad timecode `{s}` (frame numbers 00..{:02} do not exist at minute {mm:02})",
                step - 1
            )));
        }
        Ok(Timecode::new(hh, mm as u8, ss as u8, ff, true))
    } else {
        Ok(Timecode::new(hh, mm as u8, ss as u8, ff, false))
    }
}

/// Frame count -> SMPTE address at `rate`.
///
/// Non-drop decomposes directly. Drop-frame inverts the forward rule
/// `F = N - step*(tm - tm/10)` (N = nominal count, tm = total minutes) by
/// fixpoint iteration: start from the raw count, add back the skipped
/// labels, repeat until the minute count stops moving (<= 3 rounds — the
/// correction shifts time by seconds, so minutes settle immediately), then
/// decompose. Round-trips with [`timecode_to_frame`] by construction.
pub fn timecode_from_frame(frame: i64, rate: &FrameRate, drop_frame: bool) -> Result<Timecode> {
    if frame < 0 {
        return Err(VideoError::NegativeFrame(frame));
    }
    let nominal = rate.nominal_fps() as i64;
    if !drop_frame {
        let ff = frame % nominal;
        let rest = frame / nominal;
        let ss = rest % 60;
        let rest = rest / 60;
        let mm = rest % 60;
        let hh = rest / 60;
        return Ok(Timecode::new(hh as u32, mm as u8, ss as u8, ff as u32, false));
    }
    let step = rate.drop_step().ok_or_else(|| {
        VideoError::DropUnsupported(format!(
            "drop-frame timecode needs 30000/1001 or 60000/1001, got {}/{}",
            rate.num, rate.den
        ))
    })?;
    let mut nominal_count = frame;
    for _ in 0..4 {
        let tm = nominal_count / (nominal * 60);
        let next = frame + step * (tm - tm / 10);
        if next == nominal_count {
            break;
        }
        nominal_count = next;
    }
    let ff = nominal_count % nominal;
    let rest = nominal_count / nominal;
    let ss = rest % 60;
    let rest = rest / 60;
    let mm = rest % 60;
    let hh = rest / 60;
    Ok(Timecode::new(hh as u32, mm as u8, ss as u8, ff as u32, true))
}

/// SMPTE address -> frame count at `rate`. Inverse of
/// [`timecode_from_frame`]; rejects drop-minute skipped labels (see
/// [`parse_timecode`]).
pub fn timecode_to_frame(tc: &Timecode, rate: &FrameRate) -> Result<i64> {
    let nominal = rate.nominal_fps() as i64;
    if tc.minutes >= 60 || tc.seconds >= 60 || (tc.frames as i64) >= nominal {
        return Err(VideoError::BadTimecode(format!(
            "timecode {tc} out of range for {}fps nominal",
            rate.fps()
        )));
    }
    let nominal_count =
        ((tc.hours as i64 * 3600) + (tc.minutes as i64 * 60) + tc.seconds as i64) * nominal
            + tc.frames as i64;
    if !tc.drop_frame {
        return Ok(nominal_count);
    }
    let step = rate.drop_step().ok_or_else(|| {
        VideoError::DropUnsupported(format!(
            "drop-frame timecode needs 30000/1001 or 60000/1001, got {}/{}",
            rate.num, rate.den
        ))
    })?;
    let tm = tc.hours as i64 * 60 + tc.minutes as i64;
    if tc.minutes % 10 != 0 && tc.seconds == 0 && (tc.frames as i64) < step {
        return Err(VideoError::BadTimecode(format!(
            "timecode {tc} names a dropped frame number"
        )));
    }
    Ok(nominal_count - step * (tm - tm / 10))
}

/// Media seconds of a frame count (exact rational, as `f64`).
pub fn frame_to_seconds(frame: i64, rate: &FrameRate) -> f64 {
    frame as f64 * rate.den as f64 / rate.num as f64
}

/// Frame on screen at media time `t`: `floor(t * fps)`, clamped at 0.
pub fn frame_from_seconds(t_seconds: f64, rate: &FrameRate) -> i64 {
    if !(t_seconds > 0.0) {
        return 0;
    }
    (t_seconds * rate.num as f64 / rate.den as f64).floor() as i64
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoError {
    EmptyId,
    EmptyFile,
    BadRange(String),
    BadTempo(f64),
    UnsupportedRate(String),
    DropUnsupported(String),
    BadTimecode(String),
    NegativeFrame(i64),
    UnknownTrack(String),
    DuplicateTrack(String),
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => write!(f, "video track id must be non-empty"),
            Self::EmptyFile => write!(f, "video track file reference must be non-empty"),
            Self::BadRange(m) => write!(f, "bad video range: {m}"),
            Self::BadTempo(t) => write!(f, "tempo {t} must be finite and > 0"),
            Self::UnsupportedRate(m) => write!(f, "unsupported frame rate: {m}"),
            Self::DropUnsupported(m) => write!(f, "drop-frame unsupported: {m}"),
            Self::BadTimecode(m) => write!(f, "bad timecode: {m}"),
            Self::NegativeFrame(n) => write!(f, "frame count {n} must be >= 0"),
            Self::UnknownTrack(s) => write!(f, "unknown video track `{s}`"),
            Self::DuplicateTrack(s) => write!(f, "duplicate video track `{s}`"),
        }
    }
}

impl std::error::Error for VideoError {}

pub type Result<T> = std::result::Result<T, VideoError>;

/// Beats -> seconds at one tempo (mirrors `timeline::beats_to_seconds`, but
/// loud: non-positive or non-finite tempos are errors, never silent NaN).
pub fn beats_to_seconds(beats: f64, tempo: f64) -> Result<f64> {
    if !(tempo > 0.0 && tempo.is_finite()) {
        return Err(VideoError::BadTempo(tempo));
    }
    Ok(beats * 60.0 / tempo)
}

/// Seconds -> beats at one tempo.
pub fn seconds_to_beats(seconds: f64, tempo: f64) -> Result<f64> {
    if !(tempo > 0.0 && tempo.is_finite()) {
        return Err(VideoError::BadTempo(tempo));
    }
    Ok(seconds * tempo / 60.0)
}

/// One video track: a media file pinned to the beat grid.
///
/// - `file` is an opaque reference (path, URI, or `take:<key>`) — core never
///   resolves or opens it; only the thumbnail job passes it to `ffmpeg`.
/// - `start_beats` is the timeline position of the clip head;
///   `length_beats` its audible span; `offset_beats` is slip (+ shifts
///   picture later, transport leads), matching `ui/src/video/model.ts`.
/// - `start_timecode` is the SMPTE house address of media time 0, so a
///   transport position maps to a *broadcast* address, not just seconds.
/// - `follow_transport`: true = picture chases the DAW transport (scrub,
///   loop, and cycle all re-seek); false = picture free-runs on its own
///   clock from the frame showing at play-start (see
///   [`media_seconds_free_run`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoTrack {
    pub id: String,
    pub name: String,
    pub file: String,
    pub frame_rate: FrameRate,
    pub drop_frame: bool,
    pub start_beats: f64,
    pub length_beats: f64,
    pub offset_beats: f64,
    pub start_timecode: Timecode,
    pub follow_transport: bool,
}

impl VideoTrack {
    /// Range-check one track. The renderer never silently reinterprets:
    /// empty ids/files, negative starts, non-positive lengths, non-finite
    /// offsets, drop-frame on a non-NTSC rate, and a `start_timecode`
    /// disagreeing with `drop_frame` are all errors.
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() {
            return Err(VideoError::EmptyId);
        }
        if self.file.is_empty() {
            return Err(VideoError::EmptyFile);
        }
        if !(self.start_beats.is_finite() && self.start_beats >= 0.0) {
            return Err(VideoError::BadRange(format!(
                "start_beats {} must be finite and >= 0",
                self.start_beats
            )));
        }
        if !(self.length_beats.is_finite() && self.length_beats > 0.0) {
            return Err(VideoError::BadRange(format!(
                "length_beats {} must be finite and > 0",
                self.length_beats
            )));
        }
        if !self.offset_beats.is_finite() {
            return Err(VideoError::BadRange(format!(
                "offset_beats {} must be finite",
                self.offset_beats
            )));
        }
        if self.drop_frame && self.frame_rate.drop_step().is_none() {
            return Err(VideoError::DropUnsupported(format!(
                "track `{}`: drop-frame needs 30000/1001 or 60000/1001, got {}/{}",
                self.id, self.frame_rate.num, self.frame_rate.den
            )));
        }
        if self.start_timecode.drop_frame != self.drop_frame {
            return Err(VideoError::BadRange(format!(
                "track `{}`: start_timecode drop flag ({}) disagrees with track ({})",
                self.id, self.start_timecode.drop_frame, self.drop_frame
            )));
        }
        // Frame field must fit the rate; reuses the range check.
        timecode_to_frame(&self.start_timecode, &self.frame_rate)?;
        Ok(())
    }

    /// End of the track in timeline beats.
    pub fn end_beats(&self) -> f64 {
        self.start_beats + self.length_beats
    }

    /// Frame number of [`VideoTrack::start_timecode`] (media time 0).
    pub fn start_frame(&self) -> Result<i64> {
        timecode_to_frame(&self.start_timecode, &self.frame_rate)
    }
}

/// The video sidecar: tracks pinned to the timeline, stored beside the
/// project (e.g. `video.json`), never inside it. No frozen schema, no IPC —
/// like [`crate::timeline::TimelineDoc`], so the typegen drift gate is
/// unaffected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoDoc {
    pub tracks: Vec<VideoTrack>,
}

impl VideoDoc {
    pub fn empty() -> Self {
        Self { tracks: Vec::new() }
    }

    pub fn track(&self, id: &str) -> Option<&VideoTrack> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Insert a track (validated; rejects duplicate ids).
    pub fn add_track(&mut self, track: VideoTrack) -> Result<()> {
        track.validate()?;
        if self.tracks.iter().any(|t| t.id == track.id) {
            return Err(VideoError::DuplicateTrack(track.id));
        }
        self.tracks.push(track);
        Ok(())
    }

    /// Remove a track by id, returning it. Unknown ids are errors.
    pub fn remove_track(&mut self, id: &str) -> Result<VideoTrack> {
        let pos = self
            .tracks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| VideoError::UnknownTrack(id.to_string()))?;
        Ok(self.tracks.remove(pos))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// Track covering `transport_beats` (`start <= b < end`). Latest start wins
/// when tracks overlap — mirrors the UI `activeClip` rule so engine and
/// preview agree on which picture is up.
pub fn active_track(doc: &VideoDoc, transport_beats: f64) -> Option<&VideoTrack> {
    let mut hit: Option<&VideoTrack> = None;
    for track in &doc.tracks {
        if transport_beats >= track.start_beats && transport_beats < track.end_beats() {
            match hit {
                Some(prev) if prev.start_beats > track.start_beats => {}
                _ => hit = Some(track),
            }
        }
    }
    hit
}

/// Media time (seconds into the file) the preview should show for a
/// transport position. `None` when the position is before the clip head or
/// the slip pushed picture before media 0 (hold first frame) — mirrors the
/// UI `videoTimeForTransport` null cases. Not clamped to duration: the
/// preview holds its last frame.
pub fn media_seconds_for_transport(
    track: &VideoTrack,
    transport_beats: f64,
    tempo: f64,
) -> Result<Option<f64>> {
    if transport_beats < track.start_beats {
        return Ok(None);
    }
    let media_beats = transport_beats - track.start_beats - track.offset_beats;
    if media_beats < 0.0 {
        return Ok(None);
    }
    Ok(Some(beats_to_seconds(media_beats, tempo)?))
}

/// SMPTE address showing for a transport position (`None` = no picture).
/// Media time 0 carries [`VideoTrack::start_timecode`]; later positions
/// advance by whole media frames.
pub fn timecode_for_transport(
    track: &VideoTrack,
    transport_beats: f64,
    tempo: f64,
) -> Result<Option<Timecode>> {
    let Some(media) = media_seconds_for_transport(track, transport_beats, tempo)? else {
        return Ok(None);
    };
    let frame = track.start_frame()? + frame_from_seconds(media, &track.frame_rate);
    Ok(Some(timecode_from_frame(
        frame,
        &track.frame_rate,
        track.drop_frame,
    )?))
}

/// Free-run picture: media seconds after `elapsed_wall_seconds` of wall-clock
/// playback started at `play_transport_beats`. Base is the media time
/// showing at play-start (0.0 when the start position has no picture —
/// transport started outside the clip). Only meaningful when
/// `follow_transport` is false; the caller advances `elapsed_wall_seconds`
/// on the video clock while audio advances on the sample clock, and
/// [`DriftMonitor`] watches the gap.
pub fn media_seconds_free_run(
    track: &VideoTrack,
    play_transport_beats: f64,
    tempo: f64,
    elapsed_wall_seconds: f64,
) -> Result<f64> {
    let base = media_seconds_for_transport(track, play_transport_beats, tempo)?.unwrap_or(0.0);
    Ok((base + elapsed_wall_seconds).max(0.0))
}

// ---------------------------------------------------------------------------
// Sample-accurate video-clock vs audio-clock drift.
// ---------------------------------------------------------------------------

/// Audio position of `samples` ticks at `sample_rate` Hz, in seconds.
/// `i64` samples are exact in `f64` well past any session length.
pub fn audio_time_seconds(samples: i64, sample_rate: u32) -> f64 {
    samples as f64 / sample_rate as f64
}

/// Which video frame should be on screen for an audio position: the
/// greatest frame whose start time is at or before the audio instant.
/// Exact `i128` floor division — no float involved.
pub fn expected_video_frame(audio_samples: i64, sample_rate: u32, rate: &FrameRate) -> i64 {
    let n = audio_samples as i128 * rate.num as i128;
    let d = rate.den as i128 * sample_rate as i128;
    n.div_euclid(d).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Audio sample tick of a frame's start, rounded to nearest (half up).
/// Inverse of [`expected_video_frame`] up to the in-frame remainder.
pub fn audio_samples_for_frame(frame: i64, sample_rate: u32, rate: &FrameRate) -> i64 {
    let n = frame as i128 * rate.den as i128 * sample_rate as i128;
    let d = rate.num as i128;
    // Frames are non-negative, so truncating division rounds half up.
    ((n + d / 2) / d).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Signed drift `video - audio` in samples: how far the picture clock sits
/// ahead of (+) or behind (-) the sample clock. Computed as
/// `round(frame*den*sr/num) - samples` in `i128`, so a 1-hour 48 kHz /
/// 29.97 df session (frame counts ~10^5, products ~10^12, far below the
/// `f64` 2^53 integer cliff — and no float is used at all) reports exactly
/// what the two clocks say.
pub fn drift_in_samples(
    audio_samples: i64,
    sample_rate: u32,
    video_frame: i64,
    rate: &FrameRate,
) -> i64 {
    audio_samples_for_frame(video_frame, sample_rate, rate)
        .wrapping_sub(audio_samples)
}

/// Drift in seconds (display; control decisions use [`drift_in_samples`]).
pub fn drift_seconds(
    audio_samples: i64,
    sample_rate: u32,
    video_frame: i64,
    rate: &FrameRate,
) -> f64 {
    drift_in_samples(audio_samples, sample_rate, video_frame, rate) as f64 / sample_rate as f64
}

/// What the engine should do about the measured drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftAction {
    /// Within tolerance: keep playing, carry the residual.
    InSync { drift_samples: i64 },
    /// Past tolerance: re-seek picture by `skip_samples` (signed: + means
    /// picture is ahead, hold/skip forward the audio-side expectation).
    Resync { drift_samples: i64, skip_samples: i64 },
}

/// Watches one audio-rate / frame-rate pair. The default tolerance is half
/// a video frame in samples (the smallest visible error: any smaller drift
/// is sub-frame and unfixable by a frame-granular seek), minimum 1 sample.
#[derive(Debug, Clone, Copy)]
pub struct DriftMonitor {
    pub sample_rate: u32,
    pub frame_rate: FrameRate,
    pub threshold_samples: i64,
}

impl DriftMonitor {
    pub fn new(sample_rate: u32, frame_rate: FrameRate) -> Result<Self> {
        if sample_rate == 0 {
            return Err(VideoError::BadRange("sample_rate must be nonzero".into()));
        }
        let half_frame =
            (frame_rate.den as i64 * sample_rate as i64 + frame_rate.num as i64 - 1)
                / (2 * frame_rate.num as i64);
        Ok(Self {
            sample_rate,
            frame_rate,
            threshold_samples: half_frame.max(1),
        })
    }

    pub fn with_threshold(mut self, threshold_samples: i64) -> Self {
        self.threshold_samples = threshold_samples.max(0);
        self
    }

    /// Classify one joint observation of the two clocks.
    pub fn observe(&self, audio_samples: i64, video_frame: i64) -> DriftAction {
        let drift = drift_in_samples(audio_samples, self.sample_rate, video_frame, &self.frame_rate);
        if drift.abs() <= self.threshold_samples {
            DriftAction::InSync {
                drift_samples: drift,
            }
        } else {
            DriftAction::Resync {
                drift_samples: drift,
                skip_samples: drift,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Thumbnail strip extraction as a background job (sidecar pattern).
// ---------------------------------------------------------------------------

/// Which tool produced a [`ThumbStrip`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StripTool {
    /// `ffmpeg` at this binary path.
    Ffmpeg(String),
    /// `ffmpeg` decoded, `tiny-codec` encoded the JPEGs (binary path shown).
    TinyCodec(String),
    /// No usable `ffmpeg`: every frame is a layout placeholder.
    Placeholder(String),
}

/// Who JPEG-encodes strip frames. `ffmpeg` writes the `.jpg` itself;
/// [`StripJpeg::TinyCodec`] decodes a raw frame with `ffmpeg` and encodes
/// the JPEG with `tiny-codec` (q72, 4:2:0, optimized Huffman) instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StripJpeg {
    /// `ffmpeg -q:v 4` writes the JPEG (default, historical behavior).
    #[default]
    Ffmpeg,
    /// Raw frame via `ffmpeg`, JPEG via `tiny-codec`.
    TinyCodec,
}

/// One filmstrip cell: a JPEG on disk, or a placeholder the lane renders as
/// an empty box (same slot, same timestamp — the layout never shifts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThumbFrame {
    pub index: usize,
    pub at_seconds: f64,
    pub path: Option<String>,
    pub placeholder: bool,
}

/// A filmstrip for one media file: `frames` in `at_seconds` order.
/// Serializes beside `video.json` (e.g. `thumbs/<track>.json`) so the lane
/// paints instantly on reopen and re-extracts only on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThumbStrip {
    pub file: String,
    pub frames: Vec<ThumbFrame>,
    pub tool: StripTool,
}

impl ThumbStrip {
    /// All-placeholder strip: same slots, no pixels.
    pub fn placeholder(file: &str, positions: &[f64], reason: &str) -> Self {
        Self {
            file: file.to_string(),
            frames: positions
                .iter()
                .enumerate()
                .map(|(index, at_seconds)| ThumbFrame {
                    index,
                    at_seconds: *at_seconds,
                    path: None,
                    placeholder: true,
                })
                .collect(),
            tool: StripTool::Placeholder(reason.to_string()),
        }
    }

    pub fn is_placeholder(&self) -> bool {
        matches!(self.tool, StripTool::Placeholder(_))
    }
}

/// What to extract. `count == 0` means the default (8). Positions default
/// to slice centers over `duration_hint_seconds` (default 60 s).
#[derive(Debug, Clone)]
pub struct ThumbStripRequest {
    pub file: String,
    pub count: usize,
    pub width_px: u32,
    pub positions_seconds: Option<Vec<f64>>,
    pub duration_hint_seconds: Option<f64>,
    /// `ffmpeg` binary: explicit path > `$CCEZ_VIDEO_FFMPEG` > `PATH`.
    /// `None` = auto-detect. An unusable value degrades to placeholders.
    pub ffmpeg_bin: Option<String>,
    /// Who JPEG-encodes the frames. Default [`StripJpeg::Ffmpeg`].
    pub jpeg: StripJpeg,
    /// Where to write JPEGs. Default: a fresh `ccez-video-thumbs-*` temp dir.
    pub out_dir: Option<PathBuf>,
}

impl ThumbStripRequest {
    pub fn new(file: &str, count: usize) -> Self {
        Self {
            file: file.to_string(),
            count,
            width_px: 160,
            positions_seconds: None,
            duration_hint_seconds: None,
            ffmpeg_bin: None,
            jpeg: StripJpeg::Ffmpeg,
            out_dir: None,
        }
    }

    fn effective_count(&self) -> usize {
        if self.count == 0 {
            8
        } else {
            self.count.clamp(1, 64)
        }
    }

    fn positions(&self) -> Vec<f64> {
        if let Some(pos) = &self.positions_seconds {
            if !pos.is_empty() {
                let mut p: Vec<f64> = pos.iter().map(|t| t.max(0.0)).collect();
                p.truncate(64);
                return p;
            }
        }
        let n = self.effective_count();
        let dur = self.duration_hint_seconds.unwrap_or(60.0);
        let dur = if dur.is_finite() && dur > 0.0 { dur } else { 60.0 };
        (0..n).map(|i| dur * (i as f64 + 0.5) / n as f64).collect()
    }
}

static STRIP_JOB_SEQ: AtomicU64 = AtomicU64::new(1);

fn ffmpeg_usable(bin: &str) -> bool {
    Command::new(bin)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve the `ffmpeg` binary (explicit > env > `PATH`); `None` means
/// "degrade to placeholders". Never errors — absence is a normal state
/// (CI runners, minimal installs), not a failure.
pub fn resolve_ffmpeg(explicit: Option<&str>) -> Option<String> {
    if let Some(bin) = explicit {
        if !bin.is_empty() && ffmpeg_usable(bin) {
            return Some(bin.to_string());
        }
        return None;
    }
    if let Ok(env) = std::env::var("CCEZ_VIDEO_FFMPEG") {
        if !env.is_empty() {
            if ffmpeg_usable(&env) {
                return Some(env);
            }
            return None;
        }
    }
    if ffmpeg_usable("ffmpeg") {
        return Some("ffmpeg".to_string());
    }
    None
}

fn strip_out_dir(req: &ThumbStripRequest) -> Option<PathBuf> {
    if let Some(dir) = &req.out_dir {
        if std::fs::create_dir_all(dir).is_ok() {
            return Some(dir.clone());
        }
        return None;
    }
    let dir = std::env::temp_dir().join(format!(
        "ccez-video-thumbs-{}-{}",
        std::process::id(),
        STRIP_JOB_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::create_dir_all(&dir).is_ok() {
        Some(dir)
    } else {
        None
    }
}

/// Encode one strip frame with `tiny-codec`: q72, 4:2:0, optimized Huffman
/// (measured 7–53% smaller than standard tables on stills; thumbnails sit
/// at the high end of that range). `None` on bad input — never panics, so
/// the strip job keeps its always-usable contract.
pub fn thumb_jpeg_tiny_codec(w: u32, h: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || rgb.len() != w as usize * h as usize * 3 {
        return None;
    }
    Some(tiny_jfif::mux::encode_rgb_full(
        w,
        h,
        rgb,
        72,
        tiny_jfif::mux::Subsampling::Yuv420,
        tiny_jfif::mux::HuffmanMode::Optimized,
    ))
}

/// Extract one frame with `ffmpeg` as raw RGB24 on stdout, then JPEG-encode
/// it with [`thumb_jpeg_tiny_codec`]. Height is inferred from the byte
/// count (`scale=W:-2` fixes an even height on the decode side). Any
/// failure is `false` (placeholder cell).
fn grab_frame_tiny(ffmpeg: &str, file: &str, at_seconds: f64, width_px: u32, out: &Path) -> bool {
    let w = width_px.clamp(32, 640);
    let raw = Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-ss",
            &format!("{:.3}", at_seconds.max(0.0)),
            "-i",
            file,
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={w}:-2"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .output();
    let Ok(raw) = raw else { return false };
    if !raw.status.success() || raw.stdout.len() % (w as usize * 3) != 0 {
        return false;
    }
    let h = (raw.stdout.len() / (w as usize * 3)) as u32;
    let Some(jpeg) = thumb_jpeg_tiny_codec(w, h, &raw.stdout) else {
        return false;
    };
    std::fs::write(out, jpeg).is_ok() && out.is_file()
}

/// Extract one frame with `ffmpeg`: seek, decode one picture, scale to
/// `width_px` wide (even height). Any failure is `None` (placeholder cell).
fn grab_frame(ffmpeg: &str, file: &str, at_seconds: f64, width_px: u32, out: &Path) -> bool {
    let w = width_px.clamp(32, 640);
    let status = Command::new(ffmpeg)
        .args([
            "-y",
            "-v",
            "error",
            "-ss",
            &format!("{:.3}", at_seconds.max(0.0)),
            "-i",
            file,
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={w}:-2"),
            "-q:v",
            "4",
        ])
        .arg(out)
        .output();
    match status {
        Ok(o) => o.status.success() && out.is_file(),
        Err(_) => false,
    }
}

/// Blocking strip extraction. Best-effort by contract: missing `ffmpeg`,
/// missing file, unwritable output, or per-frame decode failures all yield
/// placeholder cells — the return is always a usable [`ThumbStrip`].
pub fn extract_strip(req: &ThumbStripRequest) -> ThumbStrip {
    let positions = req.positions();
    let Some(ffmpeg) = resolve_ffmpeg(req.ffmpeg_bin.as_deref()) else {
        return ThumbStrip::placeholder(&req.file, &positions, "no usable ffmpeg on PATH");
    };
    if !Path::new(&req.file).is_file() {
        return ThumbStrip::placeholder(&req.file, &positions, "media file not found");
    }
    let Some(dir) = strip_out_dir(req) else {
        return ThumbStrip::placeholder(&req.file, &positions, "thumbnail dir not writable");
    };
    let stem = Path::new(&req.file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("thumb");
    let mut frames = Vec::with_capacity(positions.len());
    let mut any_real = false;
    for (index, at_seconds) in positions.iter().enumerate() {
        let out = dir.join(format!("{stem}_{index:03}.jpg"));
        let grabbed = match req.jpeg {
            StripJpeg::Ffmpeg => grab_frame(&ffmpeg, &req.file, *at_seconds, req.width_px, &out),
            StripJpeg::TinyCodec => {
                grab_frame_tiny(&ffmpeg, &req.file, *at_seconds, req.width_px, &out)
            }
        };
        if grabbed {
            any_real = true;
            frames.push(ThumbFrame {
                index,
                at_seconds: *at_seconds,
                path: Some(out.to_string_lossy().into_owned()),
                placeholder: false,
            });
        } else {
            frames.push(ThumbFrame {
                index,
                at_seconds: *at_seconds,
                path: None,
                placeholder: true,
            });
        }
    }
    ThumbStrip {
        file: req.file.clone(),
        frames,
        tool: if any_real {
            match req.jpeg {
                StripJpeg::Ffmpeg => StripTool::Ffmpeg(ffmpeg),
                StripJpeg::TinyCodec => StripTool::TinyCodec(ffmpeg),
            }
        } else {
            StripTool::Placeholder("ffmpeg decoded no frames".into())
        },
    }
}

/// Background filmstrip job: [`extract_strip`] on a worker thread. The
/// caller keeps the [`JoinHandle`] and collects the [`ThumbStrip`] when
/// convenient; transport never blocks on it.
pub fn spawn_strip_job(req: ThumbStripRequest) -> JoinHandle<ThumbStrip> {
    std::thread::spawn(move || extract_strip(&req))
}

/// Demo doc: a 29.97 df title card from beat 0 and a 24 fps scene from 16,
/// both following transport. `vid_scene` is the drift/transport validation
/// target below.
pub fn sample_video_doc() -> VideoDoc {
    VideoDoc {
        tracks: vec![
            VideoTrack {
                id: "vid_title".to_string(),
                name: "Title card".to_string(),
                file: "take:vid_title".to_string(),
                frame_rate: FrameRate::R2997,
                drop_frame: true,
                start_beats: 0.0,
                length_beats: 8.0,
                offset_beats: 0.0,
                start_timecode: Timecode::new(0, 0, 0, 0, true),
                follow_transport: true,
            },
            VideoTrack {
                id: "vid_scene".to_string(),
                name: "Scene 1".to_string(),
                file: "take:vid_scene".to_string(),
                frame_rate: FrameRate::R24,
                drop_frame: false,
                start_beats: 16.0,
                length_beats: 16.0,
                offset_beats: 0.0,
                start_timecode: Timecode::new(1, 0, 0, 0, false),
                follow_transport: true,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ccez-video-{name}-{}", std::process::id()))
    }

    #[test]
    fn timecode_round_trip_non_drop() {
        // Every supported non-drop rate, across minute/hour boundaries.
        let rates = [
            FrameRate::R24,
            FrameRate::R25,
            FrameRate::R30,
            FrameRate::R48,
            FrameRate::R50,
            FrameRate::R60,
            FrameRate::R23976,
        ];
        for rate in rates {
            let nominal = rate.nominal_fps() as i64;
            let mut frames: Vec<i64> = (0..5000).step_by(7).collect();
            // Minute + hour boundaries in nominal counts.
            frames.extend([
                nominal * 60 - 2,
                nominal * 60 - 1,
                nominal * 60,
                nominal * 60 + 1,
                nominal * 3600 - 1,
                nominal * 3600,
                nominal * 3600 + 25,
                10 * nominal * 3600 + 3,
            ]);
            for f in frames {
                let tc = timecode_from_frame(f, &rate, false).unwrap();
                assert_eq!(tc.drop_frame, false, "frame {f} at {}/{}", rate.num, rate.den);
                assert_eq!(timecode_to_frame(&tc, &rate).unwrap(), f);
                // String round-trip too.
                let reparsed = parse_timecode(&tc.to_string(), &rate).unwrap();
                assert_eq!(reparsed, tc, "string round-trip of {tc}");
            }
        }
    }

    #[test]
    fn timecode_drop_frame_round_trip_2997() {
        let rate = FrameRate::R2997;
        // Wide sweep over the first 12 minutes (covers drop + non-drop
        // minutes) plus hourly anchors, every 13th frame.
        let mut frames: Vec<i64> = (0..22000).step_by(13).collect();
        frames.extend([17980, 17981, 17982, 17983, 215_999, 216_000, 2_589_408]);
        for f in frames {
            let tc = timecode_from_frame(f, &rate, true).unwrap();
            assert_eq!(
                timecode_to_frame(&tc, &rate).unwrap(),
                f,
                "drop-frame round-trip of frame {f} ({tc})"
            );
            let reparsed = parse_timecode(&tc.to_string(), &rate).unwrap();
            assert_eq!(reparsed, tc);
        }
        // Known anchors: 10 minutes of 29.97 df is 17982 frames, not 18000.
        assert_eq!(
            timecode_to_frame(&parse_timecode("00:10:00;00", &rate).unwrap(), &rate).unwrap(),
            17982
        );
        // First valid label of a drop minute skips ;00 and ;01.
        assert_eq!(
            timecode_to_frame(&parse_timecode("00:01:00;02", &rate).unwrap(), &rate).unwrap(),
            1800
        );
        assert_eq!(timecode_from_frame(1800, &rate, true).unwrap().to_string(), "00:01:00;02");
        // Skipped labels are errors, not silent reinterpretations.
        assert!(parse_timecode("00:01:00;00", &rate).is_err());
        assert!(parse_timecode("00:01:00;01", &rate).is_err());
        // Drop separator on a non-drop rate is an error.
        assert!(parse_timecode("00:01:00;02", &FrameRate::R24).is_err());
        // Non-drop separator formats with ':'.
        assert_eq!(
            timecode_from_frame(1800, &FrameRate::R30, false).unwrap().to_string(),
            "00:01:00:00"
        );
    }

    #[test]
    fn timecode_drop_frame_round_trip_5994() {
        let rate = FrameRate::R5994;
        let frames: Vec<i64> = (0..44000).step_by(29).collect();
        for f in frames {
            let tc = timecode_from_frame(f, &rate, true).unwrap();
            assert_eq!(timecode_to_frame(&tc, &rate).unwrap(), f, "59.94 df frame {f}");
        }
        // 10 minutes at 59.94 df: 36000 - 4*9 = 35964.
        assert_eq!(
            timecode_to_frame(&parse_timecode("00:10:00;00", &rate).unwrap(), &rate).unwrap(),
            35964
        );
    }

    #[test]
    fn drift_stays_sample_accurate_for_an_hour() {
        // The required drift test: 1 hour of 48 kHz audio against 29.97 df
        // picture. The exact frame for the hour mark must report ~zero
        // drift, and a one-frame error must report ~one frame in samples.
        let sr = 48_000u32;
        let rate = FrameRate::R2997;
        let hour_samples = 3600i64 * sr as i64;
        let frame = expected_video_frame(hour_samples, sr, &rate);
        // Sanity: 3600 s * 30000/1001 = 107892.1... -> frame 107892.
        assert_eq!(frame, 107_892);
        let drift = drift_in_samples(hour_samples, sr, frame, &rate);
        assert!(
            drift.abs() <= 801,
            "exact frame must be within half a frame (801 samples), got {drift}"
        );
        // Round-trip: the frame's own start tick must read drift 0. The
        // tick is rounded to a whole sample, so it can land a fraction of
        // a sample before the true frame boundary — the frame it names is
        // then off by at most one.
        let tick = audio_samples_for_frame(frame, sr, &rate);
        assert_eq!(drift_in_samples(tick, sr, frame, &rate), 0);
        assert!(
            (expected_video_frame(tick, sr, &rate) - frame).abs() <= 1,
            "rounded tick must name frame {frame} ± 1"
        );
        // One frame off = one frame of drift (~1601.6 samples at 29.97).
        let one_frame = drift_in_samples(tick, sr, frame + 1, &rate);
        assert_eq!(one_frame, 1602, "one-frame drift, got {one_frame}");
        // Monitor: exact is InSync; a 5-frame slip is a Resync.
        let mon = DriftMonitor::new(sr, rate).unwrap();
        assert_eq!(mon.threshold_samples, 801);
        assert!(matches!(
            mon.observe(tick, frame),
            DriftAction::InSync { drift_samples: 0 }
        ));
        match mon.observe(tick, frame + 5) {
            DriftAction::Resync { drift_samples, skip_samples } => {
                assert_eq!(drift_samples, skip_samples);
                assert!(drift_samples > 5 * 1600);
            }
            other => panic!("expected Resync, got {other:?}"),
        }
        // 24 fps film @ 48 kHz is sample-exact: 2000 samples/frame, drift 0.
        let film = FrameRate::R24;
        let f = expected_video_frame(hour_samples, sr, &film);
        assert_eq!(f, 86_400);
        assert_eq!(drift_in_samples(hour_samples, sr, f, &film), 0);
    }

    #[test]
    fn transport_maps_beats_to_picture_and_timecode() {
        let doc = sample_video_doc();
        let scene = doc.track("vid_scene").unwrap();
        // Beat 16 @ 120 BPM = media 0; house address is 01:00:00:00.
        let tc = timecode_for_transport(scene, 16.0, 120.0).unwrap().unwrap();
        assert_eq!(tc.to_string(), "01:00:00:00");
        // Beat 20 @ 120 BPM = 2 s in = frame 48 @ 24 fps.
        let tc = timecode_for_transport(scene, 20.0, 120.0).unwrap().unwrap();
        assert_eq!(tc.to_string(), "01:00:02:00");
        // Before the head: no picture.
        assert_eq!(timecode_for_transport(scene, 15.9, 120.0).unwrap(), None);
        assert_eq!(media_seconds_for_transport(scene, 15.9, 120.0).unwrap(), None);
        // Slip: +2 beats offset pushes picture later (transport leads).
        let mut slipped = scene.clone();
        slipped.offset_beats = 2.0;
        assert_eq!(media_seconds_for_transport(&slipped, 16.0, 120.0).unwrap(), None);
        assert_eq!(
            media_seconds_for_transport(&slipped, 18.0, 120.0).unwrap(),
            Some(0.0)
        );
        // Bad tempo is loud.
        assert!(media_seconds_for_transport(scene, 20.0, 0.0).is_err());
        // Latest-start overlap wins, mirroring the UI activeClip rule.
        assert_eq!(active_track(&doc, 4.0).unwrap().id, "vid_title");
        assert_eq!(active_track(&doc, 20.0).unwrap().id, "vid_scene");
        assert!(active_track(&doc, 40.0).is_none());
    }

    #[test]
    fn free_run_advances_on_the_wall_clock() {
        let doc = sample_video_doc();
        let scene = doc.track("vid_scene").unwrap();
        // Play from beat 20 @ 120 BPM (media 2.0 s) + 1.5 s wall = 3.5 s.
        let t = media_seconds_free_run(scene, 20.0, 120.0, 1.5).unwrap();
        assert!((t - 3.5).abs() < 1e-9, "got {t}");
        // Play from outside the clip: base 0 + wall.
        let t = media_seconds_free_run(scene, 4.0, 120.0, 1.0).unwrap();
        assert!((t - 1.0).abs() < 1e-9, "got {t}");
    }

    #[test]
    fn track_validation_rejects_bad_shapes() {
        let mut doc = VideoDoc::empty();
        let good = sample_video_doc().tracks[0].clone();
        doc.add_track(good.clone()).unwrap();
        assert!(matches!(
            doc.add_track(good),
            Err(VideoError::DuplicateTrack(_))
        ));
        let mut bad = sample_video_doc().tracks[0].clone();
        bad.id.clear();
        assert_eq!(bad.validate(), Err(VideoError::EmptyId));
        let mut bad = sample_video_doc().tracks[0].clone();
        bad.file.clear();
        assert_eq!(bad.validate(), Err(VideoError::EmptyFile));
        // Drop-frame on 24 fps: unsupported.
        let mut bad = sample_video_doc().tracks[1].clone();
        bad.drop_frame = true;
        assert!(matches!(bad.validate(), Err(VideoError::DropUnsupported(_))));
        // Timecode flag disagreeing with the track flag.
        let mut bad = sample_video_doc().tracks[1].clone();
        bad.start_timecode = Timecode::new(0, 0, 0, 0, true);
        assert!(matches!(bad.validate(), Err(VideoError::BadRange(_))));
        assert!(matches!(
            doc.remove_track("nope"),
            Err(VideoError::UnknownTrack(_))
        ));
        // Sidecar save/load round-trip.
        let dir = test_dir("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("video.json");
        sample_video_doc().save(&path).unwrap();
        assert_eq!(VideoDoc::load(&path).unwrap(), sample_video_doc());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_degrades_to_placeholders_without_ffmpeg() {
        // Forced degrade: a bogus binary path must yield placeholders, not
        // errors — this is the ffmpeg-optional contract, independent of
        // whether the test machine has ffmpeg installed.
        let req = ThumbStripRequest {
            file: "take:vid_scene".to_string(),
            count: 4,
            width_px: 160,
            positions_seconds: None,
            duration_hint_seconds: Some(32.0),
            ffmpeg_bin: Some("/nonexistent/ccez-ffmpeg".to_string()),
            jpeg: StripJpeg::Ffmpeg,
            out_dir: None,
        };
        assert!(resolve_ffmpeg(req.ffmpeg_bin.as_deref()).is_none());
        let strip = extract_strip(&req);
        assert!(strip.is_placeholder());
        assert_eq!(strip.frames.len(), 4);
        assert!(strip.frames.iter().all(|f| f.placeholder && f.path.is_none()));
        // Slice centers over 32 s: 4, 12, 20, 28.
        let ats: Vec<f64> = strip.frames.iter().map(|f| f.at_seconds).collect();
        assert_eq!(ats, vec![4.0, 12.0, 20.0, 28.0]);
        // Same via the background job: transport never blocks on thumbs.
        let handle = spawn_strip_job(req);
        let strip = handle.join().expect("strip job panicked");
        assert!(strip.is_placeholder());
        assert_eq!(strip.frames.len(), 4);
    }

    #[test]
    fn strip_extracts_real_frames_when_ffmpeg_exists() {
        // Uses the real ffmpeg when present; skips (not fails) without it.
        if resolve_ffmpeg(None).is_none() {
            eprintln!("SKIP: no ffmpeg on PATH");
            return;
        }
        // Build a 2 s test pattern clip with the same ffmpeg.
        let dir = test_dir("real");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("pattern.mp4");
        let made = Command::new(resolve_ffmpeg(None).unwrap())
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=128x96:rate=30",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&src)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !made || !src.is_file() {
            eprintln!("SKIP: could not synthesize test clip");
            return;
        }
        let req = ThumbStripRequest {
            file: src.to_string_lossy().into_owned(),
            count: 3,
            width_px: 64,
            positions_seconds: Some(vec![0.2, 1.0, 1.8]),
            duration_hint_seconds: None,
            ffmpeg_bin: None,
            jpeg: StripJpeg::Ffmpeg,
            out_dir: Some(dir.join("thumbs")),
        };
        let strip = extract_strip(&req);
        assert!(!strip.is_placeholder(), "tool: {:?}", strip.tool);
        assert_eq!(strip.frames.len(), 3);
        for f in &strip.frames {
            assert!(!f.placeholder);
            let p = f.path.as_ref().expect("real frame needs a path");
            assert!(Path::new(p).is_file(), "missing {p}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_request_defaults_to_ffmpeg_engine() {
        assert_eq!(ThumbStripRequest::new("f.mp4", 8).jpeg, StripJpeg::Ffmpeg);
        assert_eq!(StripJpeg::default(), StripJpeg::Ffmpeg);
    }

    fn synthetic_thumb_rgb(w: u32, h: u32) -> Vec<u8> {
        let mut rgb = vec![0u8; w as usize * h as usize * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 3;
                rgb[i] = (x * 255 / (w - 1).max(1)) as u8;
                rgb[i + 1] = (y * 255 / (h - 1).max(1)) as u8;
                rgb[i + 2] = 128;
            }
        }
        rgb
    }

    #[test]
    fn tiny_codec_thumb_encodes_synthetic_rgb() {
        // No ffmpeg needed: the tiny-codec half of the seam stands alone.
        let (w, h) = (64, 48);
        let rgb = synthetic_thumb_rgb(w, h);
        let jpeg = thumb_jpeg_tiny_codec(w, h, &rgb).expect("encodes");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
        let (dw, dh, back) = tiny_jfif::demux::decode_jpeg(&jpeg).expect("decodes");
        assert_eq!((dw, dh), (w, h));
        let psnr = tiny_jfif::metrics::psnr_luma(w, h, &rgb, &back);
        assert!(psnr >= 40.0, "thumb luma PSNR {psnr}");
        // Bad input is None, never a panic (strip-job contract).
        assert!(thumb_jpeg_tiny_codec(0, h, &[]).is_none());
        assert!(thumb_jpeg_tiny_codec(w, h, &rgb[..rgb.len() - 1]).is_none());
    }

    #[test]
    fn strip_extracts_tiny_codec_frames_when_ffmpeg_exists() {
        // Same skip-guarded real-ffmpeg harness as the Ffmpeg-engine test,
        // but the JPEGs must come from tiny-codec and decode in it too.
        if resolve_ffmpeg(None).is_none() {
            eprintln!("SKIP: no ffmpeg on PATH");
            return;
        }
        let dir = test_dir("real-tiny");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("pattern.mp4");
        let made = Command::new(resolve_ffmpeg(None).unwrap())
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=128x96:rate=30",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&src)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !made || !src.is_file() {
            eprintln!("SKIP: could not synthesize test clip");
            return;
        }
        let req = ThumbStripRequest {
            file: src.to_string_lossy().into_owned(),
            count: 2,
            width_px: 64,
            positions_seconds: Some(vec![0.2, 1.8]),
            duration_hint_seconds: None,
            ffmpeg_bin: None,
            jpeg: StripJpeg::TinyCodec,
            out_dir: Some(dir.join("thumbs")),
        };
        let strip = extract_strip(&req);
        assert!(!strip.is_placeholder(), "tool: {:?}", strip.tool);
        assert!(matches!(strip.tool, StripTool::TinyCodec(_)));
        assert_eq!(strip.frames.len(), 2);
        for f in &strip.frames {
            assert!(!f.placeholder);
            let p = f.path.as_ref().expect("real frame needs a path");
            let bytes = std::fs::read(p).expect("thumb readable");
            let (dw, dh, _) = tiny_jfif::demux::decode_jpeg(&bytes).expect("thumb decodes");
            assert_eq!(dw, 64);
            assert_eq!(dh, 48);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
