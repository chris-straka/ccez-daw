# S-4 primer: batch-export every state mix in one pass

New to batch export? Start here. This note teaches the one big idea behind
`core/src/batchexport.rs` — **the game needs one mix per state, and the DAW
should render them all at once from the same bytes the validator approved**
— then shows how to build, validate, and preview a batch without touching
the frozen v1 shapes.

## 1. The idea

A GA-4 package ships one stem per cue layer plus a manifest, and the game
mixes layers live per state (`field`, `combat`, `dungeon`, `boss`,
`village`, `night` — the TP-like set, with params `threat`, `time_of_day`,
`health_low`, `mounted` shaping playback). That is flexible, but shipping a
game means getting six mixes right per cue by hand —FATigue and drift.

The batch export renders **one mix WAV per (cue, state)** in a single pass,
next to the base package, under one manifest (`batch.json`):

```text
<package>/
  package.json        # base GA-4 manifest (untouched)
  bank_<id>.json      # base bank files (untouched)
  stems/<cue>_<layer>.wav
  sfx/<event>_<n>.wav
  variants/<cue>_<state>.wav   # NEW: one mix per cue × state
  batch.json          # NEW: base package verbatim + variant list
```

The game can drop a state mix straight into a level; the per-layer stems
stay available for live crossfades. Both come from one command, one seed,
one validator run.

## 2. Why mixes can't disagree with layers

Variant mixes are **sums of the shipped layer stems** — decoded with the
`bounce` WAV codec, summed in f32, re-encoded with the same codec. The bytes
the game loops are arithmetic over the bytes the validator approved, so a
mix can never drift from its layers.

Two edge rules keep the manifest complete (every cue × every state, no
holes):

- **Shorter layers wrap.** Layers can have different loop lengths (an 8-beat
  bed under 4-beat drums). The mix runs the longest audible loop; shorter
  layers repeat from their own start. Everything is whole-beat loops at one
  tempo, so wrapping stays on the grid.
- **Silence is data.** A state with no audible layers still gets a variant:
  digital silence of `DEFAULT_LOOP_BEATS` at the cue tempo. "No music here"
  is a file the game can load, not a missing file the game must explain.

## 3. Build and write a batch (the whole API in fifteen lines)

```rust
use ccez_core::batchexport::{BatchRequest, build_batch, write_batch};
use ccez_core::gaexport::ExportOptions;

let request = BatchRequest::with_default_states(
    "overworld-batch", vec!["cue_overworld".into()], vec!["bank_ui".into()]);
let options = ExportOptions::default(); // 48 kHz, seed 2026

match build_batch(&project, &cues, &banks, &params, &request, options) {
    Ok(built) => {
        // built.manifest.variants: 1 cue × 6 states = 6 mixes, e.g.
        // variants/cue_overworld_combat.wav layers [bed, drums].
        write_batch(std::path::Path::new("out/overworld-batch"), &built)?;
    }
    Err(errors) => eprintln!("batch rejected:\n- {}", errors.join("\n- ")),
}
```

What the builder decides, so you don't have to:

- Base package first, via `gaexport::build_package` with the same ids,
  seed, and rate — one message per violation, no partial batch.
- One variant per (cue, state) with the audible layer set in cue order
  (`layers_for_state`: empty `states` = always-on bed), the longest-layer
  loop, and the canonical path `variants/<cue>_<state>.wav`.
- Custom state lists are allowed (`BatchRequest::new` with explicit
  states); empty states is a request error, never a silent empty batch.

## 4. Validate it (the S-4 gate)

```rust
use ccez_core::batchexport::validate_batch;

let report = validate_batch(&dir, &project, &cues, &banks, &params, options);
assert!(report.is_ok(), "must not ship: {:?}", report.errors);
```

Check → test map (each check is a failing test in `batchexport.rs`):

- Base GA-4 package approves clean (attributed as `base package: …`).
- Manifest versions are the v1 values; every (cue × state) listed exactly
  once — missing and unexpected variants both fail.
- Each variant's `layer_ids` equal the audible set in cue order; paths are
  canonical; loops are finite and positive.
- Every variant file exists, parses as WAV at the export rate, and holds
  exactly its loop length in samples at the cue tempo.
- The first variant re-mixes byte-identical from the shipped layer stems.

The browser-side mirror (`ui/src/gameaudio/batch.ts`) runs the manifest
half of this without audio: `checkBatchCompleteness` reports missing/extra
variants, wrong layer sets, off-canonical paths, and bad loops —
`isBatchComplete` is the green light. Covered by
`ui/tests/gameaudio-batch.test.ts`.

## 5. Rules (frozen schema — read before extending)

- Never add a field to `ExportPackage`/`ExportStem`/`SfxBank` in place.
  `BatchRequest`/`BatchManifest` are caller-side documents (like
  `ExportRequest`), deliberately outside the `emit` typegen surface, so
  `ui/src/generated/*.ts` is untouched: `bun run typegen -- --check`
  stays green. If a batch shape ever becomes engine-loaded contract,
  extend `core/src`, regenerate, and confirm the diff is purely additive.
- The manifest carries no seed: reproducible batches use
  `DEFAULT_EXPORT_SEED` unless the caller passes an explicit seed, and the
  validator takes the same seed back.
- Breaking change = new version + migration note, per every v1 contract.
