# Track E primer: the timeline (one model, two views)

New to DAW timelines? Start here. This note teaches the three ideas behind
`core/src/timeline.rs` (mirrored in `ui/src/timeline/model.ts`), then maps
each one to the exact function that implements it. The frozen rules it obeys
live in `contracts/project-schema.md` (the `Track`/`Clip` shapes) and
`contracts/op-log-format.md` (the `ClipMoved` op); the data shapes live in
`core/src/model.rs`, which this track does not touch.

## 1. Sections are objects; clips belong by overlap

Most timelines glue clips to named parts with pointers, and the pointers rot
(move a section, forget a clip). Here a [`Section`](../../core/src/timeline.rs)
is just a named beat range — `Verse 0..16`, `Chorus 16..32` — and a clip
belongs to a section when their beat ranges *overlap*
([`section_clips`](../../core/src/timeline.rs)). Nothing is stored on the
clip, so there is nothing to go stale.

The map: [`TimelineDoc::add_section`](../../core/src/timeline.rs)
(validates: no duplicates, finite start >= 0, positive length; adjacency is
legal), [`section_end`](../../core/src/timeline.rs),
[`section_clips`](../../core/src/timeline.rs). The demo
([`sample_timeline_project`](../../core/src/timeline.rs) +
[`sample_timeline_doc`](../../core/src/timeline.rs)) ships Verse `0..16`
and Chorus `16..32` over two tracks, with `clip_chorus_2` (bass, start 24)
as the validation target.

```rust
let chorus = doc.section("sec_chorus").unwrap();
let ids: Vec<&str> = section_clips(&project, chorus).iter().map(|c| c.id.as_str()).collect();
assert_eq!(ids, vec!["clip_chorus_1", "clip_chorus_2"]);
```

## 2. Linear and launcher are views; arrangements are orders

The **linear** view lays each track's clips on a beat ruler
([`clips_sorted_on_track`](../../core/src/timeline.rs)); the **launcher**
view groups the *same* clips into `(section, track)` slots
([`launcher_slots`](../../core/src/timeline.rs)) — one column per track,
one row per section, empty cells kept as silent slots. Neither view owns
anything: both borrow ids from the one `Project`. An [`Arrangement`] is a
third kind of view — a play order (list of section ids) laid out by
[`arrangement_layout`](../../core/src/timeline.rs), which concatenates
section lengths at running offsets. Two arrangements over the same sections
are two song structures with zero clip duplication (the demo `arr_radio`
plays Chorus-Verse-Chorus for 48 beats from the same two objects).

The map: [`clips_sorted_on_track`](../../core/src/timeline.rs),
[`launcher_slots`](../../core/src/timeline.rs),
[`arrangement_layout`](../../core/src/timeline.rs),
[`TimelineView`](../../ui/src/timeline/Timeline.tsx) (SolidJS: linear
lanes + launcher grid over the same `project` prop).

```rust
let radio = arrangement_layout(&doc, "arr_radio")?;
assert_eq!(radio[2].section_id, "sec_chorus"); // same object, replayed
```

## 3. Clips carry object-level sound; motion is frozen ops

Each clip has a [`ClipProps`](../../core/src/timeline.rs) sidecar —
gain dB, pitch semitones, time ratio, fades, an FX device chain, and a
routing-target override — carried in [`TimelineDoc`] (stored beside the
project, e.g. `timeline.json`, never inside it). [`validate`](../../core/src/timeline.rs)
rejects out-of-range props loudly; clips without overrides get flat unity
defaults ([`props_for`](../../core/src/timeline.rs)), so the mixer reads
`Project` first and consults props as overrides.

And the punchline: timeline edits are ordinary history. Dragging a clip
builds a frozen `ClipMoved` op with the frozen `{"startBeats": n}` payload
([`move_clip_op`](../../core/src/timeline.rs)); moving a section fans out
to one such op per member clip
([`move_section_ops`](../../core/src/timeline.rs)) — no new op kind, no
new IPC. Section moves, launcher triggers, and linear drags all stay
undoable through the Track A engine by design.

```rust
let op = move_clip_op("ui", "clip_chorus_2", 32.0);
let seq = engine.apply(&op.actor, op.kind.clone(), &op.target, &op.value_json)?;
engine.undo()?; // Chorus-2 is back at 24
```

## 4. Helpers the views share

- [`clip_end`](../../core/src/timeline.rs) / beat math:
  [`beats_to_seconds`](../../core/src/timeline.rs) /
  [`seconds_to_beats`](../../core/src/timeline.rs) at one tempo.
- [`find_overlaps`](../../core/src/timeline.rs): half-open overlap
  pairs per track (touching edges are legal; sorted input breaks early).
- [`ClipProps::effective_length_beats`](../../core/src/timeline.rs):
  audible length under the time ratio.

## 5. How to verify

- `cargo test --manifest-path core/Cargo.toml timeline` — includes the
  **move-Chorus-2** validation (frozen `ClipMoved` op through the engine +
  undo restores start 24), section fan-out, shared arrangements,
  launcher/linear agreement, prop validation, and overlap detection.
- `cd ui && bun run test` — includes the `ui/tests/timeline.test.ts` cases
  mirroring the Rust move-Chorus-2 shape.
- `cd ui && bun run check` and `bun run typegen -- --check` — green across
  the full workspace (the parallel `plugins/` integration breakage present
  when this note was drafted has since landed and passes cleanly).

## 6. Integration status & remaining boundaries

- Timeline, Session View, and Comping (`comp.rs` / `record.rs`) are now
  wired into the Tauri shell and frontend views (comping and take-lane
  capture landed in subsequent tracks).
- `model.rs`, `ipc.rs`, `emit.rs`, and `ui/src/generated/*` untouched: the
  whole track is additive views + sidecar over frozen types.
