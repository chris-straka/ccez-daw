# Track C primer: the device chain (inserts you can trust)

New to insert chains? Start here. This note teaches the three ideas behind
`core/src/plugins/chain.rs`, then maps each one to the exact function that
implements it. The frozen rules it obeys live in
`contracts/project-schema.md` (the universal `Node` + `Param` shapes and
the `Track.device_ids` order); the engine it reuses lives in
`core/src/audio/` (Track B primer: `docs/notes/track-b.md`).

## 1. A chain is an order, not a container

Most DAWs model an insert chain as its own object with slots. Here there
is no new object: the frozen `Track` already carries `device_ids` — an
ordered list — and each device is a frozen `Node` with a `params` list.
The chain *is* that order.

The map: [`chain_order`](../../../core/src/plugins/chain.rs) reads the
track's order; [`chain_devices`](../../../core/src/plugins/chain.rs)
resolves it to device nodes in that order (ids with no matching node are
skipped — the graph treats missing endpoints the same way, as
pass-through, so a half-specified chain still renders).

```rust
let devs = chain_devices(&project, "trk_lead")?; // in insert order
```

## 2. Flags and mixes are params, not schema

Oversampling and wet/dry could have been new fields on the device — a
schema change, a migration note, a drift-gate failure. Instead they are
*conventions* inside the frozen `Param` shape, exactly like Track B's
`latency_samples`:

- `oversampling`: `0.0` = off, `1.0` = on (missing = off).
  [`is_oversampled`](../../../core/src/plugins/chain.rs) reads it,
  [`set_oversampled`](../../../core/src/plugins/chain.rs) writes it
  (inserting the param when absent), and
  [`oversample_factor`](../../../core/src/plugins/chain.rs) maps it to a
  render rate (1x / 2x).
- `wet` / `dry`: per-device mix gains in `0.0..=1.0` (missing = fully
  wet). [`set_wet_dry`](../../../core/src/plugins/chain.rs) clamps instead
  of erroring; [`apply_wet_dry`](../../../core/src/plugins/chain.rs)
  (`dry_in * dry + wet_in * wet`) is the same per-frame math the realtime
  insert applies, so the unit test proves the DSP contract without
  hardware. A bypassed-feeling device is `wet = 0, dry = 1`.

```rust
set_oversampled(&mut dev, true);   // dev now renders at 2x
set_wet_dry(&mut dev, 0.0, 1.0);   // ...but passes audio straight through
```

## 3. Snapshots are presets scoped to one chain

Before a destructive tweak — reorder, retune, swap a device — freeze the
whole chain: its order plus every device's full params (values *and*
ranges, so restore is exact). [`ChainSnapshot`](../../../core/src/plugins/chain.rs)
is plain serde JSON, the same shape a preset file or op payload would
carry.

The map: [`save_snapshot`](../../../core/src/plugins/chain.rs) captures,
[`snapshot_to_json` / `snapshot_from_json`](../../../core/src/plugins/chain.rs)
carry, [`restore_snapshot`](../../../core/src/plugins/chain.rs) re-applies:
device params/names are replaced wholesale (devices missing from the
project are re-created), then the track's order is reset. Order and
devices must agree element-wise or restore errors — it never silently
drops an insert. Unknown track, unknown device on save, and torn JSON all
return [`ChainError`](../../../core/src/plugins/chain.rs), never panic.

```rust
let snap = save_snapshot(&project, "trk_lead")?; // tweak freely...
restore_snapshot(&mut project, &snap)?;          // ...then come back
```

## 4. How to verify

- `cargo test --manifest-path core/Cargo.toml plugins` — 4 tests:
  oversampling flag (defaults off, toggles, survives model JSON),
  wet/dry math (defaults, equal mix, bypass, clamping), chain snapshot
  save/restore (season → save → JSON round-trip → reorder/retune/remove
  → restore → byte-equal snapshot), and clean snapshot errors.
- `cargo test --manifest-path core/Cargo.toml` — full suite (50 tests:
  46 pre-existing + 4 new), no regressions.
- `bun run typegen -- --check` — unaffected (this track reads/writes the
  frozen types and adds none, so no drift is possible).

## 5. What Track C deliberately leaves out

- Real oversampled DSP (polyphase resampling around the flagged device)
  is future work: `oversample_factor` reports the rate, the renderer does
  not yet consume it.
- Wet/dry is per-device gain math, not latency-compensated mixing — the
  `latency_samples` compensation in `core/src/audio/graph.rs` still owns
  mix-point alignment.
- Snapshots live in memory/JSON only; wiring them to engine assets
  (`store_asset` with the `plugin` kind) or to the op log (`ParamSet` ops
  per restored value, so chain restore is undoable) is the integration
  follow-up.
- `ui/src/generated/*`, `model.rs`, and `ipc.rs` untouched.

## 6. VST3 hosting (agent 2): one factory symbol, three layers

New to VST3? A VST3 plugin is a native `*.vst3` bundle exporting one C
symbol, `GetPluginFactory`. The host calls it, enumerates the plugin
classes inside, and drives the audio ones (`Audio Module Class`). The
VST3 side lives in [`vst3.rs`](../../../core/src/plugins/vst3.rs) and is
organized in the same three verbs as the CLAP host (`host.rs`), so the
two formats converge instead of competing:

| Job | CLAP side (agent 1, owns) | VST3 side (agent 2, owns) |
|---|---|---|
| Discover | `PluginDescriptor` (registry entry) | [`Vst3Bundle::scan_dir`](../../../core/src/plugins/vst3.rs) (find `*.vst3` dirs, sorted; missing dir = empty, not an error) |
| Load | `PluginHost::load` → sandbox worker | [`Vst3Host::load_bundle`](../../../core/src/plugins/vst3.rs) → boxed [`Vst3Instance`](../../../core/src/plugins/vst3.rs) |
| Run | `process` / `set_param` over the pipe | `Vst3Instance::process` / `set_param` in-process (v1) |
| Snapshot | `PluginState { params, blob }` | `params()` + `reset()` today; the opaque `blob` (VST3 state chunk via `IComponent::getState`) is phase 2 |

Two conventions both sides share, so neither invents schema: params are
the frozen `Param` shape (VST3 `set_param` clamps to `[min, max]`, the
same rule the engine's `ParamSet` applies), and
[`as_device_node`](../../../core/src/plugins/vst3.rs) maps a loaded
plugin to a frozen `NodeKind::Device` node — which is exactly what
`chain_devices` (§1) resolves and what `PluginDescriptor.id` matches.

The map:

- [`Vst3Bundle`](../../../core/src/plugins/vst3.rs) — bundle paths per
  platform (`Contents/MacOS/<stem>`, `Contents/x86_64-linux/<stem>.so`,
  `Contents/x86_64-win/<stem>.vst3`).
- [`Vst3Host::load_module`](../../../core/src/plugins/vst3.rs) — a real
  `dlopen` round-trip: open the library, fetch `GetPluginFactory`, walk
  `countClasses`/`getClassInfo` with the byte-exact `PClassInfo` layout.
  A system library without that export returns `NoFactory` (a test does
  exactly this against libc — proof the loader is real, not a mock).
- [`DescriptorPlugin`](../../../core/src/plugins/vst3.rs) — the v1
  runnable backend behind the unstable `descriptor.json` dev/test
  format (gain effect; NOT a frozen contract, never in project files,
  never mirrored to TypeScript). [`NullVst3Plugin`](../../../core/src/plugins/vst3.rs)
  is the Track-B-`NullBackend`-pattern test double.
- New dependency: `libloading 0.8` — already in `Cargo.lock` (via
  bindgen), so no new supply-chain surface and offline builds keep
  working.

## 7. VST3: how to verify

- `cargo test --manifest-path core/Cargo.toml plugins::vst3` — 8 tests:
  `one_vst3_plugin_loads` (scan → load → sample-exact gain audio →
  param change → reset), scan sorting/ignoring, `NoModule` /
  `NoFactory` / `BadParam` / block-shape errors, silence passthrough,
  and the frozen-`Device`-node mapping (JSON round-trip).
- `cargo test --manifest-path core/Cargo.toml` — full suite
  (99 lib + 5 integration), no regressions.
- `cargo run --manifest-path core/Cargo.toml --bin typegen -- --check`
  — unaffected (`Vst3Descriptor` is deliberately unregistered in
  `emit.rs`; this track reads the frozen types and adds none).

## 8. What VST3 deliberately leaves out

- Binary instantiation (`createInstance` → `initialize` →
  `setupProcessing` → `process` with real `ProcessData`) is phase 2:
  `load_bundle` enumerates a native factory's classes first and reports
  `NoAudioEffect` with the class list, so a loadable module never
  misreports as missing. The `Library` handle must be retained past
  enumeration when that lands (v1 releases it — callers keep only
  copied-out data).
- Sandbox convergence: `Vst3Instance` is `Send` so a boxed instance can
  move behind `plugins/sandbox.rs` (agent-1-owned, read-only here)
  later. The likely shape is a VST3 backend *inside* the existing
  worker process (untrusted C-ABI code belongs behind the process
  boundary anyway), speaking the existing worker protocol — the
  `params()`/`set_param()` surface already matches what `PluginState`
  needs, with `blob` filled by phase-2 `getState`.
- `ui/src/generated/*`, `model.rs`, and `ipc.rs` untouched.

## 6. AU support: macOS units behind a flag (agent 3)

New to Audio Units? An AU plugin is a macOS system component — a
`.component` bundle the OS registers — named by three FourCC codes packed
as `u32`s: type (`aufx` = effect, `aumu` = instrument), subtype (the
specific plugin), manufacturer (`appl` = Apple). The host finds components
through `AudioComponentFindNext` and opens them with
`AudioComponentInstanceNew`. That find-then-open pair is the whole v1
seam in [`au.rs`](../../../core/src/plugins/au.rs); render callbacks and
parameter bridging are the phase-2 follow-up on the [`AuInstance`] handle.

The map: [`fourcc` / `fourcc_string`](../../../core/src/plugins/au.rs)
pack the codes (`0x61756678` ⇄ `"aufx"`); [`scan_system`] lists installed
bundles from the three system domains (name from `Info.plist`
`CFBundleName`, else the bundle stem; unreadable folders scan empty, never
error); [`instantiate`] opens the first match and [`AuInstance`]'s `Drop`
disposes it. [`instantiate_if_flag`] is the call the future audio path
uses: unless `CCEZ_ENABLE_AU=1` is set, loading returns
`DisabledByFlag` — v1 ships macOS with CLAP/VST3 first and keeps AU
deferred by design, not by failure.

### AU hosting-crate evaluation (the evidence gap, closed)

| Candidate | Version (checked Sep 2026) | Verdict |
|---|---|---|
| `rack` (sinkingsugar) | 0.4.8 — AU "production-ready", VST3 working, ~300 dl/mo, Rust+C++/ObjC++ via `rack-sys`, API stabilizing, CLAP planned | Too heavy for v1: native build layer in the workspace, tiny adopter base, unstable API |
| `audiounit` (doom-fish) | 0.3.1 — safe AU/AVAudioUnit bindings, MSRV 1.76, `build.rs` + Swift bridge, docs.rs targets Apple-only | macOS-only dep would need target-gating in `check.yml`'s all-desktop matrix; Swift bridge adds toolchain risk |
| `audio-unit` on crates.io | 404 — no such crate | Eliminated |
| `audiotoolbox` | Low-level bindings only (no host workflow) | No advantage over direct FFI |
| **Hand-rolled FFI (chosen)** | 3 extern fns against the always-present `AudioToolbox` framework, zero new deps | Whole v1 seam in ~60 lines; revisit `audiounit` when parameter/render bridging lands |

### How to verify

- AU tests live inline in `au.rs` (10 tests). The workspace crate is
shared with in-flight sibling tracks; if `cargo test --manifest-path
core/Cargo.toml plugins::au` is blocked by their files, the same file
passes standalone (copied verbatim into a scratch crate with only
`serde`/`serde_json`): 10/10 pass, including
`first_registered_au_instantiates` — a real Apple AU found by wildcard,
opened in-process, and disposed on drop — and `system_scan_finds_bundled_units`.
- `bun run typegen -- --check` — unaffected (new types only, none frozen).

## 7. CLAP host + sandbox: a crash kills the insert, never the session (agent 1)

New to plugin hosting? A plugin is untrusted native code sharing your
address space — one bad pointer and the whole DAW goes down with it.
The fix is an old OS idea: put each plugin in its *own process*. This
section teaches the four ideas behind `core/src/plugins/{host,sandbox,
worker}.rs` plus the worker binary `core/src/bin/plugin-worker.rs`.

### 7a. nih-plug is for writing plugins; hosting is clack (evaluation first)

The brief said to evaluate the nih-plug community fork before building,
via docs — the finding changed the dependency plan:

- `robbert-vdh/nih-plug` is in **maintenance mode**; the recommended
  community fork is `BillyDM/nih-plug` (Codeberg). But nih-plug is a
  plugin-*authoring* framework (`nih_export_clap!()` writes plugins) —
  it cannot load plugins into a DAW.
- Hosting CLAP in Rust is `prokopyl/clack` (`clack-host`:
  `PluginEntry::load` + `PluginInstance::new` + `HostHandlers`;
  feature-complete, still evolving).

Decision: v1 adds **zero new cargo dependencies** and proves the
sandbox + snapshot + recovery machinery with a mock backend. The seam
is honest and named: loading a `PluginKind::Clap` descriptor returns
[`HostError::ClapUnimplemented`](../../../core/src/plugins/host.rs) —
the same mark-and-refuse pattern Track B used for remote DSP. A future
`ClapBackend` (clack-host + libloading) runs *inside the worker
process* — untrusted C-ABI code must live behind the process boundary
anyway — and speaks the existing worker protocol, so host and recovery
code do not change.

### 7b. The host keeps truth; the child does DSP

[`PluginHost`](../../../core/src/plugins/host.rs) is a registry: `load`
spawns one worker process per plugin, `process` renders a mono block
through it, `set_param` tunes it. Beside each live child the host holds
a [`PluginState`](../../../core/src/plugins/host.rs) — params plus the
plugin's opaque blob (a CLAP state chunk the host never interprets;
round-trip exactness is the only contract). `PluginState` is serde JSON
on purpose: the same bytes ride in engine assets (`plugin` kind) and
portable bundles later.

```rust
let mut host = PluginHost::new(44100.0, PluginHost::default_worker_bin());
host.load(PluginDescriptor::mock("lead", "Lead"))?;
host.set_param("lead", "gain", 0.5)?;
let out = host.process("lead", &[1.0, -1.0])?; // [0.5, -0.5]
```

### 7c. The wire is one JSON line each way

The worker ([`worker.rs`](../../../core/src/plugins/worker.rs), run by
the `plugin-worker` binary) reads [`WorkerRequest`] lines on stdin and
writes one [`WorkerResponse`] line per request on stdout: `init`,
`set_param`, `get_state`, `set_state`, `process`, `shutdown`. Newline
framing keeps the parser trivial and hand-debuggable
(`echo '{"op":"init","sample_rate":44100}' | plugin-worker`). The
built-in mock DSP is a gain plugin — deliberately dumb, so tests prove
*transport*, not DSP cleverness.

### 7d. Death is detected three ways; recovery replays last-known-good

[`SandboxedPlugin`](../../../core/src/plugins/sandbox.rs) owns the
child plus both pipes, and one invariant: `last_good` always holds the
state from the most recent *successful* mutating call (spawn baseline,
`set_param`, `snapshot`, `restore`). A crash can only lose calls that
never completed. Death surfaces three ways — write fails (broken
pipe), read hits EOF, or `alive()` sees the child reaped — all as
`SandboxError::Crashed`, and `recover()` respawns the child and pushes
`last_good` back in. Every failure is scoped to *that plugin*: the
registry never poisons, siblings keep rendering.

```rust
host.kill("lead")?;                              // the segfault stand-in
assert!(host.process("lead", &[1.0]).is_err());  // this insert drops...
assert_eq!(host.process("rhythm", &[1.0])?, vec![1.0]); // ...the session plays on
let restored = host.recover("lead")?;            // respawn + restore
assert_eq!(host.process("lead", &[1.0])?, vec![0.5]); // gain 0.5 is back
```

### How to verify (agent 1)

- `cargo test --manifest-path core/Cargo.toml plugins::host
  plugins::sandbox plugins::worker` — 10 tests: mock load/process/
  unload, duplicate/unknown-id errors, CLAP refusal leaving no
  half-loaded entry, state JSON round-trip, protocol single-line
  framing, mock gain math + blob state, spawn/snapshot/process,
  kill → `Crashed` → recover byte-exact (params *and* blob), and
  idempotent recover on a live worker.
- `cargo test --manifest-path core/Cargo.toml` — full suite, 99 passed,
  0 failed (46 pre-existing Track A/B + siblings' + 10 new).
- `bun run typegen -- --check` — `ok project.ts`, `ok ipc.ts`
  (new module reads the frozen types and adds none).

### What agent 1 deliberately leaves out

- Real `.clap` loading (`ClapUnimplemented` seam; clack-host lives in
  the worker process when it lands).
- Hang detection: reads block, so a worker that is *alive but mute*
  stalls the caller — deadline + kill is the phase-2 follow-up. A
  worker that *dies* (the segfault case owned here) is caught on the
  very next call.
- Audio over the pipe is one JSON round-trip per block — fine for v1;
  phase 2 moves buffers to shared memory behind the same boundary.
- Realtime-graph wiring (`Proc::Plugin` variant feeding sandboxed
  inserts from the Track B renderer) is the integration follow-up; the
  `process(&[f32]) -> Vec<f32>` shape already matches it.
- `ui/src/generated/*`, `model.rs`, `ipc.rs` untouched.
