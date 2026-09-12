# Track P-2 primer: AI sidecar model pins (decisions, not downloads)

New to sidecars? Read `docs/notes/track-m.md` first — it teaches the
background-job + one-interface-seam + editable-output pattern this note
builds on. This note answers v1's deferred open question: **which local
model, if any, does each sidecar get?** Six sidecars, six verdicts —
"unpinned" stops being an option after v3.

The rule behind every verdict: the neural model is the ceiling, the
deterministic baseline is the floor that always works offline. A pin never
changes op shapes or UI — it only changes which provider sits behind the
existing seam (`TranscriptionProvider` / `HttpTranscriptionSidecar` in
`mcp/src/ai/sidecar.ts`, `submit_*` / `apply_*` in `core/src/ai/`). The
DAW runs fully with models absent; weights arrive by download-on-first-use
into a cache dir, never bundled with the app.

## 1. Where pins live and how they resolve

- Rust truth: [`models.rs`](../../../core/src/ai/models.rs) — `PINS`
  (the verdict table), `resolve(sidecar)` (presence check →
  `Baseline` | `Sidecar`), `model_cache_dir()` (`$CCEZ_MODEL_DIR`, else
  `~/.local/share/ccez-daw/models`). No network I/O, no new dependency.
- TS mirror: [`models.ts`](../../../mcp/src/ai/models.ts) —
  `MODEL_PINS`, `resolveProviderFor(kind)` (absent → local baseline
  provider, cached → HTTP sidecar with the pinned id),
  `ensureModelCached(sidecar, weightUrl)` (fetch-once into the same cache
  path, no-op when present, throws for baselines-only sidecars).

Try it: point `$CCEZ_MODEL_DIR` at an empty dir and every sidecar resolves
to baseline (the models-absent tests assert exactly this); drop a file
named like the pin's `file` into it and that sidecar flips to the HTTP
seam. No restart dance, no config edit — presence is one file.

## 2. The verdicts

| Sidecar | Verdict | Size | License |
|---|---|---|---|
| transcribe-drums | **OaF-Drums** (Magenta Onsets-and-Frames drums variant) | ~40 MB checkpoint | Apache-2.0 (re-verify checkpoint header at wire-up) |
| transcribe-melody | **Basic Pitch** (Spotify Audio Intelligence Lab) | ~15 MB ONNX | Apache-2.0 |
| transcribe-chords | **Basic Pitch** (same weights; chord labels from pitch content + template matcher) | shared with melody | Apache-2.0 |
| separation | **htdemucs** 4-stem (Meta AI Research) | ~80–350 MB by variant/precision | MIT (code + weights) |
| cleanup | **DeepFilterNet3** (Schröter et al.) | ~4–30 MB ONNX | MIT/Apache-2.0 dual (re-verify LICENSE at wire-up) |
| groove-transfer | **baselines-only** — no model | 0 | n/a (analytic MIDI transform) |

## 3. Why these, and what was rejected

- **Transcription (Basic Pitch for melody + chords).** Polyphonic,
  instrument-agnostic, pitch-bend aware, built for producers turning
  recordings into MIDI — exactly our commit-an-editable-clip shape. ~15 MB
  ONNX is downloadable on first use without a disk-budget conversation,
  and Apache-2.0 permits the wiring. Rejected: CREPE (monophonic f0 only —
  no predominant-melody-in-a-mix story, larger full model) and pYIN/SWIPE
  (classical DSP, strictly worse robustness than the learned models — that
  lane is already our baseline's job).
- **Drums (OaF-Drums, not Basic Pitch).** Basic Pitch transcribes pitched
  notes; drums are unpitched onsets + classification, a different head.
  OaF-Drums (the drums port of Magenta's Onsets-and-Frames piano model,
  same Apache-2.0 family) is the canonical small open answer, top of the
  perceptual rankings on the standard drum benchmarks. Rejected: ADTLib
  (TF1-era, unmaintained) and PercNN-style one-off checkpoints (no
  canonical small redistributable).
- **Separation (htdemucs 4-stem).** State-of-the-art quality, MIT code +
  weights (bundling-permissive), and its 4 stems are exactly our
  drums/bass/vocals/other commit shape. The size (~hundreds of MB) is why
  this verdict *requires* download-on-first-use and can never ride in the
  installer. Pinned variant is plain `htdemucs` (quality baseline), not
  `_ft` (slower) or `_6s` (six stems would change the commit shape).
- **Cleanup (DeepFilterNet3).** Full-band, real-time, a few MB of ONNX —
  the rare model that is both better than a gate and small enough to
  forget about. Our hysteresis gate stays as the floor (and as the whole
  story for music-content cleanup, which a speech enhancer must not
  touch). License evidence is downstream (dual MIT/Apache-2.0 widely
  reported) — re-verify upstream LICENSE before wiring the download URL.
- **Groove (baselines-only, explicit).** Groove transfer here is
  extract-template + blend-into-target on symbolic MIDI — there is no
  audio understanding to learn, so a neural model buys nothing at any
  size. The HTTP seam stays open if a future *style* model is chosen, but
  nothing is deferred that the baseline can't do today.

## 4. License and size discipline

- Nothing is bundled: every pinned model arrives via
  download-on-first-use after the user asks, into the user-level cache.
  A license problem can therefore never ship in the installer — the worst
  case is a seam that keeps returning baselines.
- Two pins carry a re-verify flag (OaF-Drums checkpoint header,
  DeepFilterNet upstream LICENSE). The flags are in the pin table itself,
  not just in this note — the wire-up task must clear them before
  pasting a final download URL.
- Size discipline: transcription + cleanup pins are tens of MB total and
  could one day be vendored; htdemucs at hundreds of MB stays
  download-only permanently.

## 5. What P-2 added (files)

- `core/src/ai/models.rs` (new): verdict table + `resolve` + cache-dir
  probing, with models-absent / cached-weights / table-completeness tests.
- `core/src/ai/mod.rs`: `pub mod models` + re-export (additive only).
- `mcp/src/ai/models.ts` (new): mirrored table + provider resolution +
  first-use download; `mcp/src/ai/index.ts` re-exports it.
- `mcp/tests/ai-models.test.ts` (new): verdict completeness,
  models-absent → local provider, cached → HTTP sidecar, download-once +
  baselines-only-throws (fake `fetch`, no network).

## 6. How to verify

```sh
cargo test --manifest-path core/Cargo.toml --lib ai::models
cargo test --manifest-path core/Cargo.toml --lib ai
cargo test --manifest-path core/Cargo.toml --test ai_separation_editable
cd mcp && bun test && bun run check
bun run typegen -- --check   # green: no model.rs / ipc.rs changes
```

Models-absent and models-present runs are both covered: the test suites
exercise the baseline path with an empty `$CCEZ_MODEL_DIR` and the seam
path with a fake-cached weight file, so the DAW is proven fully
functional with nothing downloaded.
