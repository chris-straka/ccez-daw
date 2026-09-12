# Q primer: the record workflow (punching, count-in, monitoring, take lanes)

New to recording in a DAW? Start here. This note teaches the four ideas
behind `core/src/record.rs` (mirrored in `ui/src/record/record.ts`), then
maps each one to the exact function that implements it. The frozen rules it
obeys live in `contracts/project-schema.md` (the `Clip` shape) and
`contracts/op-log-format.md` (the `ClipAdded` op); the data shapes live in
`core/src/model.rs`, which this track does not touch.

Out of scope on purpose: Surround/Atmos, hardware control surfaces, and
cloud/collab. **Hardware inserts are explicitly out** — monitoring here is
software-only (input through the DAW, compensated for interface latency).
There is no outboard send/return path anywhere in this track.

## 1. Punching answers "which beats get captured?"

Players rarely record a whole song in one go. *Punch in/out* means the
transport plays freely but only a chosen span is captured — the classic
use is re-singing one bad line in an otherwise good verse. Two flavors:

- **Manual** punch: the player (or footswitch) decides in the moment.
  The rule is trivial — capture whenever the transport is recording.
- **Auto** punch: a pre-set range `[start, end)` decides. The playhead
  rolls from before the line so the singer gets context, but only audio
  inside the range is kept.

The map: [`validate_punch`](../../core/src/record.rs) (finite bounds,
`start >= 0`, `start < end`), [`is_punching`](../../core/src/record.rs)
(auto is end-exclusive with a 1e-9 beat epsilon, so a playhead sitting
exactly on the out-point counts as *out*). A hand-punched pass that
overshoots the window is trimmed by
[`punch_overlap`](../../core/src/record.rs) — a pass missing the window
entirely yields `None`, never an empty take.

```rust
let mode = PunchMode::Auto(validate_punch(8.0, 12.0)?);
assert!(is_punching(8.0, &mode, true));   // in-point: capturing
assert!(!is_punching(12.0, &mode, true)); // out-point: playing on
```

## 2. Count-in is the metronome with a job

A metronome clicks forever; a *count-in* clicks exactly long enough to
give the player tempo, then stops dead as recording starts. This module
treats the count-in as a schedule derived from musical units: bars times
the project's beats-per-bar (from the time signature), so accents land on
real bar lines.

The map: [`count_in_beats`](../../core/src/record.rs),
[`count_in_seconds`](../../core/src/record.rs) (beats at the project
tempo), [`count_in_clicks`](../../core/src/record.rs). Offsets are
negative — they count *up to* the record downbeat, which is itself never
a click. A 1-bar 4/4 count-in clicks at `-4, -3, -2, -1` with the accent
on `-4`.

```rust
let clicks = count_in_clicks(&CountIn { bars: 1, beats_per_bar: 4 })?;
assert_eq!(clicks[0], CountInClick { offset_beats: -4.0, accent: true });
```

## 3. Software monitoring hears you late, then corrects for it

When a track is armed you want to hear your live input through the DAW
(direct monitoring in software). The catch: the audio interface delays
everything — samples take time to get *in* and time to get back *out* —
so what you hear lags the click, and what gets recorded lands late on
the timeline. The fix is pure arithmetic: measure the round-trip latency
in samples, convert to beats, and shift the captured material back by
exactly that amount.

The map: [`MonitorMode`](../../core/src/record.rs) +
[`should_monitor`](../../core/src/record.rs) (`Off` never, `On` whenever
armed, `Auto` when armed and *not* playing back — during playback you hear
the recorded lanes, otherwise you hear yourself),
[`latency_beats`](../../core/src/record.rs) (in + out samples at the
interface sample rate and project tempo),
[`compensate_capture`](../../core/src/record.rs) (subtract the delay).

```rust
let late = latency_beats(256, 256, 48_000.0, 120.0)?; // one 512-sample round trip, in beats
let on_grid = compensate_capture(captured_beat, late);
```

## 4. Take lanes are just clips, stacked

Every punched pass becomes an ordinary clip spanning the punch range with
`source = "take:<id>"` ([`punch_take_clip`](../../core/src/record.rs)) —
it enters the project through the frozen `ClipAdded` op, so takes are
undoable and survive restart for free. *Take lanes* are the UI stacking
of those clips: lane 0 is the earliest pass, later passes stack above
([`assign_take_lanes`](../../core/src/record.rs), same
`(start_beats, id)` order `takes_for_region` returns). Comping then needs
no new machinery: pick sections across lanes and stitch them with
[`build_comp`](../../../core/src/comp.rs), exactly as Track E documents.

```rust
let punch = validate_punch(0.0, 4.0)?;
let takes = vec![
    punch_take_clip("take_vox_p1", "trk_vox", "Vox p1", ClipKind::Audio, &punch),
    punch_take_clip("take_vox_p2", "trk_vox", "Vox p2", ClipKind::Audio, &punch),
];
let keeper = build_comp("clip_vox_keeper", "trk_vox", "Vox keeper",
    &[CompSection::new("take_vox_p2", 0.0, 2.0),
      CompSection::new("take_vox_p1", 2.0, 4.0)], &takes)?;
assert_eq!(keeper.source, "comp:take_vox_p2+take_vox_p1");
```

The validation path (`punch-take` in `cargo test -p ccez-core record`
and `bun test tests/record.test.ts`) runs exactly the snippet above on
both sides of the mirror, so Rust/TS drift shows up as red.

## Why no contract changes

Deliberately, this track adds no `OpKind`, no project-schema field, no
IPC command, and no `contracts/` edit: takes are `ClipAdded` payloads,
the keeper is a `build_comp` composite, and count-in/monitor state is a
UI-side concern over the existing `EngineState`. The typegen drift gate
therefore passes untouched, and parallel tracks extending `emit.rs`
cannot conflict with this one.
