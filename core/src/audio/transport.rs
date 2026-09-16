//! Realtime transport: `engine_play` / `engine_stop` with audible output.
//!
//! Teaching note: "transport" is the DAW's play button made real. Pressing
//! play must do exactly three things — open (or reuse) an output stream
//! whose callback renders the deterministic graph, advance the musical
//! position from the tempo, and report callback health — and pressing stop
//! must undo the first. Everything else (which notes sound, how loud) is
//! the graph's job, not the transport's.
//!
//! [`TransportController`] owns that lifecycle:
//!
//! - [`TransportController::play`] opens a real cpal output stream driving
//!   [`render_mono_block`](super::device::render_mono_block) in its block
//!   callback. Calling `play` twice reuses the running stream (idempotent).
//! - [`TransportController::stop`] closes (drops) the stream, or tells the
//!   null-device pump loop to finish, and the state returns to stopped.
//! - Tempo maps to block scheduling through
//!   [`beats_for_frames`]: each rendered block advances `position_beats` by
//!   `frames * tempo / (60 * sample_rate)`, so the timeline and the audio
//!   clock agree by construction.
//! - Underrun/overrun accounting lives in
//!   [`SharedCounters`](super::device::SharedCounters), fed by both the
//!   cpal callback and the null backend, and is readable lock-free via
//!   [`TransportController::stats`]. The returned [`TransportStats::state`]
//!   is the frozen [`EngineState`](crate::model::EngineState), so the
//!   counters are exposed *through* the engine state the UI already polls.
//!
//! Headless/CI machines have no audio device. `play` treats
//! [`DeviceError::NoDevice`](super::device::DeviceError) as a normal,
//! expected condition and degrades to a null-device pump thread that runs
//! the identical render path — never a panic, never silence presented as
//! success (the backend kind is observable via [`TransportStats::backend`]).
//!
//! Link-style sync (see [`super::link`]) joins a tempo/phase session from
//! [`TransportController::with_link_bus`]. While
//! [`TransportController::set_link_enabled`] is on, `tempo()` and block
//! scheduling follow the session tempo and [`TransportStats`] reports the
//! enable flag, peer count, and session phase — no new IPC commands, and
//! the frozen `EngineState` enum is untouched, so the typegen drift gate
//! is unaffected.
//!
//! Reads no frozen types beyond `EngineState` and adds no IPC or
//! project-schema surface, so the typegen drift gate is unaffected.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::model::EngineState;

use super::device::{
    AudioBackend, AudioCommand, AudioEngine, CpalBackend, DeviceError, NullBackend, SharedCounters,
};
use super::link::{LinkBus, LinkSession};
use super::render::RenderGraph;

/// Nominal device rate used for tempo scheduling on the null path (and as
/// the fallback when the hardware rate is not yet known).
pub const DEFAULT_SAMPLE_RATE: u32 = 44100;
/// Frames per null-device pump iteration. Small enough to keep position
/// updates smooth, large enough to stay out of the way.
pub const NULL_BLOCK_FRAMES: usize = 512;

/// Beats advanced by rendering `frames` samples at `sample_rate` Hz under
/// `tempo_bpm`. This is the single tempo→schedule mapping every output path
/// uses: the cpal callback, the null pump, and the offline bounce agree on
/// musical position because they all call this.
pub fn beats_for_frames(frames: usize, sample_rate: u32, tempo_bpm: f64) -> f64 {
    if sample_rate == 0 || !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
        return 0.0;
    }
    frames as f64 * tempo_bpm / (60.0 * sample_rate as f64)
}

/// Which output is actually making sound (or standing in for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportBackend {
    /// A live cpal output stream.
    Cpal,
    /// No hardware: a pump thread renders the same path on a timer.
    /// Expected on headless/CI, never an error.
    Null,
}

/// Point-in-time transport health. `state` is the frozen engine state the
/// UI already renders; the counters ride along with it.
///
/// Link sync status rides along here too — not as new IPC commands (the
/// frozen table is untouched) but as fields on the state the UI already
/// polls: `link_enabled` + `link_peers` are the enable/peer-count surface,
/// and `tempo_bpm` follows the session tempo while enabled.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportStats {
    pub state: EngineState,
    pub backend: Option<TransportBackend>,
    pub tempo_bpm: f64,
    pub position_beats: f64,
    pub blocks: u64,
    pub underruns: u64,
    pub overruns: u64,
    /// True while the transport follows the Link session tempo/phase.
    pub link_enabled: bool,
    /// Peers currently on this transport's Link session (including self).
    pub link_peers: usize,
    /// Position inside the session cycle, in beats (`[0, quantum)`).
    /// Runs on the session clock even while stopped — Link phase is
    /// independent of the play button, like the real protocol.
    pub link_phase: f64,
}

/// A live hardware stream. `cpal::Stream` is `!Send` on some backends
/// (it borrows platform callback state), so it can never sit inside the
/// `Mutex` Tauri shares — instead a dedicated owner thread holds it alive
/// (dropping it halts audio) and parks until `stop` arrives. The
/// controller keeps only the `AudioEngine` handle plus the thread's
/// stop signal and join handle, all of which are `Send`.
struct LiveCpal {
    engine: AudioEngine,
    stop_tx: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LiveCpal {
    /// Spawn the stream owner thread: it opens the hardware stream itself
    /// (a `cpal::Stream` is `!Send` on some backends, so it is created and
    /// dropped on one thread and never moved) and reports the UI-side
    /// `AudioEngine` back over `ready`. Dropping the stream on stop halts
    /// audio.
    fn spawn(
        counters: SharedCounters,
        ready: std::sync::mpsc::Sender<Result<AudioEngine, DeviceError>>,
        stop_rx: std::sync::mpsc::Receiver<()>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            match CpalBackend::open_default_with_counters(None, counters) {
                Ok((stream, engine, _)) => {
                    if ready.send(Ok(engine)).is_err() {
                        return;
                    }
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready.send(Err(e));
                }
            }
        })
    }

    fn shutdown(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        // `engine` (and its senders) drop here; the callback already
        // drained Stop, and a disconnected channel just idles.
    }
}

struct LiveNull {
    engine: AudioEngine,
    pump: Option<std::thread::JoinHandle<()>>,
}

/// Play/stop lifecycle over one output stream. `Send` so a Tauri
/// `Mutex<AppState>` can own it; the audio callback itself never touches
/// this struct (it owns its `CallbackState`), which is what keeps the UI
/// side wait-free.
pub struct TransportController {
    inner: Mutex<Inner>,
    tempo_bits: AtomicU32,
    position_beats_bits: AtomicU64,
    counters: SharedCounters,
}

struct Inner {
    state: EngineState,
    backend: Option<TransportBackend>,
    cpal: Option<LiveCpal>,
    null: Option<LiveNull>,
    sample_rate: u32,
    link_enabled: bool,
    link: LinkSession,
}

impl std::fmt::Debug for TransportController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock().expect("transport lock");
        f.debug_struct("TransportController")
            .field("state", &inner.state)
            .field("backend", &inner.backend)
            .finish()
    }
}

impl TransportController {
    pub fn new() -> Self {
        Self::with_link_bus(&LinkBus::new(120.0))
    }

    /// Join `bus` as this transport's Link session instead of a fresh solo
    /// session. Tests use this to put two transports (or a transport and a
    /// raw peer) on one session; production uses [`TransportController::new`].
    pub fn with_link_bus(bus: &LinkBus) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: EngineState::Stopped,
                backend: None,
                cpal: None,
                null: None,
                sample_rate: DEFAULT_SAMPLE_RATE,
                link_enabled: false,
                link: bus.join(),
            }),
            tempo_bits: AtomicU32::new(120.0f32.to_bits()),
            position_beats_bits: AtomicU64::new(0.0f64.to_bits()),
            counters: SharedCounters::new(),
        }
    }

    /// Start the transport. Idempotent: a second `play` while playing
    /// reuses the running stream. Tries hardware first; on
    /// [`DeviceError::NoDevice`] degrades to the null device. A
    /// [`DeviceError::Stream`] (hardware present but unusable) is returned
    /// so the caller can surface it — but the controller stays stopped and
    /// consistent, never half-open.
    pub fn play(&self) -> Result<EngineState, DeviceError> {
        {
            let inner = self.inner.lock().expect("transport lock");
            if inner.state == EngineState::Playing {
                return Ok(EngineState::Playing);
            }
        }
        // Fresh counters per run so stats describe this run; the cpal
        // callback feeds these same counters (and the null pump clones
        // them), so both backends report through one lens.
        self.counters.blocks.store(0, Ordering::Relaxed);
        self.counters.underruns.store(0, Ordering::Relaxed);
        self.counters.overruns.store(0, Ordering::Relaxed);
        // The stream opens on its owner thread (it is `!Send` on some
        // backends); this thread blocks only until the device answers.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let thread =
            LiveCpal::spawn(self.counters.clone(), ready_tx, stop_rx);
        match ready_rx.recv() {
            Ok(Ok(engine)) => {
                let mut inner = self.inner.lock().expect("transport lock");
                inner.cpal = Some(LiveCpal {
                    engine,
                    stop_tx,
                    thread: Some(thread),
                });
                inner.backend = Some(TransportBackend::Cpal);
                inner.state = EngineState::Playing;
                Ok(EngineState::Playing)
            }
            Ok(Err(DeviceError::NoDevice(_))) => {
                let _ = thread.join();
                self.play_null();
                Ok(EngineState::Playing)
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(DeviceError::Stream(
                "audio owner thread failed to start".to_string(),
            )),
        }
    }

    /// Start the transport on the null device unconditionally. Public so
    /// tests (and headless hosts) can prove the full start/stop lifecycle
    /// without hardware; `play` calls this itself when cpal reports no
    /// device.
    /// Start the transport on the null device unconditionally. Public so
    /// tests (and headless hosts) can prove the full start/stop lifecycle
    /// without hardware; `play` calls this itself when cpal reports no
    /// device.
    pub fn play_null(&self) -> EngineState {
        let mut inner = self.inner.lock().expect("transport lock");
        if inner.state == EngineState::Playing {
            return EngineState::Playing;
        }
        // Fresh counters per transport run so stats describe this run.
        self.counters.blocks.store(0, Ordering::Relaxed);
        self.counters.underruns.store(0, Ordering::Relaxed);
        self.counters.overruns.store(0, Ordering::Relaxed);
        self.position_beats_bits
            .store(0.0f64.to_bits(), Ordering::Relaxed);

        let (engine, rx, params) = AudioEngine::channel();
        let counters = self.counters.clone();
        let rate = inner.sample_rate;
        // Sleep per block so the pump advances in roughly realtime (a
        // 512-frame block at 44100 Hz is ~11.6 ms); stats derive position
        // from blocks rendered with the same math.
        let beat_nanos = (NULL_BLOCK_FRAMES as u64 * 1_000_000_000) / rate as u64;
        let pump = std::thread::spawn(move || {
            let mut backend = NullBackend::with_counters(rx, params, counters);
            while backend.pump(NULL_BLOCK_FRAMES) {
                std::thread::sleep(std::time::Duration::from_nanos(beat_nanos));
            }
        });
        inner.null = Some(LiveNull {
            engine,
            pump: Some(pump),
        });
        inner.backend = Some(TransportBackend::Null);
        inner.state = EngineState::Playing;
        EngineState::Playing
    }

    /// Stop the transport: drop the cpal stream (audio halts when the
    /// stream drops) or tell the null pump to finish and join it.
    /// Idempotent: stopping a stopped transport is a no-op.
    pub fn stop(&self) -> EngineState {
        // Send Stop while the engine handles are still alive — the null
        // pump only exits after draining it, and dropping the senders first
        // would leave the pump spinning on a disconnected channel forever.
        {
            let inner = self.inner.lock().expect("transport lock");
            if inner.state != EngineState::Playing {
                return inner.state.clone();
            }
            if let Some(n) = inner.null.as_ref() {
                n.engine.send(AudioCommand::Stop);
            }
            if let Some(c) = inner.cpal.as_ref() {
                c.engine.send(AudioCommand::Stop);
            }
        }
        let null_handle = {
            let mut inner = self.inner.lock().expect("transport lock");
            // Shut the hardware stream down (drops it on its owner thread,
            // halting audio) before reporting stopped.
            if let Some(cpal) = inner.cpal.as_mut() {
                cpal.shutdown();
            }
            inner.cpal = None;
            let pump = inner.null.as_mut().and_then(|n| n.pump.take());
            inner.null = None;
            inner.backend.take();
            inner.state = EngineState::Stopped;
            // Finalize position from blocks actually rendered, at the
            // effective tempo (the session tempo while Link is enabled).
            let blocks = self.counters.blocks.load(Ordering::Relaxed);
            let tempo = if inner.link_enabled {
                inner.link.tempo()
            } else {
                f32::from_bits(self.tempo_bits.load(Ordering::Relaxed)) as f64
            };
            let rate = inner.sample_rate;
            let beats = blocks as f64 * beats_for_frames(1, rate, tempo);
            self.position_beats_bits
                .store(beats.to_bits(), Ordering::Relaxed);
            pump
        };
        if let Some(handle) = null_handle {
            let _ = handle.join();
        }
        EngineState::Stopped
    }

    /// Set the transport tempo in BPM. Applied to block scheduling from the
    /// next rendered block (atomic store — never blocks the callback).
    /// While Link is enabled the tempo belongs to the session: this moves
    /// the whole session (every peer follows), matching Link semantics.
    pub fn set_tempo(&self, tempo_bpm: f64) {
        if tempo_bpm.is_finite() && tempo_bpm > 0.0 {
            let clamped = tempo_bpm.clamp(1.0, 960.0);
            let inner = self.inner.lock().expect("transport lock");
            if inner.link_enabled {
                inner.link.set_tempo(clamped);
            } else {
                self.tempo_bits
                    .store((clamped as f32).to_bits(), Ordering::Relaxed);
            }
        }
    }

    /// Current effective tempo: the session tempo while Link is enabled,
    /// the local tempo otherwise.
    pub fn tempo(&self) -> f64 {
        let inner = self.inner.lock().expect("transport lock");
        if inner.link_enabled {
            inner.link.tempo()
        } else {
            f32::from_bits(self.tempo_bits.load(Ordering::Relaxed)) as f64
        }
    }

    /// Enable or disable following the Link session's tempo/phase.
    /// Enabling carries the local tempo into the session so there is no
    /// jump; disabling snapshots the session tempo back into the local
    /// tempo for the same reason. Never touches audio state.
    pub fn set_link_enabled(&self, enabled: bool) {
        let mut inner = self.inner.lock().expect("transport lock");
        if inner.link_enabled == enabled {
            return;
        }
        if enabled {
            let local =
                f32::from_bits(self.tempo_bits.load(Ordering::Relaxed)) as f64;
            inner.link.set_tempo(local);
        } else {
            let session = inner.link.tempo();
            self.tempo_bits
                .store((session as f32).to_bits(), Ordering::Relaxed);
        }
        inner.link_enabled = enabled;
    }

    pub fn link_enabled(&self) -> bool {
        self.inner.lock().expect("transport lock").link_enabled
    }

    /// Peers currently on this transport's Link session (including self).
    pub fn link_num_peers(&self) -> usize {
        self.inner.lock().expect("transport lock").link.num_peers()
    }

    /// This transport's session phase in beats (`[0, quantum)`), on the
    /// shared session clock — independent of play/stop, like Link.
    pub fn link_phase(&self) -> f64 {
        self.inner.lock().expect("transport lock").link.phase()
    }

    /// Join one more peer onto this transport's session (a handle for
    /// tests and future discovery layers — not a new transport).
    pub fn join_link_peer(&self) -> LinkSession {
        self.inner
            .lock()
            .expect("transport lock")
            .link
            .join_peer()
    }

    /// Swap the rendered graph on the live output (whatever the backend).
    /// No-op while stopped.
    pub fn swap_graph(
        &self,
        graph: RenderGraph,
        delays: std::collections::BTreeMap<(String, String), u64>,
        out_node: String,
    ) {
        let inner = self.inner.lock().expect("transport lock");
        let engine = inner
            .cpal
            .as_ref()
            .map(|l| &l.engine)
            .or_else(|| inner.null.as_ref().map(|l| &l.engine));
        if let Some(engine) = engine {
            engine.send(AudioCommand::SwapGraph {
                graph,
                delays,
                out_node,
            });
        }
    }

    /// Current engine state plus counters and clock, in one snapshot.
    /// While Link is enabled `tempo_bpm` follows the session tempo (block
    /// scheduling follows it too, so the timeline and the session agree by
    /// construction); Link status itself is visible as `link_enabled`,
    /// `link_peers`, and `link_phase` — no new IPC commands needed.
    pub fn stats(&self) -> TransportStats {
        let inner = self.inner.lock().expect("transport lock");
        let (blocks, underruns, overruns) = self.counters.snapshot();
        // Live null-path position: derive from blocks rendered with the
        // current tempo (same math the pump uses per block).
        let tempo = if inner.link_enabled {
            inner.link.tempo()
        } else {
            f32::from_bits(self.tempo_bits.load(Ordering::Relaxed)) as f64
        };
        let position_beats = if inner.state == EngineState::Playing {
            blocks as f64 * beats_for_frames(1, inner.sample_rate, tempo)
        } else {
            f64::from_bits(self.position_beats_bits.load(Ordering::Relaxed))
        };
        TransportStats {
            state: inner.state.clone(),
            backend: inner.backend,
            tempo_bpm: tempo,
            position_beats,
            blocks,
            underruns,
            overruns,
            link_enabled: inner.link_enabled,
            link_peers: inner.link.num_peers(),
            link_phase: inner.link.phase(),
        }
    }

    pub fn engine_state(&self) -> EngineState {
        self.inner.lock().expect("transport lock").state.clone()
    }
}

impl Default for TransportController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::graph::AudioGraph;
    use crate::model::{Edge, EdgeKind, Project, Track};

    fn rig() -> (RenderGraph, std::collections::BTreeMap<(String, String), u64>) {
        let mut p = Project::new("p", "Transport");
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
        let topo = AudioGraph::from_project(&p);
        let delays = topo.all_edge_delays().expect("acyclic");
        let mut g = RenderGraph::from_audio_graph(topo);
        g.set_proc("a", crate::audio::render::Proc::Constant(0.25));
        g.set_proc("b", crate::audio::render::Proc::Constant(0.25));
        g.set_proc("mix", crate::audio::render::Proc::Mix);
        (g, delays)
    }

    #[test]
    fn null_transport_start_stop_lifecycle() {
        let t = TransportController::new();
        assert_eq!(t.engine_state(), EngineState::Stopped);
        assert_eq!(t.play_null(), EngineState::Playing);
        // Idempotent: second play reuses the running pump.
        assert_eq!(t.play_null(), EngineState::Playing);
        assert_eq!(t.stats().backend, Some(TransportBackend::Null));
        // Give the pump thread a chance to render at least one block.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while t.stats().blocks == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(t.stats().blocks > 0, "null pump must render blocks");
        assert_eq!(t.stop(), EngineState::Stopped);
        // Idempotent stop.
        assert_eq!(t.stop(), EngineState::Stopped);
        assert_eq!(t.engine_state(), EngineState::Stopped);
    }

    #[test]
    fn play_degrades_gracefully_without_hardware() {
        // The real entry point: on a machine with audio hardware this opens
        // a stream; on headless/CI it falls back to the null device. Either
        // way it must return Playing and stop cleanly — never panic.
        let t = TransportController::new();
        assert_eq!(t.play().expect("play never fails headless"), EngineState::Playing);
        let stats = t.stats();
        assert_eq!(stats.state, EngineState::Playing);
        assert!(stats.backend.is_some(), "some backend must serve play");
        assert_eq!(t.stop(), EngineState::Stopped);
    }

    #[test]
    fn offline_and_stream_blocks_are_byte_identical() {
        // The null test: the same graph rendered offline (the bounce path)
        // and through the streaming backend (the audible path) must agree
        // byte for byte — they share `render_mono_block`, this proves it.
        let (g, delays) = rig();
        let frames = 256;
        let offline = g
            .render(frames, 1, &delays)
            .expect("offline renders")["mix"]
            .clone();

        let (engine, rx, params) = AudioEngine::channel();
        let mut backend = NullBackend::new(rx, params);
        engine.send(AudioCommand::SwapGraph {
            graph: g,
            delays,
            out_node: "mix".to_string(),
        });
        assert!(backend.pump(frames));
        assert_eq!(backend.rendered.len(), 1);
        assert_eq!(
            backend.rendered[0], offline,
            "stream block must equal the offline bounce block"
        );
        // A second block of constants is identical too (stateless rig).
        assert!(backend.pump(frames));
        assert_eq!(backend.rendered[1], offline);
    }

    #[test]
    fn underruns_count_silence_substitution() {
        let (engine, rx, params) = AudioEngine::channel();
        let mut backend = NullBackend::new(rx, params);
        // No graph swapped: every block substitutes silence -> underruns.
        assert!(backend.pump(64));
        assert!(backend.pump(64));
        assert_eq!(backend.rendered[0], vec![0.0; 64]);
        let (_, underruns, _) = backend.counters.snapshot();
        assert_eq!(underruns, 2, "silence substitution must count as underrun");
        // Swap a graph: rendering real audio stops the count.
        let (g, delays) = rig();
        engine.send(AudioCommand::SwapGraph {
            graph: g,
            delays,
            out_node: "mix".to_string(),
        });
        assert!(backend.pump(64));
        let (_, underruns_after, _) = backend.counters.snapshot();
        assert_eq!(underruns_after, 2, "rendered blocks must not count");
        assert_eq!(backend.rendered[2], vec![0.5; 64]);
    }

    #[test]
    fn command_bursts_count_as_overruns() {
        let (engine, rx, params) = AudioEngine::channel();
        let mut backend = NullBackend::new(rx, params);
        for i in 0..5 {
            engine.set_param("mix:volume", i as f32);
        }
        assert!(backend.pump(64));
        let (_, _, overruns) = backend.counters.snapshot();
        assert_eq!(overruns, 4, "4 commands beyond the first count as overruns");
    }

    #[test]
    fn tempo_maps_to_block_scheduling() {
        // 120 BPM at 44100 Hz: one 512-frame block is ~0.0116 s.
        let beats = beats_for_frames(512, 44100, 120.0);
        assert!((beats - 512.0 * 120.0 / (60.0 * 44100.0)).abs() < 1e-12);
        // Double tempo, double beats per block.
        assert!((beats_for_frames(512, 44100, 240.0) - 2.0 * beats).abs() < 1e-12);
        // A full second of blocks at 60 BPM is exactly one beat.
        let per_block = beats_for_frames(512, 44100, 60.0);
        let blocks_per_sec = 44100.0 / 512.0;
        assert!((per_block * blocks_per_sec - 1.0).abs() < 1e-9);
        // Degenerate inputs never poison the clock.
        assert_eq!(beats_for_frames(512, 0, 120.0), 0.0);
        assert_eq!(beats_for_frames(512, 44100, 0.0), 0.0);
        assert_eq!(beats_for_frames(512, 44100, f64::NAN), 0.0);
    }

    #[test]
    fn tempo_set_clamps_and_sticks() {
        let t = TransportController::new();
        assert_eq!(t.tempo(), 120.0);
        t.set_tempo(96.0);
        assert_eq!(t.tempo(), 96.0);
        t.set_tempo(0.0);
        assert_eq!(t.tempo(), 96.0, "non-positive tempo is ignored");
        t.set_tempo(f64::NAN);
        assert_eq!(t.tempo(), 96.0, "NaN tempo is ignored");
    }

    #[test]
    fn transport_follows_session_tempo_when_link_enabled() {
        let bus = super::super::link::LinkBus::new(120.0);
        let t = TransportController::with_link_bus(&bus);
        assert!(!t.link_enabled());
        assert_eq!(t.link_num_peers(), 1);
        t.set_link_enabled(true);
        assert!(t.link_enabled());
        // A second in-process peer moves the session; the transport follows.
        let peer = t.join_link_peer();
        assert_eq!(t.link_num_peers(), 2);
        peer.set_tempo(96.0);
        assert_eq!(t.tempo(), 96.0);
        let stats = t.stats();
        assert_eq!(stats.tempo_bpm, 96.0);
        assert!(stats.link_enabled);
        assert_eq!(stats.link_peers, 2);
        // Same shared clock: transport phase agrees with the peer's.
        assert!(
            (stats.link_phase - peer.phase()).abs() < 0.05,
            "transport and peer must agree on phase"
        );
        // Setting tempo through the transport moves the whole session.
        t.set_tempo(100.0);
        assert_eq!(peer.tempo(), 100.0);
        assert_eq!(t.stats().tempo_bpm, 100.0);
    }

    #[test]
    fn link_disable_restores_local_tempo_without_jump() {
        let t = TransportController::new();
        t.set_tempo(140.0);
        t.set_link_enabled(true);
        assert_eq!(t.tempo(), 140.0);
        t.set_tempo(90.0);
        assert_eq!(t.tempo(), 90.0);
        t.set_link_enabled(false);
        assert!(!t.link_enabled());
        assert_eq!(
            t.tempo(),
            90.0,
            "disabling keeps the last session tempo locally"
        );
        assert_eq!(t.stats().tempo_bpm, 90.0);
        assert_eq!(t.stats().link_peers, 1);
        // Local control again, session untouched by local edits.
        t.set_tempo(110.0);
        assert_eq!(t.tempo(), 110.0);
    }
}
