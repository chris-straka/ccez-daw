//! Distributed scheduling: which subgraphs run remote, on what, and what
//! happens when the remote side disappears.
//!
//! Teaching note: this module answers two halves of one question. The
//! planning half ([`sched`] partition policy + capability handshake,
//! [`sched_failover`] failover policy + offline bounce) is pure functions
//! over [`Project`](crate::model::Project) data and never touches the
//! network. The transport half ([`codec`] frames, [`partition`] graph cut,
//! [`executor`] subset renderer, [`latency`] RTT/prefetch,
//! [`transport`] TCP server + failover session) streams audio blocks over
//! localhost TCP, CPU-only, with no new dependencies.
//!
//! - [`sched`]: **partition policy** (which nodes go remote) plus the
//!   **load/capability handshake** (which endpoint takes them).
//! - [`sched_failover`]: **failover policy** (remote unreachable → render
//!   locally or abort) plus the **offline bounce** that honors it.
//! - [`transport`]: **block-streaming session** over the [`partition`] cut,
//!   with sticky local failover when the link dies.
//!
//! The plan applies through the existing [`Schedule::mark_remote`](crate::audio::schedule::Schedule::mark_remote)
//! seam, so the renderer needs no changes: a fully-local plan renders
//! exactly like v0, and any still-remote node still refuses loudly via
//! `require_all_local`.

pub mod codec;
pub mod executor;
pub mod latency;
pub mod partition;
pub mod sched;
pub mod sched_failover;
pub mod transport;

pub use codec::{CodecError, Message};
pub use executor::RemoteExecutor;
pub use latency::LatencyEstimator;
pub use partition::Partition;
pub use sched::{
    NodeCapability, PartitionPlan, PartitionPolicy, Registry, RemoteError, RemoteJob,
    RemoteResult, DEFAULT_MAX_REMOTE_LATENCY, DEFAULT_MAX_REMOTE_SHARE,
};
pub use sched_failover::{
    bounce_with_failover, BounceReport, FailoverDecision, FailoverMode, FailoverPolicy,
};
pub use transport::{RemoteServer, RemoteSession, SessionOptions, SessionStatus};
