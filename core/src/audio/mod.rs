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
pub mod input;
pub mod graph;
pub mod link;
pub mod live;
#[cfg(feature = "link-net")]
pub mod link_net;
pub mod render;
pub mod schedule;
pub mod transport;

pub use device::{
    render_mono_block, AudioBackend, AudioCommand, AudioEngine, CpalBackend, NullBackend,
    SharedCounters,
};
pub use input::{
    list_input_devices, monitor_levels, open_input_or_null, AnyInput, CpalInput, InputDeviceInfo,
    InputSelect, MonitorLevels, NullInput, TakeCapture,
};
pub use link::{LinkBus, LinkSession, DEFAULT_QUANTUM};
#[cfg(feature = "link-net")]
pub use link_net::{UdpLinkNode, HEARTBEAT, LINK_UDP_MAGIC, WIRE_LEN};
pub use transport::{
    beats_for_frames, TransportBackend, TransportController, TransportStats, DEFAULT_SAMPLE_RATE as TRANSPORT_SAMPLE_RATE,
    NULL_BLOCK_FRAMES,
};
pub use graph::{AudioGraph, GraphError, LATENCY_PARAM};
pub use live::{live_graph, loop_beats, MAX_LIVE_LOOP_BEATS, MIN_LIVE_LOOP_BEATS};
pub use render::{Proc, RenderGraph};
pub use schedule::{Placement, RemoteRef, Schedule, ScheduleError};
