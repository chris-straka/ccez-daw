# Track F primer: the piano roll (FL fingers, engine-grade notes)

New to piano rolls? Start here. This note teaches the three ideas behind
`ui/src/pianoroll/`, then maps each one to the exact function that
implements it. The frozen rules it obeys live in
`contracts/project-schema.md` (the untouched v0 `Clip` shape) and
`contracts/op-log-format.md`; the note bytes it edits are defined by the
Track F engine half in `core/src/midi/` (`MidiNote`, `MidiClip`).

## 1. Notes live beside the schema, not inside it

A v0 `Clip { kind: Midi, source }` carries no notes — `source` is an opaque
asset key (`take:<n>`), and the bytes behind it are the JSON encoding of a
`MidiClip` (sorted note list + clip length). Opening a project never parses
a note; the piano roll loads the asset lazily, only when a MIDI clip is
actually opened for edit or render.

Why? The alternative — a `notes` field on `Clip` — is a schema change: a
migration note, a version bump, a drift-gate failure. The asset convention
keeps `model.rs`, `ipc.rs`, and `ui/src/generated/*` untouched (verify with
`bun run typegen -- --check`), while bundles carry MIDI for free through
the engine's `midi`-kind asset API.

The map: [`MidiClip::encode` / `decode`](../../core/src/midi/clip.rs) plus
[`save_to_engine` / `load_from_engine`](../../core/src/midi/clip.rs) on the
Rust side; [`encodeClip` / `decodeClip`](../../ui/src/pianoroll/model.ts) on
the UI side. Both sides validate every field (`pitch 0..=127`, `velocity
1..=127`, `len_beats > 0`, `probability 0..=1`, `ratchets >= 1`,
`timing_offset_beats ±0.25`, `pitch_bend ±48`, `pressure`/`timbre 0..=1`)
and keep notes sorted by `(start_beats, pitch, note_id)`, so
encode → decode → encode is byte-stable. Per-note MPE expression
(`channel`, `pitch_bend`, `pressure`, `timbre`) rides on the note itself;
[`assign_mpe_channels`](../../core/src/midi/mod.rs) deals overlapping notes
distinct member channels (one channel per sounding note — MPE's one hard
rule). Playback expands deterministically
([`expand`](../../core/src/midi/transport.rs)), and
[`Recorder`](../../core/src/midi/transport.rs) round-trips
record(play(clip)) == clip.

```ts
const clip = decodeClip(await loadAsset(sourceKey)); // lazy: only on open
```

## 2. The mouse speaks FL: click-create, drag-length, right-sweep-delete

Three gestures, zero toolbar modes — the FL convention this roll copies:

- **Left-click empty space** creates a snapped note (`createNote`: pitch
  rounds to the row, beat snaps to the grid, default length = one grid
  step).
- **Drag a note body** moves it (pitch + time, snapped on commit);
  **drag the note's right edge** resizes its length. The last `6px` of a
  note (`RESIZE_HANDLE_PX`) is the resize zone, everything else on the note
  is the move zone — [`hitTest`](../../ui/src/pianoroll/geometry.ts)
  disambiguates, topmost note wins.
- **Right-button press/sweep** deletes notes under the cursor and keeps
  deleting as the pointer sweeps (`sweepDelete`), with `contextmenu`
  suppressed so the browser stays out of the way.

The map: [`createNote` / `moveNote` / `resizeNote` / `sweepDelete` /
`deleteNote`](../../ui/src/pianoroll/store.ts) — pure, immutable (return a
new clip), headless. They never touch IPC: persistence (engine asset save)
is the caller's job, so the edit path stays out of the IO path. Edits that
can't apply (unknown `note_id`) throw; resize clamps to `1/16` beat instead
of corrupting. Coordinates are pure too:
[`beatToX` / `xToBeat` / `pitchToY` / `yToPitch` /
`snapBeat`](../../ui/src/pianoroll/geometry.ts) — pitch rows run top (127)
to bottom (0), black-key rows are shaded visual-only.

## 3. Hot canvas: the framework commits, the rAF loop draws

Per-frame drawing must not pass through Solid signals. `PianoRoll.tsx`
splits the two worlds:

- **Draw path (hot):** a `requestAnimationFrame` loop reads a plain mutable
  `frame` snapshot (`notes`, `hover`, in-progress `drag` ghost) and paints
  grid + notes + ghost on a 2D canvas. No signals, no effects, no
  allocations that matter.
- **Commit path (reactive):** pointer events update only the mutable drag
  preview; `pointerup` (or each right-sweep hit) folds the gesture through
  the pure store functions and calls `onChange` once. A single
  `createEffect` refreshes the frame snapshot when the `clip` prop changes —
  that effect is the only reactive code touching frame data.

The map: [`PianoRoll`](../../ui/src/pianoroll/PianoRoll.tsx) (`onPointerDown`
/ `onPointerMove` / `onPointerUp` + `draw`), re-exported from
[`index.ts`](../../ui/src/pianoroll/index.ts) alongside the model,
geometry, and store helpers.

## 4. How to verify

- `bun test ui/tests/pianoroll.test.ts` — the scripted note-edit test: the
  full FL gesture sequence headlessly (create → resize → move →
  sweep-delete), hit-test disambiguation (body vs resize handle vs empty),
  coordinate round-trips + snap, missing-id/degen-erate edits, and
  encode/decode byte-stability + rejection. 5 tests, 31 assertions.
- `bun test ui/tests` — full UI suite (42 tests, no regressions).
- `bunx tsc --noEmit -p ui/tsconfig.json` — strict typecheck clean.
- `bun run typegen -- --check` — unaffected (this track mirrors the MIDI
  JSON by hand in `pianoroll/model.ts` and touches no generated file).

## 5. What Track F agent 2 deliberately leaves out

- Wiring `PianoRoll` into `App.tsx` (still a v0 stub panel) and engine
  asset save/load behind `onChange` — the integration follow-up; the
  component's `onChange` seam is already the attachment point.
- Note-level undo as op-log entries: MIDI edits persist as asset bytes, not
  `OpKind` entries (the frozen op set has no note kind) — undo scoping for
  note edits belongs to a versioned-contract decision, never silent drift.
- Velocity editing, marquee select, and keyboard nudge: the store functions
  compose to them, but no gesture claims them yet.
- `ui/src/generated/*`, `model.rs`, and `ipc.rs` untouched.
