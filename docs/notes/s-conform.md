# S primer: platform loudness conformance (track S-3)

New to loudness conformance? Start here. This note teaches the three ideas
behind `core/src/meter/conform.rs` and `ui/src/metering/conform.ts`, then
maps each one to the exact function that implements it. The frozen rules
live in `contracts/` — this track adds no schema and no IPC (it only
*finishes* rendered buffers), so the typegen drift gate cannot fail. The
measurement it stands on is Track Q's `docs/notes/q-metering.md`
(BS.1770-style integrated LUFS + 4x true peak); the stems it finishes come
from `core/src/bounce.rs`.

Game context: a LoZ:TP-like action-adventure (field/combat/dungeon/boss/
village/night) shipping on PC, Switch, PlayStation, Xbox, and mobile.
Each platform finishes the same mix to a different loudness — one click
per target, one report artifact per stem, so the Godot importer
(`godot-template/`) can prove what shipped.

## 1. A preset is a target with a platform name on it

Measuring tells you the number; conforming *moves* it. Every platform row
is just a `LoudnessTarget` (integrated LUFS + true-peak ceiling) plus
provenance, and every conform is Track Q's measure → gain → re-measure
pipeline — no new DSP:

| Preset | Target | Ceiling | Source |
|---|---|---|---|
| PC | −16 LUFS | −1 dBTP | desktop/streaming convention (approximate) |
| Switch | −24 LUFS | −1 dBTP | console convention (approximate) |
| PlayStation | −24 LUFS | −2 dBTP | Sony ASWG-R001 (spec-backed) |
| Xbox | −24 LUFS | −1 dBTP | console convention (approximate) |
| Mobile | −18 LUFS | −1 dBTP | portable convention (approximate) |

Console rows follow the Wwise mastering guidance (console −24 LUFS ±2,
mobile/portable −18 LUFS ±2, max peak −1 dBTP); PlayStation follows Sony
ASWG-R001 (−24 LKFS, max true peak −2 dBTP). PC, Switch, and Xbox carry no
published platform-holder LUFS spec, so their rows are marked
`approximate: true` — convention, not certification, one commit to update
when a spec appears. The table lives twice, deliberately:
[`PRESETS`](../../../core/src/meter/conform.rs) in Rust (the truth) and
[`CONFORM_PRESETS`](../../../ui/src/metering/conform.ts) in TypeScript
(the bounce panel) — keep them in sync.

```rust
let out = conform_to_preset(&stem, "playstation")?;
// out.stem.samples are finished; out.conformance says what happened.
```

## 2. One click: conform on bounce

The bounce panel previews first, then commits. Preview is pure math —
[`preview_conform_gain`](../../../core/src/meter/conform.rs) (Rust) /
[`previewConformGain`](../../../ui/src/metering/conform.ts) (TS) wraps
`gain_for_target`: the dB distance to the target, unless the ceiling wins
(a hot master gets quieter than the preset asks — the ceiling always
wins). The commit renders the mix (reference tracks excluded, same rule
as Track Q) and finishes it in one call:

```rust
let out = conform_mix_to_preset(&project, &config, "switch")?;
// out.bounce.wav_bytes ship; out.conformance is the proof.
```

Silence is the safe no-op (gain 1.0, never ceiling-limited — matching
silence would divide by zero and prove nothing), and unknown preset ids
error cleanly instead of silently falling back to another platform's
numbers.

## 3. The report is the proof, not the promise

Every conform re-measures the *output* and writes a
`<stem-name>.conform.json` artifact next to the stems
([`ConformanceReport::filename_for`](../../../core/src/meter/conform.rs)).
The verdict is two booleans: `target_met` (output within ±1 LU of the
preset — inside the ±2 LU platform specs quote) and `ceiling_met`
(output true peak clears the ceiling within 0.1 dB of estimate wobble).
`passed` is both. A ceiling-limited bounce *fails* the target on purpose:
shipping quieter than the preset is a decision for the mixer, not a pass
for the exporter.

```json
{ "preset": "playstation", "output_lufs": -24.1,
  "output_true_peak_dbfs": -2.3, "passed": true,
  "preset_approximate": false }
```

## 4. How to verify

- `cargo test --manifest-path core/Cargo.toml --lib meter::conform` —
  10 tests: conform-to-target per preset (output within ±1 LU, ceiling
  holds, loop points + rate untouched), unknown-preset errors, silence
  no-op that refuses to pass, report naming + JSON round-trip (including
  the `preset_approximate` flag), one-click bounce conform for all five
  presets, preview-gain parity with the real conform.
- `bun test tests/metering-conform.test.ts` (from `ui/`) — preset table
  shape + ranges, approximate flags, preview math, verdict logic, report
  naming.
- `bun run check` (repo root) — typegen `--check` + `tsc` + full
  `cargo test`, all green; this track registers no frozen types.

## 5. What S-3 deliberately leaves out

- Surround/Atmos and multi-channel weighting: every meter here is mono by
  construction (same honest simplification as Track Q).
- Loudness-range (LRA) gating per platform: the verdict is integrated +
  true peak only; LRA stays a UI-side reduction when a spec demands it.
- New IPC/bounce endpoints: the panel previews with local math and calls
  the existing bounce path; no `contracts/` or `ui/src/generated/*`
  surface was added.
- Spec certification: approximate rows are convention. When Nintendo,
  Microsoft, or PC-store guidance publishes numbers, update the two
  tables and this note in one commit.
