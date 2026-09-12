# Q-video primer: picture locked to the beat grid (Agent 2)

New to video-in-a-DAW? Start here. This note teaches the three ideas behind
`ui/src/video/*`, then maps each one to the exact function that implements
it. Video is a **UI-local sidecar**: it never touches the frozen v0 schema
(`contracts/*`, `core/src/model.rs`, `ui/src/generated/*`), never crosses
the Tauri IPC boundary, and never renders audio. The engine owns the
transport clock; a plain HTML `<video>` element owns the picture; this
track is the ruler between them.

Out of scope (do not build here): surround/Atmos, hardware surfaces,
cloud/collab.

## 1. A clip is a pin, not a file: `start_beats` + `offset_beats`

A [`VideoClip`](../../ui/src/video/model.ts) pins one media `src` to the
timeline. Transport beat `b` shows media time

```text
media_beats = b - start_beats - offset_beats
media_seconds = media_beats * 60 / tempo
```

implemented by [`videoTimeForTransport`](../../ui/src/video/model.ts).
`offset_beats` is the slip (positive = picture later, transport leads);
dragging the filmstrip edits *only* the offset via
[`applyOffsetDrag`](../../ui/src/video/model.ts), which clamps at
`-length_beats` so a lane can never be dragged fully blank. The inverse,
[`transportBeatsForVideoTime`](../../ui/src/video/model.ts), answers "which
transport beat shows media second `t`?" — the round-trip the sync test pins:

```ts
const moved = { ...scene, offset_beats: applyOffsetDrag(scene, 1.5) };
transportBeatsForVideoTime(moved, 0, 120); // 17.5: head + slip
```

No clip under the playhead (or a negative media time) returns `null`: the
preview holds first frame / shows the blank poster instead of guessing.

## 2. Sync is a pure decision: `seek` / `coast` / `hold` / `blank`

While playing, seeking every frame stutters, so the policy in
[`syncDecision`](../../ui/src/video/sync.ts) is standard AV-sync practice:
drift under [`SEEK_THRESHOLD_S`](../../ui/src/video/sync.ts) (0.12s)
**coasts**, drift over it **seeks**; paused transport always **holds** the
exact frame so stepping the timeline steps the picture; no clip under the
playhead while playing is **blank** (the caller pauses the element). The
function takes a sampled [`SyncInput`](../../ui/src/video/sync.ts) and
returns a [`SyncAction`](../../ui/src/video/sync.ts) — no DOM, no clock
reads — so `ui/tests/video-sync.test.ts` covers it with numbers, and
[`applySyncDecision`](../../ui/src/video/sync.ts) executes it against a
real `HTMLVideoElement` (tested with a stub). [`VideoPreview`](../../ui/src/video/Preview.tsx)
just wires the two: sample `el.currentTime`, decide, execute.

## 3. Views: filmstrip lane + preview pane + timecode

- [`FilmstripLane`](../../ui/src/video/Filmstrip.tsx): one row per clip on
  the same beat ruler as the Track E linear lane (`start*4px`, `length*4px`;
  `pxPerBeat` prop to rescale). Cells are CSS placeholder thumbs
  (`FILMSTRIP_THUMBS` per lane) until a thumbnail service wires `blob:` URLs.
  Pointer drag = offset slip at the lane scale, reported through
  `onOffsetChange` — the host owns the store, the lane only gestures.
- [`VideoPreview`](../../ui/src/video/Preview.tsx): the synced `<video>`
  element (real `src` when the clip has an asset URL; `take:` sources render
  a timecoded placeholder so the shell works with zero media).
- Timecode: [`formatTimecode`](../../ui/src/video/model.ts) /
  [`parseTimecode`](../../ui/src/video/model.ts) (`HH:MM:SS:FF` at the clip
  `fps`, frames carry into seconds) and
  [`timecodeForTransport`](../../ui/src/video/model.ts) for the pane header.

## Validation

```sh
cd ui && bun test tests/video-sync.test.ts   # 6 tests: offset math, drag clamp, timecode round-trip, sync decisions, stub-element apply, sample-doc validation
cd ui && bun run check                        # tsc --noEmit stays green; no generated files touched
```

---

# Appendix A: engine side — `core/src/video.rs` (Agent 1)

New to broadcast clocks? Start here. The UI note above pins picture to
beats with an `f64` fps; the engine cannot afford that approximation, so
this module re-solves the same mapping with exact arithmetic. Four ideas,
each mapping to one section of [`video.rs`](../../../core/src/video.rs):

## A1. Frame rates are fractions, not floats

An [`FrameRate`](../../../core/src/video.rs) is `num / den`: 29.97 is
really 30000/1001, never the float 29.97 (which drifts ~3.6 ms/hour
against the real clock — lip-sync drift over a feature). [`fps`](../../../core/src/video.rs)
exists for display only; every clock computation below uses the fraction.
[`from_fps`](../../../core/src/video.rs) snaps an imported `f64` to the
nearest known rate so UI `fps: 30` and engine `30/1` agree.

## A2. SMPTE timecode is integer frame math, including drop-frame

[`Timecode`](../../../core/src/video.rs) is `HH:MM:SS:FF` — or
`HH:MM:SS;FF` for drop-frame. NTSC picture runs at 29.97 fps but counts
labels at 30, so the counter would gain ~3.6 s/hour; the SMPTE fix is to
*skip* frame numbers `;00`/`;01` at the top of every minute except every
tenth ([`timecode_from_frame`](../../../core/src/video.rs) /
[`timecode_to_frame`](../../../core/src/video.rs)). Consequences the code
makes loud instead of silent:

```rust
timecode_to_frame(&parse_timecode("00:10:00;00", &FrameRate::R2997).unwrap(), &rate)
// 17982 — ten drop-frame minutes are NOT 18000 frames
parse_timecode("00:01:00;00", &FrameRate::R2997) // Err: that label never existed
parse_timecode("00:01:00;02", &FrameRate::R24)   // Err: ';' needs 30000/1001 or 60000/1001
```

Hours never wrap at 24 (a DAW address, not a broadcast signal).
[`VideoTrack`](../../../core/src/video.rs) pins one file (`file` is an
opaque path/URI/`take:` key core never opens) by `start_beats` +
`length_beats` + slip `offset_beats` — the same three numbers as the UI
`VideoClip` — plus the engine-only fields: rational `frame_rate`,
`drop_frame`, the house address [`start_timecode`](../../../core/src/video.rs)
of media time 0, and `follow_transport` (false = picture free-runs on the
wall clock from the play-start frame via
[`media_seconds_free_run`](../../../core/src/video.rs)).
[`VideoDoc`](../../../core/src/video.rs) is the `video.json` sidecar
(add / remove / save / load, duplicate ids rejected), the same sidecar
pattern as `timeline.rs` — no frozen schema, no IPC, no typegen surface.

## A3. Drift is measured in samples, with integer arithmetic

Three clocks meet here — beats, frames, samples — and floats would smear
them. [`drift_in_samples`](../../../core/src/video.rs) computes
`round(frame*den*sr/num) - samples` in `i128` (an hour of 48 kHz / 29.97
is ~10^12, exact, and no float is used at all); [`expected_video_frame`](../../../core/src/video.rs)
is the exact inverse (floor division), and [`DriftMonitor`](../../../core/src/video.rs)
turns the number into a decision: within half a video frame
(`InSync`, sub-frame error is unfixable by a frame-granular seek) or
`Resync { skip_samples }`. The UI `syncDecision` (seek/coast at 0.12 s)
is the preview-element policy; this monitor is the engine-side clock
truth — same shape (pure input, pure decision), finer ruler.

## A4. Thumbnails are a background job that cannot fail

[`spawn_strip_job`](../../../core/src/video.rs) runs
[`extract_strip`](../../../core/src/video.rs) on a worker thread so
transport never blocks on pixels. Resolution order is explicit path >
`$CCEZ_VIDEO_FFMPEG` > `PATH`; *absence is a normal state, not an error*:
no usable `ffmpeg`, a missing file, or an undecodable frame all degrade
to [`ThumbStrip::placeholder`](../../../core/src/video.rs) cells — same
slots, same timestamps, `path: None`, so the lane layout never shifts.
`ThumbStrip` serializes beside `video.json` (`thumbs/<track>.json`) for
instant repaint on reopen.

## Validation (engine)

```sh
cargo test --manifest-path core/Cargo.toml --lib video  # 9 tests
```

Timecode round-trips (all non-drop rates + 29.97/59.94 drop-frame sweeps
with the 17982 anchor), the 1-hour 48 kHz drift test (exact frame 107892,
one-frame slip = 1602 samples, monitor InSync/Resync, plus the sample-exact
24 fps film case), transport→picture→timecode mapping against the demo doc
(`vid_scene`, beat 20 @ 120 BPM = `01:00:02:00`), validation rejections,
sidecar save/load, placeholder degrade without `ffmpeg`, and a real-`ffmpeg`
end-to-end (skips cleanly when no binary is present). `bun run typegen --
--check` is unaffected: this track emits no types, like `branch.rs`.
