# Track B primer: the audio engine (sound that never waits for the UI)

New to realtime audio? Start here. This note teaches the four ideas behind
`core/src/audio/`, then maps each one to the exact function that implements
it. The frozen rules it obeys live in `contracts/project-schema.md` (the
universal node model + routing edges) and `contracts/ipc-table.md` (the
`engine_play` / `engine_stop` / `engine_set_tempo` transport commands —
wiring those stubs to this engine is the integration follow-up, not this
track).

## 1. One graph for four kinds of signal

Most DAWs keep separate routers for audio, MIDI, modulation, and
sidechains, and the four drift apart. Here there is exactly one edge list —
the frozen `Edge` table — and the four `EdgeKind`s are *views* over it:

- `Audio` + `Midi` edges carry signal and define **processing order**.
- `Modulation` + `Sidechain` edges are **control-rate**: the consumer reads
  the producer's *previous-block* value, so they never impose order and
  never create cycles. A feedback LFO is legal; a feedback audio cable is
  an error.

The map: [`AudioGraph::from_project`](../../../core/src/audio/graph.rs)
splits the project's routing into `signal_edges` vs `control_edges`;
[`topo_order`](../../../core/src/audio/graph.rs) sorts signal flow with
Kahn's algorithm and returns `GraphError::Cycle` for audio loops while
ignoring modulation-only loops. Unknown edge endpoints become implicit
zero-latency nodes (mix buses / external ports) instead of errors.

```rust
let graph = AudioGraph::from_project(&project);
let order = graph.topo_order()?; // producers before consumers
```

## 2. Latency compensation is input alignment

A lookahead limiter needs 128 future samples before it outputs one — that
delay is real, and without correction the limited track plays late
(audibly a flam against the dry tracks). The fix: at every mix point,
delay each *faster* input to match the slowest one.

The map: devices declare latency with a `latency_samples` param (inside
the frozen `Param` shape — no schema change). [`output_times`](../../../core/src/audio/graph.rs)
computes when each node's output emerges; [`edge_delay(from, to)`](../../../core/src/audio/graph.rs)
returns the samples of delay to insert on that mix input
(`arrival[to] − out[from]`, slowest path = 0). [`RenderGraph::render`](../../../core/src/audio/render.rs)
realizes them as delay lines. The test renders two impulses — one through
a 64-sample delay, one direct — and asserts the compensated mix peaks at
exactly 2.0 on frame 64 (and shows the uncompensated render flamming at
frames 0 and 64, the bug being fixed).

## 3. Multicore is levelization; remote is a hook

Nodes in one topo level have all their producers in earlier levels, so
they are independent and may run on separate threads.
[`Schedule::build`](../../../core/src/audio/schedule.rs) computes those
levels; the renderer runs each level under `std::thread::scope`, borrowing
buffers read-only with no locks. The test asserts 1-thread and 8-thread
renders are bit-identical.

Remote DSP (another process / machine) is a **phase-2 hook only**:
[`Placement::Remote`](../../../core/src/audio/schedule.rs) and
[`mark_remote`](../../../core/src/audio/schedule.rs) mark addresses, and
[`require_all_local`](../../../core/src/audio/schedule.rs) refuses to
render while any exist (`ScheduleError::RemoteUnimplemented`). Phase 2
adds a transport behind that seam without re-cutting the scheduler.

## 4. The frontend never blocks the audio thread

The audio callback has a hard deadline — miss it and you hear a click —
so it may never wait on the UI. The rule is one-way:

- The **UI thread builds** (owns `Project` → `AudioGraph` → `RenderGraph`,
  all plain data) and `send()`s finished work as `AudioCommand`s
  (`SwapGraph` carries the whole built graph; `SetParam` carries one
  value). Unbounded channel: `send()` never blocks.
- The **audio thread polls**: each callback drains with `try_recv`
  (returns immediately when empty) and renders its local copy. Params live
  in atomics (`ParamBank`); the callback keeps a value cache and refreshes
  it with `try_lock`, so contention degrades to a one-block-stale value,
  never a wait.

The map: [`AudioEngine`](../../../core/src/audio/device.rs) (UI side),
[`NullBackend`](../../../core/src/audio/device.rs) (same contract minus
hardware — what tests prove), [`CpalBackend::open_default`](../../../core/src/audio/device.rs)
(real output: default device, f32/i16/u16 configs, callback renders the
latest swapped graph). The handoff test pumps from one thread while
another sends 10,000 params + `Stop`, and asserts every command arrived —
completion without hanging *is* the no-blocking proof.

## 5. How to verify

- `cargo test --manifest-path core/Cargo.toml` — includes the render-null
  test (silence in = bit-zero out), the latency-compensation test
  (sample-exact alignment + uncompensated flam), the multicore
  bit-identity test, the remote-seam refusal test, and the handoff test.
- `bun run typegen -- --check` — unaffected (Track B reads the frozen
  types and adds none, so no drift is possible).

## 6. What Track B deliberately leaves out

- `src-tauri/src/lib.rs` still holds transport stubs (`engine_play` sets a
  boolean). Opening the device on `engine_play`, closing on `engine_stop`,
  and forwarding `param_set`/`op_apply` as `AudioCommand`s is the
  integration follow-up — `AudioEngine`'s API already matches that shape.
- `Proc` has five variants (`Null`, `Impulse`, `Constant`, `Delay`,
  `Mix`). Real instruments, samplers, and effects add variants without
  touching the graph/schedule/device plumbing.
- No resampling yet: `sample_rate` is read from the device config and
  reserved for the graph-rate ≠ device-rate case in phase 2.
- `ui/src/generated/*` untouched, `model.rs`/`ipc.rs` untouched.

## 7. Freeze/bounce: the same project rendered without a clock

New to offline render? Start here. The realtime engine renders on a hard
deadline (miss it and you hear a click). Freeze/bounce renders the *same*
project with no clock at all: pick a beat window, synthesize each clip in
it deterministically, sum the subtree, and hand back a stem — mono f32
audio plus loop points — ready to encode as a WAV game deliverable.

Teaching note: "freeze" and "bounce" are one verb with two nouns.
**Freezing** a track renders it to a stem asset *inside* the project, so
the realtime graph can later play the file instead of the devices.
**Bouncing** renders one stem per track (or one mix stem) for export.
Both are pure functions over `Project`; only `freeze_track` touches the
`Engine`, and only through its public asset API — so frozen stems ride
along in portable bundles for free.

The map: [`BounceConfig`](../../../core/src/bounce.rs) (a beat window +
sample rate; tempo comes from the project so stems always agree with the
timeline), [`render_track`](../../../core/src/bounce.rs) (one track's
clips × device gains × track volume — ignores mute/solo because a stem
is raw material, not mixer state), [`render_mix`](../../../core/src/bounce.rs)
(everything summed, honoring mute and solo), [`render_stems`](../../../core/src/bounce.rs)
(one stem per track: the game-deliverable batch), [`freeze_track`](../../../core/src/bounce.rs)
(render + `store_asset("freeze-<id>.wav")`), [`encode_wav` /
`decode_wav`](../../../core/src/bounce.rs) (16-bit PCM mono WAV with a
`smpl` chunk carrying the loop points game engines import directly).

```rust
let config = BounceConfig::new(44100, 0.0, 4.0)?;
let stems = render_stems(&project, &config)?; // one WAV-ready stem per track
let key = freeze_track(&mut engine, &project, "trk", &config)?; // travels in bundles
```

Two honest v1 limits (not hidden): clip sources render procedurally —
`builtin:click` audio clips render metronome clicks, MIDI clips render a
loop-clean reference tone (exactly 220 cycles per beat, so whole-beat
windows wrap without a click) — because engine assets are opaque blobs
with no decoder yet. Real sample/MIDI decoding lands behind this same
API. And device chains contribute only `gain`/`volume` params; full DSP
lands with the device track. Pan is ignored: v1 stems are mono, the
standard shape for looped game deliverables.

How to verify: `cargo test --manifest-path core/Cargo.toml bounce` — 10
tests covering exact stem length + full-window loop points, loop-clean
tone edges, volume/device-gain scaling, mute-kills-mix vs
stems-ignore-mute, solo isolation, WAV round-trip (audio + loop within
16-bit quantization), freeze-stores-decodable-asset, out-of-window
silence, clean errors, and a both-tracks bounce of `Project::sample`.
