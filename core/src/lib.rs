//! ccez-core: project/engine truth for ccez-daw.
//!
//! Rust owns the v0 contracts. `model` holds the serde types, `ipc` holds the
//! frozen command/event table, and `emit` renders both as TypeScript (plain
//! interfaces plus Zod v4 schemas). The `typegen` binary writes
//! `ui/src/generated/`; `bun run check` fails if the committed output drifts
//! from what this crate generates.

pub mod emit;
pub mod ipc;
pub mod model;
