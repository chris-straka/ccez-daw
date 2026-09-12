//! Devices-to-WASM runtime: a sandboxed delay device with render equivalence.
//!
//! Teaching note: this module answers "what would it take to run device
//! DSP as untrusted `.wasm`?" with working code instead of a seam marker
//! ([`crate::devices::wasm`] names the boundary; this module crosses it).
//! Three files, three jobs:
//!
//! - [`guest`]: the ported DSP kernel. One pure-Rust feedback delay
//!   (ported from [`crate::dsp::Delay`], the simplest kernel in
//!   `core/src/dsp`) with zero `crate::` imports, so the *same file*
//!   compiles to native (this crate) and to WASM (the `core/wasm-guest`
//!   crate includes it by path — the two can never drift; the staged
//!   sandbox bytes are zero-import `wasm32-unknown-unknown`, and the
//!   same source is also proven against `wasm32-wasip2` — see the target
//!   note on [`sandbox`] and `scripts/build-wasm-guest.sh`). Params
//!   cross the WASM boundary as `(code, f32)` pairs; audio crosses one
//!   `f32` per call.
//! - [`params`]: the parameter bridge. The universal `node:param`
//!   address grammar from `contracts/op-log-format.md` (the same grammar
//!   [`crate::engine`] applies for `ParamSet`), split with the engine's
//!   `split_once(':')` semantics, down to guest clamps. Single addressed
//!   sets refuse typos; bulk node loads skip unknown ids.
//! - [`sandbox`]: the capability-checked renderer. [`sandbox::SandboxCaps`]
//!   bound blocks, rates, and line lengths host-side; [`sandbox::Backend`]
//!   selects the in-process simulated path (default, always available)
//!   or the wasmtime `Store` path (feature `wasm-runtime`, guest bytes
//!   from `scripts/build-wasm-guest.sh`).
//!
//! What this module deliberately does *not* do (frozen-contract rule):
//! it reads the frozen [`crate::model::Node`] and registers nothing in
//! `emit.rs`, `model.rs`, or `ipc.rs` — so `bun run typegen -- --check`
//! cannot drift. It owns no realtime-graph slot yet: `WasmDevice` renders
//! offline blocks shaped exactly like the realtime callback will pass,
//! which is the integration follow-up.
//!
//! Verify: `cargo test --manifest-path core/Cargo.toml --lib wasmdevices`
//! (equivalence vs the native kernel, boundary refusals, guest self-tests;
//! the wasmtime tests need `--features wasm-runtime` plus the committed
//! guest bytes). Primer: `docs/notes/p-wasm.md`.

pub mod guest;
pub mod params;
pub mod sandbox;

pub use guest::{GuestDelay, CODE_FEEDBACK, CODE_MIX, CODE_TIME_MS, MAX_DELAY_S};
pub use params::{default_node, parse_address, sync_from_node};
pub use sandbox::{BackendKind, Error, SandboxCaps, WasmDevice};
