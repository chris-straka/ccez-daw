# Track M primer: AI sidecars (output that stays undoable)

New to sidecars? Start here. This note teaches the three ideas behind
Track M — **background jobs, the sidecar pattern, and editable output** —
then maps each one to the exact function that implements it. The frozen
rules it obeys live in `contracts/op-log-format.md`; the note model lives
in `core/src/midi/`, the audio stem model in `core/src/bounce.rs`.

Two agents split the track: transcription (drums / melody / chords,
§1–§4) and separation + cleanup + groove-transfer (§7). Both halves
share one rule — a sidecar never writes flattened audio or frozen MIDI;
it yields ordinary ops under an `ai:<sidecar>` actor.

## 1. Transcription is a background job, never a modal

Transcribing audio blocks the musician if it blocks the thread. So every
kind (drums / melody / chords) runs through a job handle the DAW polls:

- Rust: [`Job::submit`](../../../core/src/ai/job.rs) spawns a worker
  thread with a [`JobControl`] (progress + cancel). Poll with `poll()`,
  collect with `try_take()` / `wait(timeout)`, abandon with `cancel()`.
  Dropping the handle joins the thread; a detached worker still finishes
  but its result is discarded.
- TS: [`submitTranscription`](../../../mcp/src/ai/jobs.ts) returns a
  `TranscriptionJob` with the same `poll()` / `wait()` / `cancel()`
  shape. Progress is best-effort on both sides — output correctness never
  depends on it.

Try it: submit drums/melody/chords, watch `poll()` go
`pending → running → done`, then `wait()` for the plan.

## 2. The sidecar pattern (models unpinned)

The neural model is the ceiling; the deterministic baselines are the
floor that always works offline. The seam is exactly one interface, so
swapping models changes plans, never op shapes or UI:

- Rust: `core/src/ai/{drums,melody,chords}.rs` hold the baselines
  (onset thresholding + accent voices, pitch-run segmentation, per-bar
  triad template match). A future HTTP sidecar serves any model behind
  the same `TranscriptionPlan` shape.
- TS: [`TranscriptionProvider`](../../../mcp/src/ai/sidecar.ts) is the
  only seam a model touches — `LocalTranscriptionProvider` (default,
  deterministic) vs `HttpTranscriptionSidecar` (POSTs
  `{ model, kind, input }`; the model id is a plain string like
  `"whisper-large-v3"`, nothing pins a version). `transcribeWith` is the
  entry point. Compare the NL layer's identical split in
  `mcp/src/nl/sidecar.ts`.

The DAW runs fully without any of this: the baselines are pure functions
with no new dependencies, and no provider is ever on the playback or
open-project path. The `UnreachableProvider` test helper proves it — the
offline path never calls a model.

## 3. Output is editable notes, never frozen audio

A transcription produces a [`MidiClip`](../../../core/src/midi/clip.rs):
ordinary notes you can reshape before committing. Committing is one
frozen op, applied with an `ai:<sidecar>` actor, so undo/redo, autosave,
and inspection work for free:

```rust
let clip = detect_drums(&frames, 4.0, 0.5, 4.0)?;
clip.save_to_engine(&mut engine, "take:drums-ai")?; // midi-kind asset
let draft = Clip { source: "take:drums-ai".into(), .. };
engine.apply("ai:transcribe-drums", OpKind::ClipAdded, &draft.id, &json)?;
engine.undo()?; // the AI clip is gone — it was ordinary history
```

The map: [`ai_actor`](../../../core/src/ai/mod.rs) mints `ai:<id>`
sidecar ids (`transcribe-drums` / `-melody` / `-chords`, checked against
`engine::valid_actor`), [`draft_clip_added`] builds the single
`ClipAdded` draft, and [`TranscriptionPlan`] carries
`summary + ops[] + warnings[]` (TS: `mcp/src/ai/types.ts`, applied via
`NlOpStore::applyPlan` in tests and `op_apply` in production).

Voice cheat-sheet for the drums baseline: energy ≥ 0.66 → kick (36),
≥ 0.33 → snare (38), else hat (42), all on channel 10 (index 9);
downbeats bias one step stronger. Melody emits velocity-96 segments;
chords emit root-position pads at velocity 80 plus `labels` (`C`, `Am`).

## 4. What Track M added (files)

- `core/src/ai/` (new, owns Rust truth): `mod.rs` (actors, `OpDraft`,
  `TranscriptionPlan`, `draft_clip_added`), `job.rs` (background
  handle), `drums.rs` / `melody.rs` / `chords.rs` (baselines +
  `submit_*` jobs). `core/src/lib.rs` gains `pub mod ai;`.
- `mcp/src/ai/` (new, owns TS mirror): `types.ts`, `local.ts`
  (deterministic baselines kept in sync with Rust), `sidecar.ts`,
  `jobs.ts`, `index.ts`.
- Tests as validation: each Rust baseline has a
  `*_job_yields_editable_undoable_output` test (transcribe → edit →
  store asset → `apply` with `ai:` actor → `undo` removes it);
  `mcp/tests/ai.test.ts` does the same three loops through a background
  `TranscriptionJob` plus the unpinned-model / offline assertions.

## 5. How to verify

- `cargo test --manifest-path core/Cargo.toml ai` — the nine Track M
  tests (job progress/cancel, three editable-undoable loops, actor
  rule, plan round-trip, input rejection).
- `cd mcp && bun test && bun run check` — the four Track M tests plus
  the untouched NL/MCP suites, and a clean `tsc`.
- `bun run typegen -- --check` — unaffected (Track M touches neither
  `model.rs` nor `ipc.rs`, so the drift gate stays green).
- Agent 2 (separation / cleanup / groove) adds 6 unit + 5
  integration tests — see §8 for its commands.

## 7. Separation + cleanup + groove sidecars (agent 2)

Transcription turns audio into notes. This half goes the other two
directions: audio into *more audio* (stems, cleaned takes) and feel from
one MIDI clip into another. Same job handle, same commit rule.

**Separation: one mix in, four editable stems out.**
[`separate_mix`](../../core/src/ai/separation.rs) splits mono audio into
drums / bass / vocals / other with a deterministic DSP baseline —
LP250 for bass, a rectified-derivative transient mask for drums, a
300–3000 Hz mid-band slice of the tonal residual for vocals, everything
else in other. Every stage except the drum mask is linear subtraction,
so the stems sum back to the mix sample-for-sample (asserted in tests
as `conservation_error < 1e-4`): the split can leak between stems but
can never lose audio. Each stem commits as a WAV asset
(`ai-separation-<id>-<stem>.wav`, encoded with
[`encode_wav`](../../core/src/bounce.rs) so game engines can import it)
plus one frozen `ClipAdded` op — four clips you can move, mute, or
re-freeze, each undoing independently:

```rust
let mut job = submit_separation(mix, sample_rate); // background Job
let sep = job.wait(Duration::from_secs(30)).flatten().expect("done");
let seqs = apply_separation_to_engine(&mut engine, "ai:separation", &sep, &dest)?;
engine.undo()?; // one stem gone; three more undos clear the rest
```

**Cleanup: a gate that adds, never replaces.**
[`cleanup_audio`](../../core/src/ai/separation.rs) removes DC offset,
then runs a hysteresis noise gate (10 ms attack, 100 ms release) and
records the gated regions for UI display. The cleaned take commits as
one new WAV clip (`ai-cleanup-<id>.wav`, actor `ai:cleanup`); the noisy
original stays where it was. If the threshold was wrong, re-run or
undo — nothing was destroyed.

**Groove transfer: feel as data, applied as a new clip.**
[`transfer_groove`](../../core/src/ai/separation.rs) extracts a
[`GrooveTemplate`](../../core/src/midi/groove.rs) from a source clip
and blends it into a target at `amount` 0..=1, returning a *new*
[`MidiClip`](../../core/src/midi/clip.rs) — inputs untouched, every
note still validating. It commits like a transcription: MIDI asset
(`take:groove-<id>`) + one `ClipAdded` under `ai:groove-transfer`.

The map: `SIDECAR_SEPARATION` / `SIDECAR_CLEANUP` / `SIDECAR_GROOVE`
become actors via [`ai_actor`](../../core/src/ai/mod.rs);
`submit_separation` / `submit_cleanup` / `submit_groove_transfer` wrap
the pure functions in the shared [`Job`](../../core/src/ai/job.rs)
(progress per 4096-sample chunk, cancel-aware, cancelled jobs yield
`None` — no partial stems, ever); `apply_*_to_engine` store assets and
append the `ClipAdded` drafts.

v1 limits (honest, not hidden): separation is an energy/band split, not
a neural stemmer — expect leakage, especially vocals/other on sustained
tones. Cleanup is a gate, not spectral repair (Track I owns that). A
real model slots behind the same `submit_*` / `apply_*` shapes.

## 8. Agent-2 files and verification

- `core/src/ai/separation.rs` (new): baselines + jobs + commit
  helpers for all three sidecars, with unit tests (bass/click routing,
  conservation, gate floor/burst, DC removal, groove timing shift +
  amount-0 identity, input rejection).
- `core/tests/ai_separation_editable.rs` (new): the editable-output
  validation — each job runs on a worker thread, its output commits
  through `Engine::apply` with an `ai:` actor, stem WAVs decode and
  reconstruct the mix, the MIDI asset round-trips, and `undo` removes
  every committed clip; plus actor-rule and cancel-terminal tests.
- `core/src/ai/mod.rs`: adds `pub mod separation` + re-exports and the
  three sidecar ids (transcription lines untouched).

```sh
cargo test --manifest-path core/Cargo.toml --test ai_separation_editable
cargo test --manifest-path core/Cargo.toml --lib ai::
bun run typegen -- --check   # green: no model.rs / ipc.rs changes
```

## 9. What Track M deliberately leaves out (both halves)

- No new `OpKind` and no new MCP tool: transcription reuses `ClipAdded`
  through existing surfaces (a seventh tool or kind would be a contract
  break needing a version + migration note).
- No audio I/O: inputs are energy envelopes, pitch tracks, and chroma
  vectors supplied by the caller. Wiring real DSP/model inference to
  those inputs is the follow-up — the job, actor, and op shapes already
  match what it will need.
- No UI: previewing `plan.notes` / `plan.labels` before commit belongs
  to a later track; the data it needs is already on the plan.
