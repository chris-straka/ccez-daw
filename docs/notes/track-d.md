# Track D primer: native devices you can run anywhere (agent 1)

New to device DSP? Start here. This note teaches the four ideas behind
`core/src/devices/`, then maps each one to the exact function that
implements it. The frozen rules it obeys live in
`contracts/project-schema.md` (the universal `Node` + `Param` shapes and
the `Track.device_ids` order); the chain conventions it reuses live in
`core/src/plugins/chain.rs` (Track C primer: `docs/notes/track-c.md`).

## 1. A kernel is samples in, samples out — nothing else

Every native device sound is a pure-Rust function over mono `f32`
slices in [`kernel.rs`](../../../core/src/devices/kernel.rs): gain,
one-pole lowpass/highpass, feedback-comb delay, soft-clip distortion.
Three rules keep them portable (native today, WASM tomorrow, realtime
when wired):

- `output.len() == input.len()` or `DeviceError::BadBlock`. Empty blocks
  are a legal no-op.
- No allocation inside `process`. State the kernel needs across blocks
  (yesterday's filter output, the delay line) lives in an explicit
  `*State` struct the *caller* owns and threads through — that is the
  whole "filters remember" mechanism, and why kernels never touch
  `model` types.
- Params arrive as plain scalars, already resolved from the frozen
  `Node` one layer up. Clamp, don't error (`feedback` clamps to 0.95 —
  1.0 would circulate forever; same clamp-not-error rule as `set_wet_dry`
  and the engine's `ParamSet`).

The map: [`gain_process`](../../../core/src/devices/kernel.rs),
[`lowpass_process` / `HighpassState`](../../../core/src/devices/kernel.rs)
(`a = 1 − e^(−2π·fc/sr)`; DC passes, Nyquist is crushed, and the test
pins both directions), [`delay_process`](../../../core/src/devices/kernel.rs)
(`y[n] = x[n] + fb·y[n−D]`; delay 0 bypasses the line because instant
feedback is a causality violation),
[`distortion_process`](../../../core/src/devices/kernel.rs)
(`(1+k)·x/(1+k·|x|)` — unity at zero drive, bounded at ten).

```rust
let mut s = LowpassState::default();
lowpass_process(1000.0, 44100.0, &mut s, &input, &mut output)?;
```

## 2. The device type is a param, not a schema field

The frozen `Node` has no "type" field — just `id`, `kind`, `name`,
`params` — and this track may not add one (schema change = migration
note). So the class rides *inside* the frozen `Param` shape as a numeric
code in the `device_class` param, exactly the way Track C's oversampling
flag rides in `"oversampling"`. [`classify`](../../../core/src/devices/class.rs)
reads it; [`instantiate`](../../../core/src/devices/class.rs) writes it
(tag plus class defaults — the single place reusable-device defaults
live). Codes are append-only: renumbering one would orphan saved
projects.

Nodes that predate this track (hand-built, or owned by a plugin host)
carry no code and classify as `Foreign`, which the renderer treats as
pass-through — structure survives nodes this track does not understand,
the same tolerance the graph shows half-specified chains.

```rust
let mut node = instantiate(DeviceClass::Lowpass, "lp1", "Softener");
set_param_value(&mut node, "cutoff", 400.0)?; // clamped to range; unknown ids error
assert_eq!(classify(&node), DeviceClass::Lowpass);
```

## 3. A rack says what a flat chain cannot

`Track.device_ids` says "A then B". It cannot say "A then (B beside C,
mixed back)" or nest groups inside groups. That structure lives in the
[`Rack`](../../../core/src/devices/rack.rs) sidecar — a plain serde
struct riding *alongside* the project (the Track G `VcaGroup`
precedent), never in it. Empty rack = flat chain: racks add, never
reinterpret.

- Nesting: a `Container`-class device id maps to a child list in
  `Rack.containers`. Containers nest by referencing other container ids;
  cycles are `RackError::BadRack`, never a hang (a `visiting` set,
  the same guard graph traversals use).
- Splits: [`Split`](../../../core/src/devices/rack.rs) renders every
  branch from the same input and sums them with per-branch gains
  (parallel squash beside dry — the idea; multiband DSP is later).
- The container's own wet/dry still applies to its subtree (a rack bus
  fades everything inside it), and missing device nodes render as
  pass-through, so half-specified racks still sound.

[`render_rack`](../../../core/src/devices/rack.rs) is the offline
renderer: serial chain with recursive expansion, per-device wet/dry
from the Track C conventions (`dry·dry_in + wet·wet_in`, same math the
realtime insert will apply), 2x oversampling behind the chain's flag
(naive linear resample — the polyphase upgrade is the same follow-up
Track C already names; rate-independent kernels are bit-identical
through it, and a test pins that), and an explicit
[`RackState`](../../../core/src/devices/rack.rs) threaded through so a
delay's echo lands in the *next* block (a test renders two blocks and
asserts exactly that).

```rust
rack.containers.insert("bus".into(), vec![
    RackNode::Device("comp".into()),
    RackNode::Split(Split { id: "par".into(), branches: vec![vec![RackNode::Device("sat".into())]], gains: vec![0.5] }),
]);
let mut state = RackState::default(); // one per rendered track, kept across blocks
let out = render_rack(&project, &rack, &mut state, "trk_lead", &input, 44100.0)?;
```

## 4. One knob drives many params; presets freeze one device

A [`Macro`](../../../core/src/devices/rack.rs) maps one 0..=1 knob onto
many params at once (`min + value·(max − min)`, clamped to each param's
own range, later macros win). Overrides are render-time only — the
project's stored params are never rewritten, so a macro tweak cannot
corrupt a preset. Macro values live only in the sidecar; wiring them to
`ParamSet`/automation is the integration follow-up.

A [`DevicePreset`](../../../core/src/devices/rack.rs) is one reusable
device as JSON: class, name, full params (values *and* ranges, the
chain-snapshot rule, so restore is exact). `capture` refuses foreign
nodes (a preset must know its DSP); `instantiate` stamps out a fresh id
with byte-exact params; unknown class names error instead of silently
becoming a different device. The save/reload test captures a tuned
lowpass, JSON-round-trips it, restores into a fresh project, and renders
— same DSP, zero drift.

## 5. WASM is a seam, not a dependency

Third-party DSP as `.wasm` modules is phase 2:
[`load_wasm_module`](../../../core/src/devices/wasm.rs) always returns
`WasmError::WasmUnimplemented` today — the same mark-and-refuse pattern
Track B used for remote DSP and Track C for real CLAP bundles. The
evaluation is recorded on [`WasmNote`](../../../core/src/devices/wasm.rs):
`wasmtime` is the standard embedding pick, v1 adds zero new cargo deps
(the offline-friendly rule the CLAP host kept), and the kernels were
written to the future call convention on purpose (slices in, slices
out, caller-owned state), so the seam is a loader plus a memory copy —
never a redesign.

## 6. How to verify

- `cargo test --manifest-path core/Cargo.toml --lib devices` — 18
  tests: kernel math (gain, both filters both directions, delay decay,
  distortion bounds, ragged-block errors), class tag round-trips
  (including frozen-model JSON survival) plus foreign tolerance,
  serial/nested/split/macro renders, preset save → reload → identical
  render, oversample bit-identity on constants, cross-block echo state,
  pass-through of foreign/missing devices, and clean errors (unknown
  track, bad rate, container cycle, torn JSON, macro out of range).
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions (117 pre-existing green at build time).
- `cargo run --manifest-path core/Cargo.toml --bin typegen -- --check`
  — unaffected (this track reads the frozen types and registers none,
  so no drift is possible).

## 7. What Track D agent 1 deliberately leaves out

- Realtime-graph wiring (`Proc::Device` variants feeding rack leaves
  from the Track B renderer) is the integration follow-up;
  `render_rack`'s block shape already matches it. The oversample scratch
  buffers are per-call today — the realtime path hoists them.
- Macro knobs are sidecar-only (no `ParamSet` address, no automation
  lane); filter cutoff has no resonance/Q; delay has no tempo sync or
  stereo — each is one param plus kernel math away, no structural change.
- `Rack` persistence to engine assets (`preset` kind) and the op log
  (macro moves as undoable ops) rides the Track A asset/log paths
  untouched here; `Rack::to_json`/`from_json` is the shape they will carry.
- `ui/src/generated/*`, `model.rs`, and `ipc.rs` untouched.
