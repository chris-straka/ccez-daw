# Track H primer: automation (composed) vs modulation (performed)

New to DAW expression? Start here. Every knob in this DAW — track volume,
track pan, any device param — is one abstraction: a `node:param` address
with a value (see `contracts/project-schema.md`). Track H adds the two
ways that address gets *moved over time*. They share the address but are
separate concepts, and confusing them is the classic beginner bug — so
this note teaches the split first, then maps each side to its function.

## 1. Automation is composed (the timeline writes values)

An **automation lane** is ink on the timeline: the frozen v0
`AutomationLane` (`core/src/model.rs`) holds `(beat, value)` points
against one `ParamAddress`. The renderer reads it **sample-accurately**:
sample `i` of a block maps to its exact beat
(`start_beat + i * tempo/60/sample_rate`), and the lane is evaluated there
with linear interpolation — hold-first before the first point, hold-last
after the last. There is no block quantization: two renders of the same
lane agree sample for sample.

The map: [`eval_lane`](../../../core/src/automation.rs) (one beat →
one value), [`render_lane_samples`](../../../core/src/automation.rs)
(one block → one value per sample; the audio thread reads this shape),
[`sample_beat`](../../../core/src/automation.rs) (the beat↔sample math).
The sample-accuracy test pins it: at tempo 60 with a 4 Hz sample rate,
a 0→1 ramp over 4 beats reads exactly `i/16` at sample `i`
(`out[4] == 0.25`, `out[16] == 1.0`) — no rounding, no snapping.

```rust
let out = render_lane_samples(&lane, 0.0, 60.0, 4.0, 17)?;
assert_eq!(out[8], 0.5); // sample 8 sits at beat 2, halfway up the ramp
```

## 2. Clips make automation reusable (shapes with names)

Drawing the same swell on every chorus is toil, so automation has
**clips**: an [`AutomationClip`](../../../core/src/automation.rs) holds
points *relative to beat 0* (a shape, not a placement).
[`instantiate_clip`](../../../core/src/automation.rs) stamps it at an
absolute offset, and [`merge_points`](../../../core/src/automation.rs)
merges the stamp into a lane — same-beat points are replaced, order is
restored. A clip is copy-paste with a name; the lane stays the truth.

```rust
let stamped = instantiate_clip(&swell, 8.0)?; // beats 8..12
let points = merge_points(&lane, &stamped);
```

## 3. Modulation is performed (sources wobble addresses)

A **mod route** is a live patch cable, not ink: a [`ModSource`](../../../core/src/automation.rs)
(LFO or constant) writes to any `ParamAddress` scaled by `depth` in param
units. All LFO shapes ([`LfoShape`](../../../core/src/automation.rs):
Sine, Triangle, Saw, Square) are bipolar (-1..+1 over phase 0..1), so
`depth` always means "peak deviation" regardless of shape. The
[`ModMatrix`](../../../core/src/automation.rs) is the patch bay: v1 routes
address track `volume`/`pan` and device params — the same set the engine's
`ParamSet` accepts — and anything addressable later needs no new plumbing.

## 4. The combination rule (one line)

```text
out[i] = clamp(auto(beat_i) + Σ depth·src(secs_i))
```

Automation is evaluated per sample in **beats** (composer time);
modulation per sample in **seconds** (wall-clock LFO time,
`secs = beat * 60/tempo`); the sum is clamped to the destination's range
(volume [0, 1.5], pan [-1, 1], device min/max). No lane? The automation
term is the live knob value, so the knob keeps working under modulation.
No routes? Pure automation. Unknown address? An error —
[`AutomationError::UnknownParam`](../../../core/src/automation.rs) —
never silent zero.

The map: [`param_base_and_range`](../../../core/src/automation.rs)
(value + range lookup mirroring the engine),
[`resolve_param_samples`](../../../core/src/automation.rs) (the rule,
per sample), [`render_block`](../../../core/src/automation.rs) (every
address at once: lanes plus route-only targets). Two pins: a constant
+10.0 on a 0.8-volume lane clamps every sample to exactly 1.5 (no wrap,
no NaN), and the same machinery drives a device `cutoff` (1000 Hz + 100
depth → 1100 Hz) through the identical `node:param` addressing.

## 5. Where it lives (and what it doesn't touch)

- Rust: `core/src/automation.rs` — pure functions over the frozen types
  plus the [`AutomationDoc`](../../../core/src/automation.rs) sidecar
  (`clips` + `routes`, JSON beside the project like `TimelineDoc`).
  Lanes themselves stay on `Project.automation`.
- TypeScript: `ui/src/automation/model.ts` — line-for-line mirror
  (`evalLane`, `renderLaneSamples`, `instantiateClip`, `mergePoints`,
  `lfoValueAt`, `resolveParamSamples`), tested in
  `ui/tests/automation.test.ts` (11 tests mirroring the Rust pins,
  including the sine peak `depth * 1.0` at the quarter period).
- Untouched: `model.rs`, `ipc.rs`, `ui/src/generated/*` — no new
  project-schema or IPC surface, so the typegen drift gate passes
  unchanged. Lane edits still flow through the frozen op log
  (`ParamSet` on the address); clip/route persistence is the sidecar.

## 6. How to verify

- `cargo test --manifest-path core/Cargo.toml` — 117 lib tests pass,
  including the 10 automation tests (sample-accuracy pins, mid-block
  offset, hold behavior, clip stamp + merge, clamp, device addressing,
  LFO periodicity, unknown-param error, validation, JSON round-trip).
- `cd ui && bun run check && bun test` — typecheck clean, 64/64 pass
  including the 11 automation tests.
- `bun run typegen -- --check` — `ok project.ts`, `ok ipc.ts` (no drift).

## 7. What Track H deliberately leaves out

- No new `OpKind`: lane edits reuse `ParamSet`; clip/route edits have no
  op yet (undo for sidecar edits is the follow-up — likely a sidecar
  snapshot op, not a schema change).
- No audio-thread wiring: `resolve_param_samples` is the pure math the
  callback will call per block; pushing `SetParam` streams into
  `AudioEngine` is the integration follow-up with Track B.
- LFOs are wall-clock Hz only (no beat-sync mode yet); sources are LFO +
  constant (no envelope follower / sidechain-as-modulator yet — though
  both fit `ModSource` without touching the combination rule).
