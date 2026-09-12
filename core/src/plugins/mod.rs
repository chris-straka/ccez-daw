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
//!   gain plugin. Real `.clap` binaries attach behind the same protocol
//!   (see the `ClapUnimplemented` seam in [`host`]).
//! - [`chain`](crate::plugins::chain): sibling Track C surface —
//!   device-chain conventions (oversampling flag, wet/dry, chain
//!   snapshots) on the frozen `Node`/`Param` shapes. Untouched by this
//!   track; the host here feeds chains, it does not redefine them.
//!
//! Reads the frozen `Node` / `Param` / `Track` types only — adds no IPC or
//! project-schema surface, so the typegen drift gate is unaffected.

pub mod au;
pub mod chain;
pub mod host;
pub mod sandbox;
pub mod vst3;
pub mod worker;

pub use host::{ClapNote, PluginDescriptor, PluginHost, PluginKind, PluginState};
pub use sandbox::{SandboxError, SandboxedPlugin};
