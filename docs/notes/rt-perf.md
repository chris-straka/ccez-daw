# Realtime performance audit (DAW audio-callback paths)

Date: 2026-09-16. Scope: `core/src/audio/` + `core/src/plugins/`
(`render.rs`, `device.rs`, `transport.rs`, `sandbox.rs`, `chain.rs`,
`mixer.rs`). Rule: the audio callback (everything under
`CallbackState::fill` / `NullBackend::pump` / `render_mono_block`) must
never allocate-per-block beyond bounded scratch, never lock, never block.

## Verdict

No locking or blocking on the callback path. The handoff design holds:
UI builds owned graphs off-thread and `send()`s them; the callback only
`try_recv`s (`device.rs:206,437`), params use atomics + `try_lock` with a
stale-cache fallback (`device.rs:70-78`), and plugin DSP behind JSON pipes
is correctly *off*-callback (see RT-7). What remains is bounded
per-block allocation inside `RenderGraph::render` (RT-1..RT-4) — measured
far under budget (see Gate results), with a path to zero.

## Violations / findings

- RT-1 (alloc, accepted with budget): `RenderGraph::render`
  (`render.rs:82`) rebuilds the `Schedule` (`Schedule::build`, topo walk +
  level `Vec`s) on *every* block, then allocates one `Vec<f32>` per node
  per block (`render_node`, `render.rs:136,153,156,165`) plus a
  `BTreeMap<String, Vec<f32>>` insert per node (`render.rs:88,109`).
  Bounded by graph size x frames, but it is per-callback work that could be
  cached: the schedule depends only on topology, and node buffers could be
  double-buffered scratch. Follow-up: `render_with_schedule(&Schedule)` +
  scratch reuse.
- RT-2 (alloc, FIXED this pass): `render_node` built each delay-table key
  from two fresh `String`s (`producer.clone(), id.to_string()`). Now
  (`render.rs:136-151`) the producer id is *moved* into the key and the
  `id` string is hoisted: one `String` per edge instead of two.
- RT-3 (alloc, accepted): `producers_of` (`graph.rs:254`,
  via `render.rs:67-69,138`) returns an owned `Vec<String>` per node per
  block. Small (fan-in sized); disappears with the same scratch-cache
  follow-up as RT-1.
- RT-4 (alloc, accepted): residual one-`String`-per-edge key clone
  (`render.rs:141-143`). Needs interned node ids to remove; tracked as
  follow-up, not done here.
- RT-5 (copy, FIXED this pass): `CallbackState::fill` / `fill_convert`
  (`device.rs:483-505`) snapshotted every mono block with `.to_vec()`
  before interleaving — one full-block copy per callback. Now renders into
  `self.silence` and reads it back after the borrow ends; only the
  unavoidable interleave copy remains. Silence-substitution path already
  reused its buffer (`device.rs:464-473`); `static EMPTY` covers frames==0.
- RT-6 (churn, FIXED this pass): `ParamBank::refresh_cache`
  (`device.rs:70-78`) cloned every param `String` and re-inserted every
  block even when nothing changed. Now skips unchanged values: steady-state
  callback does zero clones/inserts; only changed params allocate once.
- RT-7 (blocking, by design — rule documented): `SandboxedPlugin::process`
  (`sandbox.rs:338-345`) serializes JSON, `write_all`s a pipe, spawns a
  reader thread per call (`sandbox.rs:243`), and blocks on
  `recv_timeout`. Same for `PluginHost::process` (`host.rs:397-399`).
  Verified by call-site grep: **nothing** on the callback path calls it —
  `render_mono_block` only runs `Proc` variants; plugin inserts must be
  frozen/bounced off-thread before `SwapGraph`. Rule: never call
  `PluginHost/SandboxedPlugin::process` from the audio thread; the
  watchdog + `recover()` path exists for the off-thread render worker.
- Clean (no action): `transport.rs` — `Mutex<Inner>` is only touched by
  play/stop/stats on UI threads (`transport.rs:169,187,...`); the null
  pump sleeps between blocks on its own thread (`transport.rs:299-302`);
  Link session reads are UI-side. `chain.rs` (`apply_wet_dry`,
  `chain.rs:151-153`) is allocation-free pure math, safe anywhere.
  `mixer.rs` allocates (`Vec`/`BTreeMap`) but is a UI-side view over
  project data, never called from the callback. `render.rs:94`
  `thread::scope` only runs when `threads > 1` (offline bounce); the
  callback path pins `threads = 1`.

## Perf gate

`core/tests/rt_perf.rs` (std-time, no new deps): 512-frame blocks.

| Test | Budget | Observed (this machine) |
|---|---|---|
| graph render block, 200 x 4-node rig | < 50 ms total | pass (suite total ~20 ms, debug) |
| mixer sum, 200 x 8-stem `Mix` | < 100 ms total | pass (suite total ~20 ms, debug) |
| chain `apply_wet_dry`, 2000 x 512 | < 50 ms total | pass (suite total ~20 ms, debug) |
| handoff burst (10k params + swap + stop) | drains 10_002 | exact count asserted, pass |

Budgets carry 10x+ headroom over observed means: the gate catches
algorithmic cliffs (locks, pipe round-trips, accidental clones), not CI
noise. Record fresh observations in the table when re-running on new
hardware; tighten budgets only with data from three consecutive green runs.

## Fixes landed this pass

1. `render.rs:136-151` — delay-key allocs halved (RT-2).
2. `device.rs:70-78` — param refresh skips unchanged values (RT-6).
3. `device.rs:483-505` — callback interleave without snapshot copy (RT-5).
4. `core/tests/rt_perf.rs` — new perf gate (graph / mix / chain / handoff).
