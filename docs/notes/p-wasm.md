# Perf primer: devices-to-WASM runtime (agent 1)

New to sandboxed DSP? Start here. This note teaches the three ideas
behind `core/src/wasmdevices/` — a wasmtime sandbox that runs a ported
device kernel with render-equivalence to native — then maps each one to
the exact function that implements it. The frozen rules it obeys live in
`contracts/op-log-format.md` (the universal `node:param` address) and
`contracts/project-schema.md` (the `Node` + `Param` shapes, read but
never extended); the seam this work crosses is named in
[`wasm.rs`](../../../core/src/devices/wasm.rs) (phase-2 marker) and the
kernel contract it inherits in
[`kernel.rs`](../../../core/src/devices/kernel.rs) (slices in, slices
out, caller-owned state, clamp-don't-error).

## 1. One file, two targets: the ported guest kernel

The port target is [`Delay`](../../../core/src/dsp/delay.rs), the
simplest kernel in `core/src/dsp`: a feedback delay line (millisecond
time, feedback, wet/dry mix) whose block loop is integer index
arithmetic plus `f32` multiply-add — no per-sample transcendentals, so
it evaluates identically on every target. The port is
[`guest.rs`](../../../core/src/wasmdevices/guest.rs)
([`GuestDelay`](../../../core/src/wasmdevices/guest.rs)), kept
op-for-op identical to the native loop (same clamp order, same
`(pos + len - d) % len` tap), which is why the equivalence gate below
asserts **exact sample equality**, not a tolerance: any drift is a port
bug, by construction.

The anti-drift mechanism is structural, not procedural: `guest.rs` has
zero `crate::` imports, and the WASM crate
[`core/wasm-guest`](../../../core/wasm-guest/src/lib.rs) includes it
**by path** (`#[path = "../../src/wasmdevices/guest.rs"]`), so the
native reference and the sandboxed guest are the same file. One
addition over the native kernel:
[`step`](../../../core/src/wasmdevices/guest.rs) factors the loop body
into one-sample-in/one-sample-out — the shape the WASM boundary speaks
— and a test pins `step`-loop == `process` == native.

```rust
let mut dev = WasmDevice::new_simulated("dev_dly", 48_000.0)?;
dev.set_param_address("dev_dly:feedback", 0.5)?;
dev.process(&input, &mut output)?; // bit-identical to dsp::Delay
```

## 2. Params cross on the universal address, not a new grammar

The DAW has exactly one param-address grammar — `ParamSet` targets are
`node:param` (`contracts/op-log-format.md`), applied by the engine with
`split_once(':')`. The bridge in
[`params.rs`](../../../core/src/wasmdevices/params.rs) speaks that same
grammar in both directions:
[`parse_address`](../../../core/src/wasmdevices/params.rs) splits with
the engine's semantics (no colon or an empty half is `BadAddress`;
extra colons stay in the param half, exactly as the engine leaves
them), and [`sync_from_node`](../../../core/src/wasmdevices/params.rs)
hydrates the guest from a frozen `Node` — known ids apply with guest
clamps, unknown ids skip, the same bulk-load tolerance native
`from_node` shows. Single addressed sets go the other way: unknown ids
refuse (`UnknownParam`, state untouched), because a typo'd knob must
surface — the [`set_param_value`](../../../core/src/devices/class.rs)
rule. The guest's canonical node is the native delay's own
`default_node`, so a project swaps native for sandboxed with zero param
edits. Strings never cross into WASM: params travel as `(code, f32)`
pairs (`CODE_TIME_MS`/`CODE_FEEDBACK`/`CODE_MIX`, append-only like the
device-class codes).

## 3. The sandbox is three layers, cheapest first

[`sandbox.rs`](../../../core/src/wasmdevices/sandbox.rs) enforces
untrusted-DSP containment before the guest ever sees a sample:

1. **Capabilities** ([`SandboxCaps`](../../../core/src/wasmdevices/sandbox.rs)):
   block-length caps, sample-rate range, delay-line ceiling — checked
   host-side, so absurd rates (`inf`, `0`) never reach allocation the
   way native `Delay::new(inf)` would. Refusals are errors
   (`CapExceeded`, `BadBlock`, `BadAddress`, `UnknownDevice`,
   `UnknownParam`) — never traps, so a bad knob cannot abort a render.
2. **Narrow API**: init / set-param / reset / one-sample. No shared
   buffers, no pointers, no strings.
3. **Isolation** ([`Backend::Wasmtime`](../../../core/src/wasmdevices/sandbox.rs),
   feature `wasm-runtime`): the guest runs in a wasmtime `Store`
   behind typed `Func`s (`Engine` + `Module` + empty-`Linker` +
   `Store`, per the wasmtime embedding docs), with optional
   fuel metering — an infinite guest loop becomes a caught trap, and a
   test proves it with `fuel_per_block: Some(1)`. The default
   [`Backend::Simulated`](../../../core/src/wasmdevices/sandbox.rs)
   runs the identical guest source in-process through the same narrow
   API; the equivalence test proves the paths agree.

Target note (inspected, not assumed): the same source compiles to
`wasm32-wasip2`, but there rustc emits a Component Model component
(magic `\0asm` + version `0d 00`) whose std pulls WASI imports
(clocks/cli/io — check with
`strings delay_guest.wasm | grep wasi:`). Hosting that needs a full
WASI context, which widens exactly the aperture this track keeps at
zero imports — so the staged, tested bytes are the zero-import
`wasm32-unknown-unknown` core module (empty-`Linker` instantiation is
the proof), the script also proves the wasip2 build, and the
component-API + WASI-context wiring is the documented follow-up.

## 4. How to verify

- `cargo test --manifest-path core/Cargo.toml --lib wasmdevices` — 7
  tests, no features: impulse + sine render-equivalence vs the native
  kernel across a 5-point param grid (exact equality), mid-stream
  addressed sets steering both renders together, `step` == `process`,
  code/id agreement, and every boundary refusal (ragged blocks,
  over-cap blocks, bad addresses, wrong device id, unknown param with
  state untouched, absurd rates, reset clearing echoes).
- `scripts/build-wasm-guest.sh` — builds both targets, stages
  `core/src/wasmdevices/testdata/delay_guest.wasm` (committed, so the
  next gate is hermetic).
- `cargo test --manifest-path core/Cargo.toml --features wasm-runtime --lib wasmdevices` —
  9 tests: the 7 above plus the real guest matching native
  sample-for-sample through wasmtime and the fuel-trap proof.
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions (270 green at build time).
- `cargo run --manifest-path core/Cargo.toml --bin typegen -- --check`
  — unaffected (this track reads the frozen types and registers none,
  so no drift is possible). `wasmtime` is an *optional* dependency
  behind `wasm-runtime`: default builds download and compile nothing
  new.

## 5. What this track deliberately leaves out

- Realtime-graph wiring (a `Proc::WasmDevice` leaf fed by the Track B
  renderer): `WasmDevice::process` already takes callback-shaped
  blocks; per-sample wasmtime calls are the validation aperture, not
  the performance one — a shared-memory block API is the production
  follow-up, with the pooling allocator for multi-instance polyphony
  (one instance per module instantiation today).
- More kernels: only delay is ported. Eq/comp/reverb/synth follow the
  same recipe (port to `guest.rs`, extend the code map, extend the
  grid) — comp and friends need a tolerance-based gate, since
  `exp`/`powf`/`log10` may differ in the last ulp across targets.
- The wasip2 component-API host path (WIT world + `wasmtime-wasi`
  context) for the WASI-importing build; the mixing-desk side
  (`ParamSet` ops and automation lanes driving `node:param` addresses
  on sandboxed devices) rides the untouched engine path.
- `ui/src/generated/*`, `model.rs`, `ipc.rs`, and `devices/wasm.rs`
  untouched — the old seam marker still names phase 2; this module is
  phase 2 arriving alongside it, not a rewrite of it.
- Worker-process hosting of `.wasm` modules (the plugins-side arrival:
  `core/src/plugins/wasm.rs`, `LoadWasm` op, `PluginKind::Wasm`) — see
  `docs/notes/track-c.md` §11. This primer's in-process `Store` path
  stays the DSP-isolation reference; the worker path reuses its pin
  (`wasmtime` 48.0.2), its zero-import aperture, and its per-sample
  mono-`f32` contract.
