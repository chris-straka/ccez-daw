//! ccez-core: project/engine truth for ccez-daw.
//!
//! Rust owns the v0 contracts. `model` holds the serde types, `ipc` holds the
//! frozen command/event table, and `emit` renders both as TypeScript (plain
//! interfaces plus Zod v4 schemas). The `typegen` binary writes
//! `ui/src/generated/`; `bun run check` fails if the committed output drifts
//! from what this crate generates.

pub mod ai;
pub mod bounce;
pub mod comp;
pub mod devices;
pub mod dsp;
pub mod audio;
pub mod automation;
pub mod branch;
pub mod emit;
pub mod engine;
pub mod ipc;
pub mod library;
pub mod midi;
pub mod mixer;
pub mod model;
pub mod repair;
pub mod timepitch;
pub mod plugins;
pub mod timeline;
