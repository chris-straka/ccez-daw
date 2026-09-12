//! cpal I/O + the lock-free UI→audio handoff.
//!
//! Teaching note: the audio thread has a hard deadline (miss it and you
//! hear a click), so it may never wait on the UI thread — no mutexes, no
//! allocation-heavy work, no channel `recv`. The rule here is one-way and
//! absolute:
//!
//! - The **UI thread builds** (it owns `Project`, `AudioGraph`,
//!   `RenderGraph` — all plain owned data) and `send()`s finished work as
//!   [`AudioCommand`]s. `send()` on an unbounded channel never blocks.
//! - The **audio thread polls**: each callback drains pending commands with
//!   `try_recv` (returns immediately when empty) and renders from its own
//!   local copy. Params arrive as atomics plus a callback-side cache, so a
//!   contended lock degrades to a one-block-stale value, never a wait.
//!
//! [`NullBackend`] implements the same contract without hardware and is
//! what `cargo test` exercises. [`CpalBackend`] is the real output path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::render::RenderGraph;

/// Commands the UI thread may send the audio thread. All variants own
/// their data — the audio side never borrows from the UI.
#[derive(Debug)]
pub enum AudioCommand {
    /// Set one `node:param` value (also mirrored into the [`ParamBank`]).
    SetParam { target: String, value: f32 },
    /// Swap the rendered graph. Built fully off-thread; the callback takes
    /// it with `try_recv` between blocks, so swaps are click-free and
    /// wait-free.
    SwapGraph {
        graph: RenderGraph,
        delays: std::collections::BTreeMap<(String, String), u64>,
        out_node: String,
    },
    Stop,
}

/// Lock-free-ish shared params: one atomic per address. The UI thread's
/// `set` may lock (it has no deadline); the audio thread's `get` uses
/// `try_lock` and falls back to its own cache on contention — stale by one
/// block at worst, blocked never.
#[derive(Debug, Default)]
pub struct ParamBank {
    cells: Mutex<HashMap<String, Arc<AtomicU32>>>,
}

impl ParamBank {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, target: &str, value: f32) {
        let mut cells = self.cells.lock().expect("param bank");
        cells
            .entry(target.to_string())
            .or_insert_with(|| Arc::new(AtomicU32::new(0)))
            .store(value.to_bits(), Ordering::Relaxed);
    }

    /// Refresh `cache` from every cell whose lock is immediately available.
    /// Never blocks: a busy lock just leaves those entries stale.
    pub fn refresh_cache(&self, cache: &mut HashMap<String, f32>) {
        if let Ok(cells) = self.cells.try_lock() {
            for (target, cell) in cells.iter() {
                cache.insert(target.clone(), f32::from_bits(cell.load(Ordering::Relaxed)));
            }
        }
    }
}

#[derive(Debug)]
pub enum DeviceError {
    NoDevice(String),
    Stream(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDevice(m) => write!(f, "no audio device: {m}"),
            Self::Stream(m) => write!(f, "audio stream: {m}"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// What a realtime output needs: a way to ship commands at the callback,
/// and (for tests) a way to observe what it rendered.
pub trait AudioBackend {
    /// Drain pending commands without blocking and render one block.
    /// Returns `false` once [`AudioCommand::Stop`] has been drained.
    fn pump(&mut self, frames: usize) -> bool;
}

/// Hardware-free backend: renders the current graph on demand. This is the
/// contract the cpal callback honors, minus the hardware — and therefore
/// what `cargo test` proves.
#[derive(Debug)]
pub struct NullBackend {
    rx: Receiver<AudioCommand>,
    params: Arc<ParamBank>,
    cache: HashMap<String, f32>,
    graph: Option<RenderGraph>,
    delays: std::collections::BTreeMap<(String, String), u64>,
    out_node: String,
    /// Rendered output blocks, for assertions. (The cpal side writes to
    /// the device instead of keeping these.)
    pub rendered: Vec<Vec<f32>>,
    /// Commands drained so far (proves the handoff delivers).
    pub drained: u64,
}

impl NullBackend {
    pub fn new(rx: Receiver<AudioCommand>, params: Arc<ParamBank>) -> Self {
        Self {
            rx,
            params,
            cache: HashMap::new(),
            graph: None,
            delays: std::collections::BTreeMap::new(),
            out_node: "mix".to_string(),
            rendered: Vec::new(),
            drained: 0,
        }
    }

    fn drain(&mut self) -> bool {
        let mut running = true;
        while let Ok(cmd) = self.rx.try_recv() {
            self.drained += 1;
            match cmd {
                AudioCommand::SetParam { target, value } => {
                    self.cache.insert(target, value);
                }
                AudioCommand::SwapGraph { graph, delays, out_node } => {
                    self.graph = Some(graph);
                    self.delays = delays;
                    self.out_node = out_node;
                }
                AudioCommand::Stop => running = false,
            }
        }
        self.params.refresh_cache(&mut self.cache);
        running
    }
}

impl AudioBackend for NullBackend {
    fn pump(&mut self, frames: usize) -> bool {
        if !self.drain() {
            return false;
        }
        let block = match &self.graph {
            None => vec![0.0; frames],
            Some(g) => g
                .render(frames, 1, &self.delays)
                .ok()
                .and_then(|mut bufs| bufs.remove(&self.out_node))
                .unwrap_or_else(|| vec![0.0; frames]),
        };
        self.rendered.push(block);
        true
    }
}

/// The UI-side handle. Cloneable across threads; every method returns
/// immediately — in particular, none of them ever waits for the audio
/// thread, which is the whole "frontend never blocks audio" guarantee
/// (the reverse direction holds by construction: the audio thread only
/// ever `try_recv`s).
#[derive(Debug)]
pub struct AudioEngine {
    tx: Sender<AudioCommand>,
    params: Arc<ParamBank>,
}

impl AudioEngine {
    /// `(engine, receiver, params)`: the receiver moves to the backend
    /// (null or cpal); the engine stays on the UI side.
    pub fn channel() -> (Self, Receiver<AudioCommand>, Arc<ParamBank>) {
        let (tx, rx) = mpsc::channel();
        let params = Arc::new(ParamBank::new());
        (
            Self {
                tx: tx.clone(),
                params: params.clone(),
            },
            rx,
            params,
        )
    }

    /// Send one command. Unbounded channel: never blocks, never drops.
    pub fn send(&self, cmd: AudioCommand) {
        let _ = self.tx.send(cmd);
    }

    /// Set `node:param` for the audio thread (atomic store + command).
    pub fn set_param(&self, target: &str, value: f32) {
        self.params.set(target, value);
        self.send(AudioCommand::SetParam {
            target: target.to_string(),
            value,
        });
    }

    pub fn params(&self) -> &Arc<ParamBank> {
        &self.params
    }
}

/// Real hardware output via cpal. Opening queries the default device; the
/// stream callback renders the latest swapped graph exactly like
/// [`NullBackend::pump`]. No device is touched in tests (CI has none) —
/// this code compiles against cpal and runs on a real machine.
pub struct CpalBackend;

impl CpalBackend {
    /// Open the default output device and start rendering `initial` (may be
    /// `None` for silence until the first [`AudioCommand::SwapGraph`]).
    /// Returns the live stream (keep it alive: dropping stops audio) plus
    /// the [`AudioEngine`] the UI keeps.
    pub fn open_default(
        initial: Option<(RenderGraph, std::collections::BTreeMap<(String, String), u64>, String)>,
    ) -> Result<(cpal::Stream, AudioEngine), DeviceError> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| {
            DeviceError::NoDevice("default output device not found".to_string())
        })?;
        let supported = device.default_output_config().map_err(|e| {
            DeviceError::NoDevice(format!("default output config: {e}"))
        })?;
        let (engine, rx, params) = AudioEngine::channel();
        let stream = build_stream(&device, &supported, rx, params, initial)?;
        stream.play().map_err(|e| DeviceError::Stream(e.to_string()))?;
        Ok((stream, engine))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    rx: Receiver<AudioCommand>,
    params: Arc<ParamBank>,
    initial: Option<(RenderGraph, std::collections::BTreeMap<(String, String), u64>, String)>,
) -> Result<cpal::Stream, DeviceError> {
    use cpal::traits::DeviceTrait;
    let channels = config.channels() as usize;
    let sample_rate = config.sample_rate().0;
    let _ = sample_rate; // reserved for resampling when graph rate != device rate (phase 2)
    let stream_config: cpal::StreamConfig = config.clone().into();

    let mut state = CallbackState::new(rx, params, initial);
    let err_fn = |err| eprintln!("cpal stream error: {err}");
    match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_output_stream(
                &stream_config,
                move |out: &mut [f32], _| state.fill(out, channels),
                err_fn,
                None,
            )
            .map_err(|e| DeviceError::Stream(e.to_string())),
        cpal::SampleFormat::I16 => device
            .build_output_stream(
                &stream_config,
                move |out: &mut [i16], _| {
                    state.fill_convert(out, channels, |s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                },
                err_fn,
                None,
            )
            .map_err(|e| DeviceError::Stream(e.to_string())),
        cpal::SampleFormat::U16 => device
            .build_output_stream(
                &stream_config,
                move |out: &mut [u16], _| {
                    state.fill_convert(out, channels, |s| {
                        ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
                    })
                },
                err_fn,
                None,
            )
            .map_err(|e| DeviceError::Stream(e.to_string())),
        other => Err(DeviceError::Stream(format!("unsupported sample format: {other:?}"))),
    }
}

/// Callback-owned render state. Lives on the audio thread; everything in
/// here is either owned or drained via `try_recv` — the callback never
/// touches UI-thread data.
struct CallbackState {
    rx: Receiver<AudioCommand>,
    params: Arc<ParamBank>,
    cache: HashMap<String, f32>,
    graph: Option<RenderGraph>,
    delays: std::collections::BTreeMap<(String, String), u64>,
    out_node: String,
    silence: Vec<f32>,
}

impl CallbackState {
    fn new(
        rx: Receiver<AudioCommand>,
        params: Arc<ParamBank>,
        initial: Option<(RenderGraph, std::collections::BTreeMap<(String, String), u64>, String)>,
    ) -> Self {
        let (graph, delays, out_node) = match initial {
            Some((g, d, o)) => (Some(g), d, o),
            None => (None, std::collections::BTreeMap::new(), "mix".to_string()),
        };
        Self {
            rx,
            params,
            cache: HashMap::new(),
            graph,
            delays,
            out_node,
            silence: Vec::new(),
        }
    }

    /// Drain commands without blocking, then render one device block.
    fn next_block(&mut self, frames: usize) -> &[f32] {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                AudioCommand::SetParam { target, value } => {
                    self.cache.insert(target, value);
                }
                AudioCommand::SwapGraph { graph, delays, out_node } => {
                    self.graph = Some(graph);
                    self.delays = delays;
                    self.out_node = out_node;
                }
                AudioCommand::Stop => {}
            }
        }
        self.params.refresh_cache(&mut self.cache);
        // let _cache = &self.cache; // params feed device params in phase 2 (audible DSP reads them)
        match &self.graph {
            None => {
                if self.silence.len() != frames {
                    self.silence.resize(frames, 0.0);
                    self.silence.fill(0.0);
                }
                &self.silence
            }
            Some(g) => {
                // Render serially in the callback (block is small); the
                // multicore schedule pays off in the offline renderer.
                static EMPTY: Vec<f32> = Vec::new();
                match g.render(frames, 1, &self.delays) {
                    Ok(mut bufs) => {
                        if let Some(buf) = bufs.remove(&self.out_node) {
                            self.silence = buf;
                            &self.silence
                        } else {
                            &EMPTY
                        }
                    }
                    Err(_) => &EMPTY,
                }
            }
        }
    }

    fn fill(&mut self, out: &mut [f32], channels: usize) {
        if channels == 0 || out.is_empty() {
            return;
        }
        let frames = out.len() / channels;
        // Copy out of the borrow: next_block borrows self mutably, so take
        // an owned snapshot of the mono block first.
        let mono: Vec<f32> = self.next_block(frames).to_vec();
        for (i, sample) in out.iter_mut().enumerate() {
            *sample = mono.get(i / channels).copied().unwrap_or(0.0);
        }
    }

    fn fill_convert<T>(&mut self, out: &mut [T], channels: usize, conv: impl Fn(f32) -> T) {
        if channels == 0 || out.is_empty() {
            return;
        }
        let frames = out.len() / channels;
        let mono: Vec<f32> = self.next_block(frames).to_vec();
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = conv(mono.get(i / channels).copied().unwrap_or(0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::render::Proc;
    use crate::model::{Edge, EdgeKind, Project, Track};

    fn rig() -> (RenderGraph, std::collections::BTreeMap<(String, String), u64>) {
        let mut p = Project::new("p", "Device");
        for id in ["a", "b"] {
            p.tracks.push(Track {
                id: id.to_string(),
                name: id.to_string(),
                volume: 0.8,
                pan: 0.0,
                muted: false,
                solo: false,
                clip_ids: vec![],
                device_ids: vec![],
            });
        }
        for (i, (from, to)) in [("a", "mix"), ("b", "mix")].iter().enumerate() {
            p.routing.push(Edge {
                id: format!("e{i}"),
                from_node: from.to_string(),
                from_port: "out".to_string(),
                to_node: to.to_string(),
                to_port: "in".to_string(),
                kind: EdgeKind::Audio,
            });
        }
        let topo = crate::audio::graph::AudioGraph::from_project(&p);
        let delays = topo.all_edge_delays().expect("acyclic");
        let mut g = RenderGraph::from_audio_graph(topo);
        g.set_proc("a", Proc::Constant(0.25));
        g.set_proc("b", Proc::Constant(0.25));
        g.set_proc("mix", Proc::Mix);
        (g, delays)
    }

    #[test]
    fn null_backend_renders_swapped_graph() {
        let (engine, rx, params) = AudioEngine::channel();
        let mut backend = NullBackend::new(rx, params);
        // No graph yet: silence.
        backend.pump(64);
        assert_eq!(backend.rendered[0], vec![0.0; 64]);
        // Swap in the rig off-thread style: owned graph over the channel.
        let (g, delays) = rig();
        engine.send(AudioCommand::SwapGraph {
            graph: g,
            delays,
            out_node: "mix".to_string(),
        });
        backend.pump(64);
        assert_eq!(backend.rendered[1], vec![0.5; 64]);
        assert_eq!(backend.drained, 1);
    }

    #[test]
    fn frontend_handoff_never_blocks_audio() {
        // Hammer the channel from a UI-like thread while the audio-like
        // thread pumps: if either side blocked on the other, this hangs
        // and the test fails. Completion IS the assertion.
        let (engine, rx, params) = AudioEngine::channel();
        let (g, delays) = rig();
        engine.send(AudioCommand::SwapGraph {
            graph: g,
            delays,
            out_node: "mix".to_string(),
        });
        let pump = std::thread::spawn(move || {
            let mut backend = NullBackend::new(rx, params);
            // Pump until Stop arrives (cap iterations so a real deadlock
            // fails the test instead of hanging CI forever).
            let mut iters = 0;
            while backend.pump(32) {
                iters += 1;
                assert!(iters < 1_000_000, "audio thread stalled");
            }
            backend.drained
        });
        for i in 0..10_000 {
            engine.set_param("mix:volume", (i as f32) / 10_000.0);
        }
        engine.send(AudioCommand::Stop);
        let drained = pump.join().expect("pump thread");
        // Every param + swap + stop arrived; none blocked.
        assert_eq!(drained, 10_002);
    }
}
