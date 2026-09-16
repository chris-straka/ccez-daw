//! Track C agent 1: CLAP host + sandbox.
//!
//! A plugin host does three jobs: it **loads** instruments/effects, it
//! **runs** them where a crash cannot take the session down, and it
//! **snapshots** their state so a dead plugin comes back exactly as it
//! was. This module owns all three for `core/`:
//!
//! - [`host`]: the registry. [`PluginHost`](host::PluginHost) loads a
//!   [`PluginDescriptor`](host::PluginDescriptor) (mock or CLAP), routes
//!   audio through it, and keeps its last known-good
//!   [`PluginState`](host::PluginState).
//! - [`sandbox`]: out-of-process execution. Each plugin lives in its own
//!   child process ([`worker`]); killing it only drops audio for that
//!   insert — the session survives, and `recover()` respawns the worker
//!   and restores the snapshot.
//! - [`worker`]: the child side of the wire protocol plus a built-in mock
//!   gain plugin. Real `.clap` / `*.vst3` bundles, Audio Units, and
//!   `.wasm` device modules load behind the same protocol via
//!   [`clap::ClapBackend`] / [`vst3::Vst3Backend`] / [`au::AuBackend`] /
//!   [`wasm::WasmBackend`] (mono v1 scope); the mock stays for tests/CI.
//! - [`wasm`]: the worker-side WASM device backend. One `.wasm` module
//!   per worker via `wasmtime` (feature `wasm-runtime`), speaking the
//!   mono-`f32` kernel contract with caller-owned state bytes; params
//!   cross the existing worker protocol, so host and recovery paths did
//!   not change shape.
//! - [`au`]: the real AU backend (macOS only): find + open + mono
//!   `Float32` negotiation, global-scope parameter bridging, and
//!   `AudioUnitRender` (effects pull input through a render callback)
//!   inside the worker behind the additive `LoadAu` op.
//! - [`clap`]: the real CLAP backend (`clack-host`): entry load,
//!   enumerate + select-by-id instantiate, mono process, param events,
//!   latency query; non-mono layouts refuse with `PortLayout`.
//! - [`vst3`]: the real VST3 backend (`vst3-host`): bundle load,
//!   enumerate + select-by-class instantiate, mono process, normalized
//!   params, live latency, real state chunks — plus the
//!   descriptor/discovery layer; non-mono layouts refuse with
//!   `PortLayout`.
//! - [`ara`]: the Track Q ARA document host. [`AraDocument`](ara::AraDocument)
//!   (musical context + sources with sample access + clip-mapped regions),
//!   the [`MockAraEffect`](ara::MockAraEffect) test effect proving the
//!   analyze → edit → render loop, and [`AraHost`](ara::AraHost) owning the
//!   document; real third-party binary hosting stays a named follow-up.
//!   See `docs/notes/q-formats.md`.
//! - [`chain`](crate::plugins::chain): sibling Track C surface —
//!   device-chain conventions (oversampling flag, wet/dry, chain
//!   snapshots) on the frozen `Node`/`Param` shapes. Untouched by this
//!   track; the host here feeds chains, it does not redefine them.
//! - [`latency`]: honest plugin delay compensation — per-device latency
//!   samples ([`LatencyMap`](latency::LatencyMap)), the delay-compensated
//!   render helpers ([`DelayLine`](latency::DelayLine),
//!   [`align_dry`](latency::align_dry)), and the documented model.
//! - [`scan`]: the scan-list browser model over every format's discovery
//!   ([`ScanList`](scan::ScanList)).
//!
//! Reads the frozen `Node` / `Param` / `Track` types only — adds no IPC or
//! project-schema surface, so the typegen drift gate is unaffected.

pub mod ara;
pub mod au;
pub mod chain;
pub mod clap;
pub mod host;
pub mod latency;
pub mod sandbox;
pub mod scan;
pub mod vst3;
pub mod wasm;
pub mod worker;

pub use clap::{ClapBackend, ClapError, ClapPluginInfo};
pub use vst3::{Vst3Backend, Vst3BackendClass, Vst3BackendError};
pub use host::{ClapNote, PluginDescriptor, PluginHost, PluginKind, PluginState};
pub use latency::{DelayLine, LatencyError, LatencyMap, align_dry};
pub use sandbox::{DEFAULT_WORKER_TIMEOUT, SandboxError, SandboxedPlugin};
pub use scan::{PluginFormat, ScanError, ScanList, ScannedPlugin};
