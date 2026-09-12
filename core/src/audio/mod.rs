//! Track B: audio engine + universal node graph.
//!
//! The DAW renders sound from the same [`Project`](crate::model::Project)
//! the UI edits. This module owns everything on the sound side of that
//! boundary:
//!
//! - [`graph`]: one routing model for audio, MIDI, modulation, and
//!   sidechain edges (built straight from the frozen `Edge` list — the
//!   mixer is just a view over the `Audio` edges).
//! - [`schedule`]: multicore execution levels plus the phase-2 remote-node
//!   seam (hook only, unimplemented by design).
//! - [`render`]: deterministic offline renderer used by tests and by the
//!   cpal callback. Sample-exact: 1 thread and N threads produce identical
//!   output.
//! - [`device`]: cpal I/O plus the lock-free UI→audio handoff. The frontend
//!   never blocks the audio thread: it only ever `send()`s owned commands
//!   into a channel the callback drains with `try_recv`.
//!
//! Reads the frozen `Node` / `Edge` / `Track` types only — adds no new IPC
//! or project-schema surface, so the typegen drift gate is unaffected.

pub mod device;
pub mod graph;
pub mod render;
pub mod schedule;

pub use device::{AudioBackend, AudioCommand, AudioEngine, CpalBackend, NullBackend};
pub use graph::{AudioGraph, GraphError, LATENCY_PARAM};
pub use render::{Proc, RenderGraph};
pub use schedule::{Placement, RemoteRef, Schedule, ScheduleError};
