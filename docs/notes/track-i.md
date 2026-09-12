# Track I primer: per-clip time/pitch + in-timeline repair

New to pitch/time editing? Start here. This note teaches the four ideas
behind `core/src/timepitch.rs` and `core/src/repair.rs` (mirrored in
`ui/src/repair/model.ts`, panel in `ui/src/repair/RepairPanel.tsx`), then
maps each one to the exact function that implements it. The frozen rules it
obeys live in `contracts/project-schema.md` (the `Clip` shape — untouched)
and `contracts/op-log-format.md` (edits stay undoable); the sidecar ranges
come from Track E's `ClipProps` in `core/src/timeline.rs`
(`pitch_semitones` ±24, `time_ratio` > 0). The waves it works over are Track
E's timeline clips and Track F's `MidiClip` assets (`core/src/midi/`).

## 1. Transpose is integer pitch + fractional bend

MIDI pitch is an integer, but `pitch_semitones` is a float — so where does
+0.3 semitones go? [`split_pitch`](../../../core/src/timepitch.rs) splits
the shift: the rounded whole part moves `pitch`, the ±0.5 remainder rides
per-note `pitch_bend`. A detune survives instead of rounding away, and
integer transposes are *exactly* invertible — the round-trip validation:

```rust
let up = transpose_clip(&clip, 7.0)?;   // C-E-G -> G-B-D
assert_eq!(up.notes.map(pitch), vec![67, 71, 74]);
let back = transpose_clip(&up, -7.0)?;
assert_eq!(back.encode()?, clip.encode()?); // byte-stable
```

The map: [`semitones_to_ratio`](../../../core/src/timepitch.rs) /
[`ratio_to_semitones`](../../../core/src/timepitch.rs) (12-TET both ways),
[`transpose_clip`](../../../core/src/timepitch.rs) (errors when a result
would leave 0..=127 or the shift leaves ±24 — nothing clips silently),
[`apply_props_to_midi`](../../../core/src/timepitch.rs) (the full sidecar
edit: transpose + bend + time scale in one call).

## 2. Time ratio is playback rate; warp is a shift map

`time_ratio = 2` plays twice as fast: onsets, lengths, microtiming, and the
clip length all halve ([`time_scale_clip`](../../../core/src/timepitch.rs)).
Like transpose it inverts exactly (`r` then `1/r`). Audio agrees by
convention: [`resampled_len`](../../../core/src/timepitch.rs) maps the same
ratio to frame counts (ceil, so slow-downs keep the trailing partial frame).

Free timing is a [`WarpMap`](../../../core/src/timepitch.rs): sorted
`(at_beats, shift_beats)` pins with linear interpolation between them and
flat ends — an empty map is exactly the identity (byte-stable no-op,
tested). [`apply_warp_to_midi`](../../../core/src/timepitch.rs) moves
onsets only; lengths are preserved, so a warped note keeps its performed
duration. [`WarpMap::negated`](../../../core/src/timepitch.rs) undoes small
shifts (exact when shifts are small against pin spacing).

```rust
let map = WarpMap::new(vec![WarpMarker { at_beats: 0.0, shift_beats: 0.1 }])?;
let back = apply_warp_to_midi(&apply_warp_to_midi(&clip, &map), &map.negated());
// onsets within 1e-9, lengths untouched
```

## 3. Sample repair: DC, clicks, silence, transients, spectrum

Five pure, deterministic transforms over mono `f32` frames in
[`core/src/repair.rs`](../../../core/src/repair.rs):

- [`remove_dc`](../../../core/src/repair.rs): subtract the mean so silence
  is really zero.
- [`declick`](../../../core/src/repair.rs): a frame is a click when it
  exceeds `threshold` *and* dwarfs its neighbors (a lone spike, not loud
  music — the plateau test proves loud music survives). Clicks interpolate
  across `radius` frames; detection is read-only first, so one repair never
  seeds a neighboring detection in the same pass. Converges: a second pass
  finds nothing.
- [`trim_silence`](../../../core/src/repair.rs): first/last frame above a
  threshold (`None` when the whole buffer is hush).
- [`detect_transients`](../../../core/src/repair.rs): energy-flux onsets
  (window-ahead energy vs window-behind) — the same onsets the spectral
  editor snaps selections to. One hit per window, no double-trigger.
- Spectral edits over magnitude frames: [`gate_bands`](../../../core/src/repair.rs)
  (magnitudes below a floor go to zero, returns the zeroed count) and
  [`notch_band`](../../../core/src/repair.rs) (cut one humming band, e.g.
  mains hum — out-of-range bands and cuts outside 0..=1 are errors).

## 4. MIDI tidy + the repair panel

[`quantize_clip`](../../../core/src/repair.rs) snaps starts to a grid; the
leftover becomes `timing_offset_beats`, and remainders past the ±0.25
microtiming lane fold to the grid (documented, never silently kept).
[`remove_muted`](../../../core/src/repair.rs) drops muted notes;
[`fix_same_pitch_overlaps`](../../../core/src/repair.rs) truncates an
earlier note where a later same-(pitch, channel) note starts inside it —
the classic stuck-note fix, with a 1-tick floor. All three converge (second
pass is a no-op, tested both sides).

[`RepairPanel`](../../../ui/src/repair/RepairPanel.tsx) puts this
in-timeline: one clip in (its `MidiClip` bytes, lazily loaded from the
engine asset behind the frozen `Clip.source` per Track F), pitch in st with
the ratio shown alongside (both views agree), one warp-shift pin, and
cleanup checkboxes (drop-muted / fix-overlaps / quantize off–8th–16th).
Refusals surface as messages, never silent clamps.

## 5. How to verify

- `cargo test --manifest-path core/Cargo.toml --lib timepitch` — 5 tests
  (12-TET inverse, range refusals, identity warp, ceil frames).
- `cargo test --manifest-path core/Cargo.toml --lib repair` — 5 tests
  (DC, declick-vs-music, trim+transients, spectrum, MIDI tidy).
- `cargo test --manifest-path core/Cargo.toml --test
  timepitch_repair_roundtrip` — **5 round-trip tests, the Track I
  validation**: +7/−7 pitch edit byte-stable, props edit + exact inverse,
  fractional bend out-and-back, warp + negation, repair convergence.
- `cd ui && bun run test` — 71 pass, including 7 repair tests in
  `ui/tests/repair.test.ts` mirroring the Rust round-trips.
- `cd ui && bun run check` (`tsc --noEmit`) — clean.
- `bun run typegen -- --check` — no drift (pure logic only, like
  `timeline.rs`/`branch.rs`; `model.rs`, `ipc.rs`, `emit.rs`,
  `ui/src/generated/*` untouched).

## 6. What Track I deliberately leaves out

- No asset loader wiring: `RepairPanel` edits the passed `MidiClip` (demo
  phrase until the loader lands) and reports it via `onEdit` — the seam is
  the `clip` prop, and persistence follows the Track F engine-asset pattern.
- No realtime DSP: warps and repairs are offline clip transforms; the render
  path reads the edited bytes like any other asset.
- No new ops/IPC: pitch/time/repair edit clip *bytes* (assets), not project
  fields, so no `OpKind` was needed — undo restores the prior bytes.
- `model.rs`, `ipc.rs`, `emit.rs`, `ui/src/generated/*` untouched: the
  whole track is additive per-clip algorithms + panel over frozen types.
