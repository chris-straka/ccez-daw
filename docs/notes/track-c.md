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

## 8. What VST3 deliberately leaves out (pre-binary-hosting)

- Binary instantiation (`createInstance` → `initialize` →
  `setupProcessing` → `process` with real `ProcessData`) is phase 2:
  `load_bundle` enumerates a native factory's classes first and reports
  `NoAudioEffect` with the class list, so a loadable module never
  misreports as missing. The `Library` handle must be retained past
  enumeration when that lands (v1 releases it — callers keep only
  copied-out data). **Update: landed — see §9.** `load_bundle` keeps its
  enumerate-first behavior for discovery; audio now goes through
  `Vst3Backend` + the `LoadVst3` worker op.
- Sandbox convergence: `Vst3Instance` is `Send` so a boxed instance can
  move behind `plugins/sandbox.rs` (agent-1-owned, read-only here)
  later. The likely shape is a VST3 backend *inside* the existing
  worker process (untrusted C-ABI code belongs behind the process
  boundary anyway), speaking the existing worker protocol — the
  `params()`/`set_param()` surface already matches what `PluginState`
  needs, with `blob` filled by phase-2 `getState`. **Update: landed as
  predicted** — `Vst3Backend` lives in the worker behind `LoadVst3`, and
  the blob is a real `getState` chunk (§9).
- `ui/src/generated/*`, `model.rs`, and `ipc.rs` untouched.

## 9. VST3 binary hosting: real audio through `vst3-host` (follow-up)

Phase 2 from §8 landed following the CLAP pattern exactly (`clap.rs` +
`clap-gain` fixture + `LoadClap` op + latency reporting): `vst3.rs` now
holds a [`Vst3Backend`](../../../core/src/plugins/vst3.rs) driven by the
additive `LoadVst3` worker op, and `core/vst3-gain/` is the deterministic
test fixture. `PluginKind::Vst3` + `PluginDescriptor::vst3` ride the
unchanged host/sandbox/recovery path; mock, CLAP, and AU/ARA seams are
behavior-untouched.

### 9a. Host-crate evaluation (verdict first)

| Candidate | Version (checked Sep 2026) | Verdict |
|---|---|---|
| `vst3-host` (HelgeSverre/rust-vst3-host, MIT) | 0.9.0 — `Vst3Host::builder` + `load_plugin` + `set_parameter` + `process_audio` + `latency_samples` + `save_state`/`load_state` over the genuine Steinberg COM ABI (vendored `vst3` bindings, no Steinberg SDK) | **Chosen.** `default-features = false`: no cpal backend, no helper binaries — isolation stays in our worker process, audio stays offline. Only new non-dev deps are its needs (`vst3`, `thiserror`, `rtrb`, `crossbeam-queue`, macOS `objc2` stack); all resolve offline from `Cargo.lock`. |
| `rack` (sinkingsugar) 0.4.8 | Hosts VST3, but native C++/ObjC++ build layer + stabilizing API (per the AU evaluation in §6) | Too heavy for v1, same reason as before. |
| Hand-rolled COM FFI | Already proven for *discovery* (`load_module` factory enumeration stays) | Rejected for *audio*: re-implementing `setupProcessing`/`ProcessData` against every vendor's quirks is exactly what the crate owns now. |

One fixture-side gotcha, recorded so nobody re-learns it: on macOS the
crate resolves entry points through CoreFoundation
(`CFBundleGetFunctionPointerForName(bundle, "bundleEntry")`), which is
case-sensitive — the bundle must export lowercase `bundleEntry` /
`bundleExit` alongside the canonical `BundleEntry`. The `vst3` crate's
`gain.rs` example only exports the capitalized spells, so the fixture
adds both.

### 9b. The map

- [`Vst3Backend::load`](../../../core/src/plugins/vst3.rs) —
  `Vst3Host::builder` (session rate, 1024-frame blocks, 1 in / 1 out
  channel) + `load_plugin(bundle)` + mono bus-layout check + param
  discovery (names/ids/normalized ranges/seed values) +
  `start_processing` + baseline `save_state` blob.
- [`Vst3Backend::process`](../../../core/src/plugins/vst3.rs) — mono
  blocks (chunked at 1024 frames) with the full current param set
  re-sent every block (the CLAP rule, so a kill-and-respawn converges
  even when `set_param` replays were lost); worker ids match VST3 param
  names case-insensitively and clamp to `[min, max]`.
- [`Vst3Backend::latency_samples`](../../../core/src/plugins/vst3.rs) —
  live `IAudioProcessor::getLatencySamples` query (0 when absent),
  feeding `LatencyMap` through the unchanged `GetLatency` op.
- State is a *real* VST3 chunk: `state()` returns params + the stored
  `save_state` bytes; `set_state` pushes known params, `load_state`s the
  blob best-effort, and adopts the host-side copy regardless — snapshot
  exactness never depends on the plugin accepting it. `Drop` stops
  processing so the bundle unloads cleanly.
- Only the bundle's first audio-effect class instantiates; non-mono
  layouts refuse with `PortLayout`, never silent misrouting.
- Test fixture: `core/vst3-gain/` (standalone `cdylib`, NOT a workspace
  member, never shipped) — a `vst3`-crate mono gain effect (one `Gain`
  param id 0, normalized `0..1`, default `1.0`; fixed 32-sample declared
  latency; 12-byte `getState`/`setState` chunk on processor *and*
  controller), staged as a real `CcezGain.vst3` bundle dir
  (`Contents/MacOS/CcezGain` + `Info.plist` on macOS,
  `Contents/x86_64-linux/CcezGain.so` on Linux) and loaded by bundle
  path in tests. A system bundle exists on this machine
  (`/Library/Audio/Plug-Ins/VST3/Collective.vst3`), but the fixture wins
  for determinism: fixed gain math, fixed latency, no license/hardware
  dependency.

### 9c. How to verify

- `cargo test --manifest-path core/Cargo.toml plugins::vst3` — 12 tests:
  the 8 pre-existing (descriptor load, scan, `NoModule` / `NoFactory` /
  `BadParam` / block-shape, silence, device-node mapping) **plus** the
  real-binary set: fixture bundle loads in-process with 32-sample
  latency, sample-exact gain audio, missing bundle and unknown param as
  clean errors.
- `cargo test --manifest-path core/Cargo.toml plugins::host
  plugins::sandbox plugins::worker plugins::clap` — the sandboxed VST3
  set: load → gain audio → latency into `LatencyMap` →
  snapshot/restore with a non-empty real state chunk, kill → `Crashed`
  → recover through a re-loaded bundle, missing bundle and unknown
  param as clean errors. Mock/CLAP rows untouched.
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions (399 lib + integration targets green).
- `bun run typegen -- --check` — unaffected (new module reads the frozen
  types and adds none, so no drift is possible).

### 9d. What this follow-up deliberately leaves out

- ~~Multi-effect bundles (first audio class instantiates;
  `load_plugin_class` selection is the follow-up), non-mono layouts
  (`PortLayout` refusal), stereo/surround arrangements.~~ **Update:
  landed — see §10.** `Vst3Backend::available_classes` enumerates every
  factory class, `load_class` instantiates by uid, and the stereo fixture
  class proves non-mono layouts still refuse with `PortLayout`.
- ~~Hang detection~~ **landed — see §10** — and shared-memory audio
  transport (still phase-2, same as the CLAP track (§7): the boundary
  stays, only the transport gets faster).
- Realtime-graph wiring (`Proc::Plugin` feeding sandboxed inserts) — the
  `process(&[f32]) -> Vec<f32>` shape already matches it.
- Linux `xcb` system headers: `vst3-host` needs them only with GUI
  features enabled (not ours); default-off keeps Linux builds
  header-free. CI matrix note: if a Linux job builds without system
  headers and fails inside an optional vst3-host path, gate the dep by
  target or install `libxcb-dev` — do not re-add cpal/isolation
  features to fix it.
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

## 6b. AU parameter + render bridging: the follow-up lands (no flag on the render path)

The phase-2 follow-up §6 named is now [`AuBackend`](../../../core/src/plugins/au.rs):
hand-rolled FFI grows from 3 to 12 extern fns (initialize/uninitialize,
get/set property, get/set parameter, render — still zero new crates,
plus `CoreFoundation` string reads for `cfNameString` display names),
and the worker gains one additive `LoadAu` op behind
`PluginKind::Au` + `PluginDescriptor::au` on the unchanged
host/sandbox/recovery path.

### The map

- [`AuBackend::load`](../../../core/src/plugins/au.rs) — type gate
  first (`aufx`/`aumu`/`augn` render; output/mixer/panner/converter/…
  refuse with `UnsupportedType` before touching the registry, on every
  platform), then find + open (no `CCEZ_ENABLE_AU` consult — the tested
  render path is its own opt-in; the raw `instantiate_if_flag` seam
  keeps its deferred contract), mono-`Float32` negotiation at the
  session rate (the unit's own format patched to rate + 1 channel, with
  a canonical fallback; refusal is `FormatFailed`), `MaximumFramesPerSlice`
  1024, `AudioUnitInitialize`, param discovery
  (`ParameterList`/`ParameterInfo` + live seeds, hostile ranges skipped),
  `Drop` uninitializes before disposing.
- Params — immediate `AudioUnitSetParameter` on the global scope;
  worker ids match unit names case-insensitively or a bare parameter
  number, clamp to `[min, max]`. Unknown ids never reach the unit
  (`BadParam`); unknown ids in snapshots ride along host-side so
  snapshots stay exact. AU params persist on the unit, so no per-block
  re-send is needed (unlike CLAP/VST3 kill-and-respawn convergence).
- Render — mono blocks chunked at 1024 frames with a monotonic
  `mSampleTime` stamp. Effects register a per-chunk input-pull callback
  (`kAudioUnitProperty_SetRenderCallback`) borrowing the caller's slice
  (zero-padded tail); sources render without one. Empty input renders
  empty (zero-frame renders refuse). `Drop` never renders on a stale
  callback — registration is per-chunk by construction.
- [`AuBackend::latency_samples`](../../../core/src/plugins/au.rs) —
  live `kAudioUnitProperty_Latency` seconds × rate (0 when absent),
  feeding `LatencyMap` through the unchanged `GetLatency` op.
- State is host-side passthrough like the mock (params + opaque `blob`);
  `set_state` pushes known params immediately and adopts the host copy
  regardless. AU class-info/preset serialization is the follow-up.
- No fixture to build: tests load Apple's bundled AUDelay
  (`aufx:dely:appl` via `AuComponentDesc::apple`) — observed on this
  Mac: 4 params (`Dry/Wet Mix`, `Delay Time`, `Feedback`,
  `Lowpass Cutoff Frequency`), 0-sample latency, silence in → bit-exact
  silence out, tone passes the dry path.

### How to verify (follow-up)

- `cargo test --manifest-path core/Cargo.toml plugins::au` — 15 tests:
  the 10 pre-existing (flag deferral, scan, real-instantiate, all
  untouched) **plus** the bridging set: scope-gate refusal on every
  platform, AUDelay param list/set/get/clamp/`BadParam`, silence + tone
  + empty + over-slice renders, zero latency into `LatencyMap`,
  state restore onto the unit.
- `cargo test --manifest-path core/Cargo.toml plugins::host
  plugins::sandbox plugins::worker` — the sandboxed AU set: system unit
  loads with no flag consult → silence renders → latency into
  `LatencyMap` → snapshot/restore, unknown param as clean error,
  kill → `Crashed` → recover through a re-loaded unit; plus
  missing-codes and descriptor-JSON-compat guards. Mock/CLAP/VST3 rows
  untouched.
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions (407 lib + integration targets green).
- `bun run typegen -- --check` — unaffected (`au_desc` skips
  serialization when absent, so pre-AU snapshots read byte-identical).

### What this follow-up deliberately leaves out

- Non-mono layouts (`FormatFailed` refusal), multi-bus units, output /
  mixer / panner / converter topologies (`UnsupportedType` refusal).
- AU class-info/preset blobs (host-side passthrough, like the mock's).
- Scheduled/ramped parameter events (`AudioUnitScheduleParameters`).
- Hang detection and shared-memory audio transport — same phase-2 as
  the CLAP/VST3 tracks: the boundary stays, only the transport gets
  faster.

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

Decision (landed): the zero-new-deps v1 proved the sandbox +
snapshot + recovery machinery with a mock backend behind an honest
named seam (`HostError::ClapUnimplemented` — the same mark-and-refuse
pattern Track B used for remote DSP). Real loading has since landed as
`ClapBackend` ([`clap.rs`](../../../core/src/plugins/clap.rs),
clack-host + libloading + clack-extensions): it runs *inside the worker
process* — untrusted C-ABI code stays behind the process boundary —
and speaks the existing worker protocol (`Init`, then one additive
`LoadClap` round-trip), so host and recovery code did not change shape
and the `ClapUnimplemented` refusal is gone.

The map:

- [`ClapBackend::load`](../../../core/src/plugins/clap.rs) —
  `PluginEntry::load` + first-plugin `PluginInstance::new` + param
  discovery (names/ids/ranges/seed values) + mono port-layout check +
  `activate` at the session rate + `start_processing`.
- [`ClapBackend::process`](../../../core/src/plugins/clap.rs) — mono
  blocks (chunked at 1024 frames) with the full current param set
  carried as `ParamValueEvent`s every block; worker ids match CLAP
  param names case-insensitively and clamp to `[min, max]`.
- [`ClapBackend::latency_samples`](../../../core/src/plugins/clap.rs) —
  live query of the CLAP latency extension (0 when absent), feeding
  `LatencyMap` through the unchanged `GetLatency` op.
- State is host-side truth like the mock (params + opaque `blob`
  passthrough); `Drop` stops and deactivates so the bundle unloads
  cleanly. Only the entry's first plugin instantiates; non-mono
  layouts refuse with `PortLayout`, never silent misrouting.
- Test fixture: `core/clap-gain/` (standalone `cdylib`, NOT a workspace
  member, never shipped) — a `clack-plugin` mono gain effect (one
  `gain` param, fixed 64-sample declared latency), copied to
  `ccez-gain.clap` and loaded by path in tests.

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
  plugins::sandbox plugins::worker plugins::clap` — 17 tests: mock
  load/process/unload, duplicate/unknown-id errors, state JSON
  round-trip, protocol single-line framing (now including `LoadClap`),
  mock gain math + blob state, spawn/snapshot/process,
  kill → `Crashed` → recover byte-exact (params *and* blob),
  idempotent recover on a live worker, **plus** the real-CLAP set:
  fixture bundle loads in-process with 64-sample latency, sandboxed
  load → gain audio → latency into `LatencyMap` → snapshot/restore,
  kill → `Crashed` → recover through a re-loaded bundle, missing
  bundle and unknown param as clean errors.
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions (391 lib + integration targets green).
- `bun run typegen -- --check` — `ok project.ts`, `ok ipc.ts`
  (new module reads the frozen types and adds none).

### What agent 1 deliberately leaves out

- ~~Multi-plugin bundles (index 0 instantiates), non-mono port layouts
  (`PortLayout` refusal)~~ **Update: landed — see §10** — and the CLAP
  state extension (`blob` stays host-side passthrough, still the
  follow-up).
- ~~Hang detection: reads block, so a worker that is *alive but mute*
  stalls the caller — deadline + kill is the phase-2 follow-up.~~
  **Update: landed — see §10.** Every round-trip carries a watchdog
  deadline; a mute worker reports `Crashed` and recovers like the
  segfault case. A worker that *dies* is still caught on the very next
  call.
- Audio over the pipe is one JSON round-trip per block — fine for v1;
  phase 2 moves buffers to shared memory behind the same boundary.
- Realtime-graph wiring (`Proc::Plugin` variant feeding sandboxed
  inserts from the Track B renderer) is the integration follow-up; the
  `process(&[f32]) -> Vec<f32>` shape already matches it.
- `ui/src/generated/*`, `model.rs`, `ipc.rs` untouched.

## 10. Phase-2 hardening: multi-plugin bundles, layout refusal tests, hang detection

The three follow-ups §§7–9 named are now closed in one pass, sharing
one rule: the sandbox boundary never moves — only what crosses it
(selection ids, refusal errors, heartbeats) changes.

### 10a. Multi-plugin bundles: enumerate, select by id

- CLAP: [`ClapBackend::available_plugins`](../../../core/src/plugins/clap.rs)
  lists every plugin the entry exposes in factory order (pure
  discovery — nothing instantiates, so stereo entries list fine);
  [`ClapBackend::load_selected`](../../../core/src/plugins/clap.rs)
  instantiates by CLAP id, and an unknown id is `NoPlugin`, never a
  silent index-0 fallback. The id-less `load` keeps the old default.
- VST3: [`Vst3Backend::available_classes`](../../../core/src/plugins/vst3.rs)
  lists every factory class via `vst3-host` discovery (32-hex uids);
  [`Vst3Backend::load_class`](../../../core/src/plugins/vst3.rs)
  instantiates by uid through `load_plugin_class`. Unknown uids surface
  as `Load`.
- Plumbing is additive on both sides of the pipe: `LoadClap` gains an
  optional `plugin_id`, `LoadVst3` an optional `class_id` (serde
  `default`, so pre-selection worker lines still parse — a test pins
  that), and `PluginDescriptor` gains an optional `plugin_uid`
  (`skip_serializing_if`, so pre-selection snapshots read back
  byte-identical). `recover()` replays the same selection through the
  unchanged respawn path — a test kills a second-plugin insert and
  proves it comes back as the second plugin.
- Fixtures: `core/clap-gain/` now exposes three plugins from one
  custom entry (`com.ccez.gain`, `com.ccez.gain-two`,
  `com.ccez.stereo-gain`); `core/vst3-gain/` now exports four factory
  classes (`Ccez Gain`, `Ccez Gain Two`, `Ccez Stereo Gain`, plus the
  shared controller). Index 0 stays the historical default, so every
  pre-existing test loads what it always loaded.

### 10b. Non-mono layouts: tested refusal, per format

Stereo end-to-end (multi-channel pipe + bus negotiation per format) is
still future work; what lands here is the other half of the contract
the task allowed: a *named* refusal with a test that presents a real
non-mono plugin per format. CLAP and VST3 refuse through their existing
`PortLayout` variants — the stereo fixture entries above load far
enough to hit the layout check and fail with `2 channels` in the
message. AU keeps its `UnsupportedType` scope gate (already tested on
every platform) and `FormatFailed` negotiation refusal; the mock has no
port-layout concept (gain is channel-agnostic), so there is nothing to
refuse there.

### 10c. Hang detection: heartbeat + watchdog

- The worker gains one additive op, `Ping`: an immediate no-op reply.
- [`SandboxedPlugin`](../../../core/src/plugins/sandbox.rs) gains a
  per-plugin watchdog deadline (`DEFAULT_WORKER_TIMEOUT`, 10 s;
  `set_timeout` for tests): every round-trip read moves to a thread
  and is collected with `recv_timeout`. A mute child is killed and
  reports `Crashed` — the same variant as the segfault case, so
  `recover()` replays `last_good` unchanged. `ping()` is the
  no-side-effect poll (`PluginHost::ping` alongside it); death paths
  now kill before reaping so a *suspended* child can never block
  `wait`.
- The test suspends a live mock worker with `kill -STOP` (unix-only):
  still unreaped (`alive`) but answering nothing — the alive-but-mute
  stand-in. `process` then fails `Crashed` near the 300 ms test
  deadline, and `recover` respawns fresh with the exact pre-hang gain.

### 10d. How to verify

- `cargo test --manifest-path core/Cargo.toml plugins::` — 86 tests
  (up from ~60): enumeration order, select-by-id audio, unknown-id
  errors, stereo `PortLayout` refusal, legacy protocol/descriptor
  back-compat, heartbeat, and the simulated-mute watchdog round-trip —
  plus every pre-existing row untouched.
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions.
- `bun run check` — green (`typegen --check` unaffected: new types
  only, none frozen; `plugin_uid` skips serialization when absent).

### 10e. What §10 deliberately leaves out

- True stereo/surround audio end-to-end (multi-channel worker pipe,
  per-format bus negotiation, mix-point alignment).
- The CLAP state extension (`blob` stays host-side passthrough) and AU
  class-info/preset blobs.
- Shared-memory audio transport — the boundary stays, only the
  transport gets faster.

## 11. WASM devices in the sandboxed worker (agent 4)

Third-party DSP as `.wasm` modules, loaded where every other untrusted
format loads: *inside the worker process*, behind the unchanged sandbox
boundary. The devices-side in-process sandbox
(`core/src/wasmdevices/`, primer `docs/notes/p-wasm.md`) proves the DSP
isolation story; this section proves the *hosting* story — one module
per worker, params and state over the existing pipe.

### 11a. Runtime evaluation (verdict first)

Verdict: **`wasmtime` 48.0.2** (the `WasmNote` in
[`host.rs`](../../../core/src/plugins/host.rs) records this).

- `wasmtime` 48.0.2 is already the pinned devices-side runtime: typed
  `Func` boundary, `Store` isolation, empty-`Linker` instantiation for
  zero-import guests — and an *optional* cargo dependency, so default
  builds download and compile nothing new. Reusing the pin for the
  worker adds zero new dependency surface: same version, same feature
  flag (`wasm-runtime`), same offline-friendly rule the CLAP host kept.
- `wasm3`/`wasmi` are lighter (faster startup, smaller binary) but
  would each add a new dependency for an unproven embedding. Device
  loads are seconds-apart setup operations, not per-block work — startup
  time never dominates — so the proven runtime wins.
- The guest contract is four exports and zero imports (`wasm_init` /
  `wasm_set_param` / `wasm_sample` / `wasm_reset`), mono `f32`
  per-sample with params as `(code, f32)` pairs: the
  [`kernel.rs`](../../../core/src/devices/kernel.rs) shape, so the seam
  is a loader plus a memory copy, never a redesign. Strings never enter
  the guest; state the host must resurrect (params plus the opaque blob)
  lives in worker-owned `PluginState` bytes — caller-owned, so
  `recover()` replays `Init` + `LoadWasm` + `SetState` with zero guest
  cooperation.

### 11b. The map

- [`wasm.rs`](../../../core/src/plugins/wasm.rs) — the worker-side
  `WasmBackend`: `load`/`load_bytes` (binary or WAT text;
  `Module::new` sniffs both), `set_param` (unknown ids refuse *before*
  touching the guest, so refused sets leave stored state untouched),
  `process` (one `f32` per guest call — the narrowest aperture),
  `state`/`set_state` (byte-exact blob passthrough), `latency_samples`
  (honest 0 for memory-free gain). `GAIN_GUEST_WAT` is the test guest:
  a gain device (unity default, the null-device with a knob, like the
  mock) as inline WAT — `wasmtime` parses module text hermetically, so
  tests stage it to a temp file and `LoadWasm` it through the real file
  path with no committed bytes and no new crate.
- [`worker.rs`](../../../core/src/plugins/worker.rs) — one additive op,
  `LoadWasm { path }`, with the `LoadClap` contract (requires `Init`
  first; a failure leaves the previous backend running). Every existing
  op dispatches on the new `Backend::Wasm` arm; without the feature the
  op fails cleanly (`wasm-runtime feature not enabled`).
- [`host.rs`](../../../core/src/plugins/host.rs) — `PluginKind::Wasm`
  plus the `wasm()` descriptor constructor (path-required, like
  Clap/Vst3) and the `WasmNote` evaluation above.
- [`sandbox.rs`](../../../core/src/plugins/sandbox.rs) — respawn replays
  `LoadWasm` for `Wasm` descriptors (and refuses cleanly without the
  feature), so crash recovery reloads the module exactly like it reloads
  CLAP/VST3 bundles. Mock, CLAP, VST3, and AU arms are untouched.

### 11c. How to verify

- `cargo test --manifest-path core/Cargo.toml --lib plugins::` — the
  default gate, `wasm-runtime` off: protocol JSON roundtrip gains the
  `LoadWasm` line, a refused `LoadWasm` leaves the mock rendering, and a
  `Wasm` descriptor load refuses cleanly with no half-loaded entry.
- `cargo test --manifest-path core/Cargo.toml --features wasm-runtime --lib plugins::` —
  plus 9 WASM tests: 5 backend unit rows (`load_bytes` roundtrip,
  unknown-param refusal with state untouched, blob-exact restore,
  bad-bytes/missing-export/`Io` refusals) and 4 worker-process rows
  through a feature-built worker binary (load → unity → `SetParam` gain
  → scaled audio → snapshot/restore, kill → `Crashed` → recover
  re-rendering the pre-crash gain, missing module and unknown param as
  clean errors). The feature worker is built once per test process and
  staged to a temp copy — re-copying over a running worker's image
  SIGKILLs it on macOS, so the `OnceLock` is load-bearing, not style.
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions. `bun run check` — green (`typegen --check` unaffected:
  `PluginKind`/`WorkerRequest` live outside `emit.rs`/`model.rs`/
  `ipc.rs`, so no drift is possible).

### 11d. What §11 deliberately leaves out

- Shared-memory block transport — samples still cross one `f32` per
  guest call inside the worker and one JSON round-trip per block on the
  pipe. The boundary stays; only the transport gets faster.
- Fuel metering for worker guests (the devices-side sandbox already
  proves the pattern with `fuel_per_block`; wiring it into the worker
  backend is a small follow-up).
- More guest DSP: only gain is hosted. Filters/delays follow the same
  recipe (extend the code map, extend the guest) — the delay kernel
  itself is already ported devices-side as the reference.
- Realtime-graph wiring (`Proc::Plugin` feeding sandboxed inserts):
  `process(&[f32]) -> Vec<f32>` already matches it.
- `devices/wasm.rs` untouched — the devices-side seam marker still
  names its phase; this module is the plugins-side arrival, not a
  rewrite of it.
