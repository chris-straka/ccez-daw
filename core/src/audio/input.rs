//! cpal input capture + the headless null-input fallback.
//!
//! Teaching note: this is the missing input half of [`super::device`]. The
//! output side renders a graph to the speakers; this side captures the
//! microphone (or nothing, on CI) into the record path. The same realtime
//! rules apply:
//!
//! - The **audio thread pushes**: the cpal input callback downmixes each
//!   device block to mono `f32` and `send()`s it into an unbounded channel.
//!   `send()` never blocks, so the callback never waits on the UI.
//! - The **record side drains**: [`TakeCapture::push`] appends drained
//!   blocks while the transport punches; monitoring levels come from
//!   [`monitor_levels`], a pure function over whatever was captured.
//! - **No hardware is never an error**: [`list_input_devices`] returns an
//!   empty list, [`open_input_or_null`] returns the null source, and
//!   [`NullInput`] synthesizes a deterministic tone — so `cargo test` and
//!   headless CI exercise the identical record path without panicking.
//!
//! Takes land as ordinary audio [`Clip`](crate::model::Clip)s through the
//! frozen `ClipAdded` op ([`crate::record::take_commit_op`]), so recording
//! undoes/redoes like any other op. Adds no IPC or project-schema surface,
//! so the typegen drift gate is unaffected.

use std::sync::mpsc::{self, Receiver, Sender};

use crate::model::{Clip, Op};
use crate::record::{punch_take_clip, take_commit_op, PunchRange};

use super::device::DeviceError;

/// One selectable input source: `id` is stable for the session (the cpal
/// device name), `name` is what the record panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceInfo {
    pub id: String,
    pub name: String,
}

/// List cpal input devices. Never panics and never errors: no hardware (or
/// a backend that refuses to enumerate) yields an empty list, and the
/// caller falls back to the null input.
pub fn list_input_devices() -> Vec<InputDeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices
            .filter_map(|d| {
                d.name()
                    .ok()
                    .map(|name| InputDeviceInfo { id: name.clone(), name })
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Live input levels for one block (or one whole take): peak amplitude and
/// RMS energy, both in `[0, ∞)` and both `0` for silence/empty input. Pure
/// function — the panel calls this on whatever it drained, hardware or
/// null alike.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorLevels {
    pub peak: f32,
    pub rms: f32,
}

pub fn monitor_levels(samples: &[f32]) -> MonitorLevels {
    if samples.is_empty() {
        return MonitorLevels { peak: 0.0, rms: 0.0 };
    }
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    for &s in samples {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += (s as f64) * (s as f64);
    }
    let rms = (sum_sq / samples.len() as f64).sqrt() as f32;
    MonitorLevels { peak, rms }
}

/// UI-local input-device selection. Like [`crate::record::ArmState`] this
/// is a control-room switch, not project state: `None` means the null
/// (headless/CI-safe) input, `Some(id)` names a cpal device from
/// [`list_input_devices`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputSelect {
    selected: Option<String>,
}

impl InputSelect {
    pub fn new() -> Self {
        Self::default()
    }

    /// Select a device (`None` reselects the null input).
    pub fn select(&mut self, device_id: Option<&str>) {
        self.selected = device_id.map(|s| s.to_string());
    }

    pub fn clear(&mut self) {
        self.selected = None;
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// True while capturing from the null fallback.
    pub fn is_null(&self) -> bool {
        self.selected.is_none()
    }
}

/// Headless/CI-safe input source: synthesizes a deterministic 440 Hz sine
/// at 0.5 amplitude. Same pull shape as the hardware drain, so tests prove
/// the record path byte for byte without a microphone.
#[derive(Debug, Clone)]
pub struct NullInput {
    sample_rate_hz: u32,
    phase: f64,
}

impl NullInput {
    pub fn new(sample_rate_hz: u32) -> Self {
        Self { sample_rate_hz: sample_rate_hz.max(1), phase: 0.0 }
    }

    /// Capture `frames` mono samples. Deterministic: two inputs built the
    /// same way produce identical takes.
    pub fn capture(&mut self, frames: usize) -> Vec<f32> {
        let rate = self.sample_rate_hz as f64;
        let step = 440.0 * std::f64::consts::TAU / rate;
        let mut out = Vec::with_capacity(frames);
        for _ in 0..frames {
            out.push((self.phase.sin() * 0.5) as f32);
            self.phase += step;
            if self.phase >= std::f64::consts::TAU {
                self.phase -= std::f64::consts::TAU;
            }
        }
        out
    }
}

/// Live cpal input: the stream callback downmixes each device block to
/// mono `f32` and pushes it into a channel; the record side drains without
/// blocking. Owns its `cpal::Stream` (kept alive: dropping stops capture),
/// so it lives on one thread and is never moved across threads.
pub struct CpalInput {
    _stream: cpal::Stream,
    rx: Receiver<Vec<f32>>,
}

impl CpalInput {
    /// Open the default input device. A [`DeviceError::NoDevice`] means "no
    /// hardware here" — the caller degrades to [`NullInput`] instead of
    /// panicking (see [`open_input_or_null`]).
    pub fn open_default() -> Result<Self, DeviceError> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| DeviceError::NoDevice("default input device not found".to_string()))?;
        let supported = device.default_input_config().map_err(|e| {
            DeviceError::NoDevice(format!("default input config: {e}"))
        })?;
        let (tx, rx) = mpsc::channel();
        let stream = build_input_stream(&device, &supported, tx)?;
        {
            use cpal::traits::StreamTrait;
            stream.play().map_err(|e| DeviceError::Stream(e.to_string()))?;
        }
        Ok(Self { _stream: stream, rx })
    }

    /// Drain pending capture blocks without blocking. Returns silence when
    /// the callback has not delivered yet — the take still advances, so a
    /// slow device stretches time instead of stalling the record path.
    pub fn drain(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        while let Ok(block) = self.rx.try_recv() {
            out.extend_from_slice(&block);
            if out.len() >= frames {
                break;
            }
        }
        out.resize(frames, 0.0);
        out.truncate(frames);
        out
    }
}

fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    tx: Sender<Vec<f32>>,
) -> Result<cpal::Stream, DeviceError> {
    use cpal::traits::DeviceTrait;
    let channels = config.channels() as usize;
    let stream_config: cpal::StreamConfig = config.clone().into();
    let err_fn = |err| eprintln!("cpal input stream error: {err}");
    // Downmix one interleaved device block to mono f32. Unbounded send:
    // never blocks the callback, never drops.
    let push = move |mono: Vec<f32>| {
        let _ = tx.send(mono);
    };
    match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _| push(downmix(data, channels)),
                err_fn,
                None,
            )
            .map_err(|e| DeviceError::Stream(e.to_string())),
        cpal::SampleFormat::I16 => device
            .build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    push(downmix_convert(data, channels, |s| s as f32 / i16::MAX as f32))
                },
                err_fn,
                None,
            )
            .map_err(|e| DeviceError::Stream(e.to_string())),
        cpal::SampleFormat::U16 => device
            .build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    push(downmix_convert(data, channels, |s| {
                        s as f32 / u16::MAX as f32 * 2.0 - 1.0
                    }))
                },
                err_fn,
                None,
            )
            .map_err(|e| DeviceError::Stream(e.to_string())),
        other => Err(DeviceError::Stream(format!("unsupported sample format: {other:?}"))),
    }
}

/// Average each frame's channels into one mono sample.
fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels == 0 || data.is_empty() {
        return Vec::new();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

fn downmix_convert<T>(data: &[T], channels: usize, conv: impl Fn(T) -> f32 + Copy) -> Vec<f32>
where
    T: Copy,
{
    if channels == 0 || data.is_empty() {
        return Vec::new();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().map(|&s| conv(s)).sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Either live hardware or the deterministic null source. Returned by
/// [`open_input_or_null`]: the one call sites use so headless machines get
/// a working record path instead of a panic.
pub enum AnyInput {
    Live(CpalInput),
    Null(NullInput),
}

impl AnyInput {
    /// Capture `frames` mono samples from whichever source is live. Never
    /// panics, never errors: the null source always answers.
    pub fn capture(&mut self, frames: usize) -> Vec<f32> {
        match self {
            Self::Live(input) => input.drain(frames),
            Self::Null(input) => input.capture(frames),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null(_))
    }
}

/// Open the default input, falling back to the deterministic null source
/// when there is no hardware. Infallible by design: headless/CI gets the
/// null input, never a panic.
pub fn open_input_or_null(sample_rate_hz: u32) -> AnyInput {
    match CpalInput::open_default() {
        Ok(live) => AnyInput::Live(live),
        Err(_) => AnyInput::Null(NullInput::new(sample_rate_hz)),
    }
}

/// One armed track's in-progress take: accumulates drained capture blocks
/// while the transport punches, then lands as an ordinary audio clip
/// through the frozen `ClipAdded` op (undoable for free).
#[derive(Debug, Clone)]
pub struct TakeCapture {
    track_id: String,
    device_id: Option<String>,
    sample_rate_hz: u32,
    samples: Vec<f32>,
    peak: f32,
    sum_sq: f64,
}

impl TakeCapture {
    pub fn new(track_id: &str, device_id: Option<&str>, sample_rate_hz: u32) -> Self {
        Self {
            track_id: track_id.to_string(),
            device_id: device_id.map(|s| s.to_string()),
            sample_rate_hz: sample_rate_hz.max(1),
            samples: Vec::new(),
            peak: 0.0,
            sum_sq: 0.0,
        }
    }

    /// Append one drained capture block. Called only while punching (the
    /// caller gates on [`crate::record::is_punching`]).
    pub fn push(&mut self, block: &[f32]) {
        for &s in block {
            let a = s.abs();
            if a > self.peak {
                self.peak = a;
            }
            self.sum_sq += (s as f64) * (s as f64);
            self.samples.push(s);
        }
    }

    pub fn frames(&self) -> usize {
        self.samples.len()
    }

    pub fn track_id(&self) -> &str {
        &self.track_id
    }

    pub fn device_id(&self) -> Option<&str> {
        self.device_id.as_deref()
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Live levels over everything captured so far (drives the panel meter).
    pub fn levels(&self) -> MonitorLevels {
        if self.samples.is_empty() {
            return MonitorLevels { peak: 0.0, rms: 0.0 };
        }
        MonitorLevels {
            peak: self.peak,
            rms: (self.sum_sq / self.samples.len() as f64).sqrt() as f32,
        }
    }

    /// Finish the take as an audio clip spanning exactly `punch` with
    /// `source = "take:<id>"` — the convention `comp` consumes. The raw
    /// samples ride along as the take body in a real host; the clip itself
    /// is the undoable project half (same stance as `punch_take_clip`).
    pub fn finish_clip(&self, id: &str, name: &str, punch: &PunchRange) -> Clip {
        let _ = self.sample_rate_hz;
        punch_take_clip(id, &self.track_id, name, crate::model::ClipKind::Audio, punch)
    }

    /// Commit the finished take as one ordinary frozen `ClipAdded` op.
    pub fn commit_op(&self, actor: &str, id: &str, name: &str, punch: &PunchRange) -> Op {
        take_commit_op(actor, &self.finish_clip(id, name, punch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::model::{OpKind, Project, Track};
    use crate::record::{is_punching, PunchMode};

    fn track_json(id: &str, name: &str) -> String {
        serde_json::to_string(&Track {
            id: id.to_string(),
            name: name.to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        })
        .expect("track serializes")
    }

    #[test]
    fn null_input_record_roundtrip() {
        // The headless record path end to end: null source → gated capture
        // → take clip. Deterministic: identical runs capture identical
        // audio, and levels stay finite and positive.
        let punch = crate::record::validate_punch(0.0, 4.0).unwrap();
        let mode = PunchMode::Auto(punch.clone());
        let mut src = NullInput::new(48_000);
        let mut take = TakeCapture::new("trk_vox", None, 48_000);
        let mut beats = 0.0;
        for _ in 0..8 {
            let block = src.capture(128);
            assert_eq!(block.len(), 128);
            if is_punching(beats, &mode, true) {
                take.push(&block);
            }
            beats += 0.5;
        }
        assert_eq!(take.frames(), 8 * 128);
        let levels = take.levels();
        assert!(levels.peak > 0.0 && levels.peak <= 0.5 + 1e-6, "peak {}", levels.peak);
        assert!(levels.rms > 0.0 && levels.rms <= levels.peak, "rms {}", levels.rms);

        let clip = take.finish_clip("take_vox_p1", "Vox p1", &punch);
        assert_eq!(clip.track_id, "trk_vox");
        assert_eq!(clip.kind, crate::model::ClipKind::Audio);
        assert_eq!(clip.source, "take:take_vox_p1");
        assert_eq!(clip.length_beats, 4.0);

        // Determinism: a fresh null source replays the same take.
        let mut src2 = NullInput::new(48_000);
        let mut take2 = TakeCapture::new("trk_vox", None, 48_000);
        for _ in 0..8 {
            take2.push(&src2.capture(128));
        }
        assert_eq!(take2.levels(), take.levels());
    }

    #[test]
    fn take_lands_as_clip_and_undoes() {
        // The recorded take enters the project through the ordinary
        // ClipAdded op — and undo removes it like any other op.
        let dir = std::env::temp_dir().join(format!(
            "ccez-input-take-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let mut engine = Engine::create(&dir, Project::new("p", "P")).expect("create");
        engine
            .apply("ui", OpKind::TrackAdded, "trk_vox", &track_json("trk_vox", "Vox"))
            .expect("track");

        let punch = crate::record::validate_punch(4.0, 8.0).unwrap();
        let mut src = NullInput::new(44_100);
        let mut take = TakeCapture::new("trk_vox", None, 44_100);
        take.push(&src.capture(256));
        let op = take.commit_op("ui", "take_vox_p1", "Vox p1", &punch);
        assert_eq!(op.kind, OpKind::ClipAdded);
        engine
            .apply(&op.actor, op.kind.clone(), &op.target, &op.value_json)
            .expect("take applies");

        let clip = engine.project().clips.iter().find(|c| c.id == "take_vox_p1").expect("clip");
        assert_eq!(clip.source, "take:take_vox_p1");
        assert_eq!((clip.start_beats, clip.length_beats), (4.0, 4.0));
        engine.undo().expect("undo");
        assert!(engine.project().clips.iter().all(|c| c.id != "take_vox_p1"));
        engine.redo().expect("redo");
        assert!(engine.project().clips.iter().any(|c| c.id == "take_vox_p1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn input_helpers_never_panic_headless() {
        // Enumeration, selection, fallback, and silence levels: none of
        // these may panic or error, hardware or not.
        let _ = list_input_devices();
        let mut sel = InputSelect::new();
        assert!(sel.is_null());
        sel.select(Some("mic"));
        assert_eq!(sel.selected(), Some("mic"));
        assert!(!sel.is_null());
        sel.clear();
        assert!(sel.is_null());

        let mut any = open_input_or_null(44_100);
        let block = any.capture(64);
        assert_eq!(block.len(), 64);
        assert!(block.iter().all(|s| s.is_finite()));
        let _ = any.is_null();

        assert_eq!(monitor_levels(&[]), MonitorLevels { peak: 0.0, rms: 0.0 });
        let flat = monitor_levels(&[0.5; 16]);
        assert!((flat.peak - 0.5).abs() < 1e-6);
        assert!((flat.rms - 0.5).abs() < 1e-6);
        assert_eq!(downmix(&[], 2), Vec::<f32>::new());
        assert_eq!(downmix(&[1.0, 3.0], 2), vec![2.0]);
    }
}
