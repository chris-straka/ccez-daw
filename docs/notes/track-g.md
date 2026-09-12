# Track G primer: the mixer console (faders that can't lie)

New to console design? Start here. This note teaches the five ideas behind
`core/src/mixer.rs` (Rust truth) and `ui/src/mixer/` (SolidJS view), then
maps each one to the exact function that implements it. The frozen rules it
obeys live in `contracts/project-schema.md` ("the mixer is a view over
`Audio` edges") and `contracts/op-log-format.md` (every fader move lands as
a frozen `ParamSet` op — the mixer invents no op kinds).

## 1. The mixer owns no topology

Most DAWs keep a mixer model next to the router, and the two drift: a fader
moves, the graph doesn't, and the null-test fails. Here there is exactly one
edge list — the frozen `Edge` table — and the console is a *view* over it.

The map: [`strips`](../../../core/src/mixer.rs) walks the project's tracks
in order and annotates each with its first outbound `Audio` edge target
(`None` = straight to master). [`bus_children`](../../../core/src/mixer.rs)
and [`bus_roots`](../../../core/src/mixer.rs) read the same edges
downstream. Add a cable in the routing graph and the mixer re-reads it —
there is nothing to keep in sync because there is only one list.

```rust
let strips = mixer::strips(&project); // id, volume, pan, muted, solo, out
```

## 2. VCA groups and nested buses are multiplication

A VCA group is a named set of tracks sharing one trim; a bus is a
downstream sum carrying its own `gain`/`volume` params. Both nest by the
same rule: **gains multiply down the chain**.

The map: [`VcaGroup::new`](../../../core/src/mixer.rs) (validates id,
`[-120, +24]` dB range, no duplicate members — all UI-local, no schema
change), [`vca_trim`](../../../core/src/mixer.rs) (product of every group
containing the track), [`bus_gain`](../../../core/src/mixer.rs) (product of
a bus node's `gain`/`volume` params, 1.0 for implicit endpoints),
[`effective_track_gain`](../../../core/src/mixer.rs)
(`volume × VCA trims × bus-chain gains`, cycle-safe via a visited set).
Mute/solo are *routing* decisions, not gain — see
[`audible`](../../../core/src/mixer.rs) (any-solo ⇒ only solos sound).

```rust
let g = effective_track_gain(&project, &groups, "drums")?;
 // 0.8 × -6 dB VCA × 0.5 bus ≈ 0.2009
```

## 3. Snapshots: capture, recall, null-test

A [`MixerSnapshot`](../../../core/src/mixer.rs) freezes every fader plus
every group trim under a name (`slot-A`, `slot-B`). [`capture`](../../../core/src/mixer.rs)
reads, [`recall`](../../../core/src/mixer.rs) writes back exactly —
capture→recall with no edits is byte-identical state, which is the
**null-test** (`snapshot_null_test_capture_recall_is_identity`). Recall is
total (added-later tracks are left alone) so A/B slots survive track
add/remove; [`recall_strict`](../../../core/src/mixer.rs) errors on stale
slots before a comparison; [`snapshot_diff`](../../../core/src/mixer.rs)
names exactly the strips that changed ("Recall B: 1 strip(s) changed
(bass)").

```rust
let snap = capture(&project, &groups, "slot-a");
recall(&mut project, &mut groups, &snap); // null — project unchanged
```

## 4. Loudness-matched A/B (louder never wins by default)

Switching between two mixes at different levels always flatters the louder
one, so A/B slots compare at equal RMS: [`rms`](../../../core/src/mixer.rs)
→ [`loudness_db`](../../../core/src/mixer.rs) (20·log₁₀, silence = −120
dBFS floor) → [`match_gain_for(a, b)`](../../../core/src/mixer.rs) =
`rms(a)/rms(b)` → [`matched_copy`](../../../core/src/mixer.rs). Matching
silence returns 1.0 instead of dividing by zero — the safe no-op. The test
asserts a 0.5/0.25 buffer pair matches at exactly 2.0× (≈6.02 dB apart).

## 5. The reference track is a convention, not a schema change

Commercial mixes parked for comparison live on tracks named by convention —
id starts with `ref_` or name starts with `[REF] ` — so no frozen type
changed. [`reference_tracks`](../../../core/src/mixer.rs) lists them,
[`mix_tracks_excluding_reference`](../../../core/src/mixer.rs) is what
bounce/export sums, [`audible`](../../../core/src/mixer.rs) never sounds
them in the mix, and [`cue_plan`](../../../core/src/mixer.rs) returns the
solo-listen plan (reference on, everything dimmed) that the UI applies as
mutes. Cueing a non-reference track is an error — cueing the mix is
`audible`'s job, not this one's.

## 6. How to verify

- `cargo test --manifest-path core/Cargo.toml mixer` — 8 tests: snapshot
  null-test, recall-restores, diff-naming, VCA×bus multiplication,
  Audio-edge view, loudness match, reference cue/export, group validation.
- `bun test tests/mixer.test.ts` (in `ui/`) — 6 tests mirroring the Rust
  sidecar in TypeScript.
- `bun run check` (repo root) — typegen `--check` (no drift: this track
  adds no `model.rs`/`ipc.rs`/`emit.rs` surface), `tsc --noEmit`, full
  `cargo test`.

## 7. What Track G deliberately leaves out

- Fader moves in `Mixer.tsx` emit through `onPatch` — the host fans them
  out as frozen `ParamSet` ops (`node:param`, seq assigned by the engine).
  Wiring `onPatch` to `param_set` in `App.tsx` is the integration follow-up.
- Metering/FFT display: `rms`/`loudness_db` are the level math; per-block
  peak-hold meters and spectrum views land on top without touching this API.
- Motorized-control / MIDI-learn surfaces: they write the same snapshot and
  group shapes from a different input device.
