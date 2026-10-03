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
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::render::RenderGraph;

/// Commands the UI thread may send the audio thread. All variants own
/// their data — the audio side never borrows from the UI.
#[derive(Debug)]
pub enum AudioCommand {
    /// Set one `node:param` value (also mirrored into the [`ParamBank`]).
    SetParam {
        target: String,
        value: f32,
    },
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
    /// Never blocks: a busy lock just leaves those entries stale. Entries
    /// whose value did not change are left untouched, so the steady-state
    /// callback does no per-param `String` clone or map insert — only
    /// changed params allocate, and only once per change.
    pub fn refresh_cache(&self, cache: &mut HashMap<String, f32>) {
        if let Ok(cells) = self.cells.try_lock() {
            for (target, cell) in cells.iter() {
                let value = f32::from_bits(cell.load(Ordering::Relaxed));
                if cache.get(target.as_str()).copied() != Some(value) {
                    cache.insert(target.clone(), value);
                }
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

/// One mono-block render shared by every output path.
///
/// Both [`NullBackend::pump`] and the cpal callback call this with
/// `threads = 1`, so the offline bounce and the live stream are
/// byte-identical by construction: there is only one code path.
/// Returns `None` when the callback must substitute silence (no graph yet,
/// missing out node, or a schedule error) — the caller counts that block
/// as an underrun.
pub fn render_mono_block(
    graph: Option<&RenderGraph>,
    delays: &std::collections::BTreeMap<(String, String), u64>,
    out_node: &str,
    frames: usize,
    start_frame: u64,
) -> Option<Vec<f32>> {
    let g = graph?;
    g.render(frames, 1, delays, start_frame)
        .ok()?
        .remove(out_node)
}

/// Thread-safe block counters shared between an audio callback (or the
/// null backend's pump loop) and the UI side.
///
/// - `blocks`: blocks rendered so far.
/// - `underruns`: blocks where silence was substituted because no renderable
///   graph was available (no graph swapped yet, missing out node, or a
///   schedule error). On hardware each one is an audible dropout.
/// - `overruns`: command bursts absorbed inside one block's deadline — the
///   number of commands drained beyond the first in a single block. A
///   nonzero count means the UI thread queued more deferred work than one
///   callback budget; the audio still rendered, but the handoff is hot.
#[derive(Debug, Clone, Default)]
pub struct SharedCounters {
    /// Actual mono frames rendered, independent of callback block size.
    pub frames: Arc<AtomicU64>,
    /// Selected hardware output rate, populated before the stream starts.
    pub sample_rate: Arc<std::sync::atomic::AtomicU32>,
    pub blocks: Arc<AtomicU64>,
    pub underruns: Arc<AtomicU64>,
    pub overruns: Arc<AtomicU64>,
}

impl SharedCounters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.blocks.load(Ordering::Relaxed),
            self.underruns.load(Ordering::Relaxed),
            self.overruns.load(Ordering::Relaxed),
        )
    }
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
    /// Running frame count across pumps: the loop position. A graph swap
    /// keeps it (edits must not restart the loop), only construction
    /// zeroes it.
    rendered_frames: u64,
    /// Rendered output blocks, for assertions. (The cpal side writes to
    /// the device instead of keeping these.)
    pub rendered: Vec<Vec<f32>>,
    /// Commands drained so far (proves the handoff delivers).
    pub drained: u64,
    /// Block counters (mirrors what the cpal callback shares with the UI).
    pub counters: SharedCounters,
}

impl NullBackend {
    pub fn new(rx: Receiver<AudioCommand>, params: Arc<ParamBank>) -> Self {
        Self::with_counters(rx, params, SharedCounters::new())
    }

    pub fn with_counters(
        rx: Receiver<AudioCommand>,
        params: Arc<ParamBank>,
        counters: SharedCounters,
    ) -> Self {
        Self {
            rx,
            params,
            cache: HashMap::new(),
            graph: None,
            delays: std::collections::BTreeMap::new(),
            out_node: "mix".to_string(),
            rendered_frames: 0,
            rendered: Vec::new(),
            drained: 0,
            counters,
        }
    }

    /// Drain pending commands without blocking. Returns `(running, n)` where
    /// `running` is false once [`AudioCommand::Stop`] arrived and `n` is the
    /// number of commands drained (feeds the overrun counter).
    fn drain(&mut self) -> (bool, u64) {
        let mut running = true;
        let mut n = 0u64;
        while let Ok(cmd) = self.rx.try_recv() {
            n += 1;
            self.drained += 1;
            match cmd {
                AudioCommand::SetParam { target, value } => {
                    self.cache.insert(target, value);
                }
                AudioCommand::SwapGraph {
                    graph,
                    delays,
                    out_node,
                } => {
                    self.graph = Some(graph);
                    self.delays = delays;
                    self.out_node = out_node;
                }
                AudioCommand::Stop => running = false,
            }
        }
        self.params.refresh_cache(&mut self.cache);
        (running, n)
    }
}

impl AudioBackend for NullBackend {
    fn pump(&mut self, frames: usize) -> bool {
        let (running, n) = self.drain();
        if !running {
            return false;
        }
        if n > 1 {
            self.counters.overruns.fetch_add(n - 1, Ordering::Relaxed);
        }
        // Same code path as the cpal callback: byte-identical by
        // construction (see `render_mono_block`).
        let block = match render_mono_block(
            self.graph.as_ref(),
            &self.delays,
            &self.out_node,
            frames,
            self.rendered_frames,
        ) {
            Some(b) => b,
            None => {
                self.counters.underruns.fetch_add(1, Ordering::Relaxed);
                vec![0.0; frames]
            }
        };
        self.rendered_frames += frames as u64;
        self.counters
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        self.counters.blocks.fetch_add(1, Ordering::Relaxed);
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
    /// Returns the live stream (keep it alive: dropping stops audio), the
    /// [`AudioEngine`] the UI keeps, and the [`SharedCounters`] the
    /// realtime callback feeds (underrun/overrun accounting).
    /// A [`DeviceError::NoDevice`] means "no hardware here" — the caller
    /// (see `transport`) degrades to the null device instead of panicking.
    pub fn open_default(
        initial: Option<(
            RenderGraph,
            std::collections::BTreeMap<(String, String), u64>,
            String,
        )>,
    ) -> Result<(cpal::Stream, AudioEngine, SharedCounters), DeviceError> {
        Self::open_default_with_counters(initial, SharedCounters::new())
    }

    /// Same as [`CpalBackend::open_default`], but the callback feeds the
    /// caller's counters so one controller observes both backends.
    pub fn open_default_with_counters(
        initial: Option<(
            RenderGraph,
            std::collections::BTreeMap<(String, String), u64>,
            String,
        )>,
        counters: SharedCounters,
    ) -> Result<(cpal::Stream, AudioEngine, SharedCounters), DeviceError> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| DeviceError::NoDevice("default output device not found".to_string()))?;
        let supported = device
            .default_output_config()
            .map_err(|e| DeviceError::NoDevice(format!("default output config: {e}")))?;
        let (engine, rx, params) = AudioEngine::channel();
        let stream = build_stream(&device, &supported, rx, params, counters.clone(), initial)?;
        stream
            .play()
            .map_err(|e| DeviceError::Stream(e.to_string()))?;
        Ok((stream, engine, counters))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    rx: Receiver<AudioCommand>,
    params: Arc<ParamBank>,
    counters: SharedCounters,
    initial: Option<(
        RenderGraph,
        std::collections::BTreeMap<(String, String), u64>,
        String,
    )>,
) -> Result<cpal::Stream, DeviceError> {
    use cpal::traits::DeviceTrait;
    let channels = config.channels() as usize;
    let sample_rate = config.sample_rate().0;
    counters.sample_rate.store(sample_rate, Ordering::Relaxed);
    let stream_config: cpal::StreamConfig = config.clone().into();

    let mut state = CallbackState::new(rx, params, counters, initial);
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
                    state.fill_convert(out, channels, |s| {
                        (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                    })
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
        other => Err(DeviceError::Stream(format!(
            "unsupported sample format: {other:?}"
        ))),
    }
}

/// Callback-owned render state. Lives on the audio thread; everything in
/// here is either owned or drained via `try_recv` — the callback never
/// touches UI-thread data.
/// Callback-owned render state. Public within the crate so the transport
/// layer (and its tests) can drive the exact callback path without
/// hardware; the cpal stream owns one on the audio thread.
pub(crate) struct CallbackState {
    rx: Receiver<AudioCommand>,
    params: Arc<ParamBank>,
    cache: HashMap<String, f32>,
    graph: Option<RenderGraph>,
    delays: std::collections::BTreeMap<(String, String), u64>,
    out_node: String,
    silence: Vec<f32>,
    counters: SharedCounters,
    /// Running frame count across callbacks: the loop position (a graph
    /// swap keeps it, like [`NullBackend`]).
    rendered_frames: u64,
}

impl CallbackState {
    fn new(
        rx: Receiver<AudioCommand>,
        params: Arc<ParamBank>,
        counters: SharedCounters,
        initial: Option<(
            RenderGraph,
            std::collections::BTreeMap<(String, String), u64>,
            String,
        )>,
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
            counters,
            rendered_frames: 0,
        }
    }

    /// Drain commands without blocking, then render one device block.
    /// Uses [`render_mono_block`] — the same code path as
    /// [`NullBackend::pump`] — and feeds the shared underrun/overrun
    /// counters so the UI can observe callback health lock-free.
    fn next_block(&mut self, frames: usize) -> &[f32] {
        let mut drained = 0u64;
        while let Ok(cmd) = self.rx.try_recv() {
            drained += 1;
            match cmd {
                AudioCommand::SetParam { target, value } => {
                    self.cache.insert(target, value);
                }
                AudioCommand::SwapGraph {
                    graph,
                    delays,
                    out_node,
                } => {
                    self.graph = Some(graph);
                    self.delays = delays;
                    self.out_node = out_node;
                }
                AudioCommand::Stop => {}
            }
        }
        if drained > 1 {
            self.counters
                .overruns
                .fetch_add(drained - 1, Ordering::Relaxed);
        }
        self.params.refresh_cache(&mut self.cache);
        // let _cache = &self.cache; // params feed device params in phase 2 (audible DSP reads them)
        // Render serially in the callback (block is small); the multicore
        // schedule pays off in the offline renderer.
        self.counters
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        let start_frame = self.rendered_frames;
        self.rendered_frames += frames as u64;
        static EMPTY: Vec<f32> = Vec::new();
        match render_mono_block(
            self.graph.as_ref(),
            &self.delays,
            &self.out_node,
            frames,
            start_frame,
        ) {
            Some(buf) => {
                self.counters.blocks.fetch_add(1, Ordering::Relaxed);
                self.silence = buf;
                &self.silence
            }
            None => {
                self.counters.blocks.fetch_add(1, Ordering::Relaxed);
                self.counters.underruns.fetch_add(1, Ordering::Relaxed);
                if self.silence.len() != frames {
                    self.silence.resize(frames, 0.0);
                }
                self.silence.fill(0.0);
                if self.silence.is_empty() {
                    &EMPTY
                } else {
                    &self.silence
                }
            }
        }
    }

    fn fill(&mut self, out: &mut [f32], channels: usize) {
        if channels == 0 || out.is_empty() {
            return;
        }
        let frames = out.len() / channels;
        // Render first, then read the block back out of `self.silence`.
        // Ending the `next_block` borrow before touching `self.silence`
        // removes the old per-callback `to_vec()` snapshot copy: the only
        // copy left is the unavoidable interleave into the device buffer.
        self.next_block(frames);
        for (i, sample) in out.iter_mut().enumerate() {
            *sample = self.silence.get(i / channels).copied().unwrap_or(0.0);
        }
    }

    fn fill_convert<T>(&mut self, out: &mut [T], channels: usize, conv: impl Fn(f32) -> T) {
        if channels == 0 || out.is_empty() {
            return;
        }
        let frames = out.len() / channels;
        self.next_block(frames);
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = conv(self.silence.get(i / channels).copied().unwrap_or(0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::render::Proc;
    use crate::model::{Edge, EdgeKind, Project, Track};

    fn rig() -> (
        RenderGraph,
        std::collections::BTreeMap<(String, String), u64>,
    ) {
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
    fn both_backends_count_actual_frames_even_without_a_graph() {
        let (_, rx, params) = AudioEngine::channel();
        let mut null = NullBackend::new(rx, params);
        let (_, rx, params) = AudioEngine::channel();
        let counters = SharedCounters::new();
        let mut callback = CallbackState::new(rx, params, counters.clone(), None);
        for frames in [64, 96] {
            assert!(null.pump(frames));
            assert_eq!(callback.next_block(frames), vec![0.0; frames]);
        }
        assert_eq!(null.counters.frames.load(Ordering::Relaxed), 160);
        assert_eq!(counters.frames.load(Ordering::Relaxed), 160);
        assert_eq!(callback.rendered_frames, 160);
        assert_eq!(counters.snapshot(), (2, 2, 0));
    }

    #[test]
    fn callback_substitutes_silence_after_an_audible_graph_disappears() {
        let (engine, rx, params) = AudioEngine::channel();
        let (graph, delays) = rig();
        let mut callback = CallbackState::new(
            rx,
            params,
            SharedCounters::new(),
            Some((graph, delays, "mix".into())),
        );
        assert_eq!(callback.next_block(64), vec![0.5; 64]);
        let (graph, delays) = rig();
        engine.send(AudioCommand::SwapGraph {
            graph,
            delays,
            out_node: "missing".into(),
        });
        assert_eq!(callback.next_block(64), vec![0.0; 64]);
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
