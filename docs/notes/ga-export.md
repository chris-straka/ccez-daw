# GA-4 primer: the engine deliverable is a directory, not a project file

New to game-audio export? Start here. This note teaches the one big idea
behind `core/src/gaexport.rs` — **the DAW authors, the game ships data, and
a versioned validator stands between them** — then shows exactly how to
build, validate, and load an export without touching the frozen v1 shapes.

## 1. The idea

Nothing in the DAW project file can play in a game. The engine needs two
things, and only two things:

- **Rendered WAV stems** it can loop (`stems/<cue>_<layer>.wav`) or
  one-shot (`sfx/<event>_<n>.wav`), with loop points in beats.
- **One JSON event bank** per bank (`bank_<id>.json`) saying what to play
  when — the named events (`ui.click`) and cue layer maps the game
  already speaks.

Plus a manifest (`package.json`) tying them together. That directory *is*
the deliverable. The validator's job is making a broken export a red build
(a missing loop, a dangling clip, a typo'd `bus_sfx:volum` address) instead
of a silent in-game bug — every rule in `contracts/export-package.md` is a
failing test in `gaexport.rs`.

## 2. The pieces

- `contracts/export-package.md` — the frozen v1 layout and the five
  validator rules (machine truth: `ExportPackage`, `ExportStem`, `StemKind`
  in `core/src/game_audio.rs`).
- `core/src/gaexport.rs` — `build_package` (validates rules 1–3, renders
  every stem deterministically, assembles the manifest), `write_package`
  (writes the directory), `validate_package` (checks all five rules against
  a written directory). Reuses the bank track's validators
  (`sfx::bank::validate_bank`, `resolve_against_project`) and the
  `bounce` PCM16 mono + `smpl` loop WAV codec —
  anything that reads a frozen stem reads an export stem.
- `core/src/bin/export-validator.rs` — the `export-validator` binary: same
  checks, CLI form. `scripts/validate-export.sh` wraps it.
- `docs/notes/ga-export-loader.md` — the engine-side loader spec: what the
  game does with the directory (load order, beats→samples loop math,
  one-shot vs loop, failure policy).

## 3. Build and write a package (the whole API in fifteen lines)

```rust
use ccez_core::gaexport::{ExportOptions, ExportRequest, build_package, write_package};

let request = ExportRequest::new("demo-v1",
    vec!["cue_fight".into()], vec!["bank_ui".into()]);
let options = ExportOptions::default(); // 48 kHz, seed 2026

match build_package(&project, &cues, &banks, &params, &request, options) {
    Ok(built) => {
        // Manifest stems: stems/cue_fight_bed.wav loops 0..8 beats,
        // sfx/player.footstep_0.wav is a 0, 0 one-shot.
        write_package(std::path::Path::new("out/demo-v1"), &built)?;
    }
    Err(errors) => eprintln!("export rejected:\n- {}", errors.join("\n- ")),
}
```

What the builder decides, so you don't have to:

- One stem per cue layer (`stems/<cue>_<layer>.wav`, `MusicLayer`) and one
  per pooled event clip (`sfx/<event>_<n>.wav`, `SfxClip`, `<n>` = index
  into the event's `clip_ids`).
- Music loop points come from the data: `0` to the longest resolvable clip
  in the layer (`DEFAULT_LOOP_BEATS` when nothing resolves). SFX stems are
  always `0, 0` one-shots of `SFX_ONESHOT_SECONDS`.
- Music renders are loop-clean reference tones at the cue tempo (the bounce
  philosophy — v0 sources are opaque blobs with no decoder yet); SFX renders
  are per-clip stand-in tones with a seed-drawn detune so rule 5 exercises
  a real seeded path. Same project + seed = byte-identical packages.
- `event_bank_path` names the first bank's file (`bank_<id>.json`, written
  with the bank track's own `bank_to_json`, so it re-serializes byte-stable).

## 4. Validate it (the GA-4 gate)

```rust
use ccez_core::gaexport::validate_package;

let report = validate_package(&dir, &project, &cues, &banks, &params, options);
assert!(report.is_ok(), "must not ship: {:?}", report.errors);
```

Or from a shell, against a context bundle
(`{ project, cues, banks, params }`):

```sh
scripts/validate-export.sh out/demo-v1 context.json
# export-validator: out/demo-v1 approved (validator version 1)
```

Rule → test map (each rule is a failing test in `gaexport.rs`):

1. Unknown `cue_ids`/`bank_ids`, wrong `schema_version`/`validator_version`.
2. Dangling layer/event `clip_ids`, empty event pools, duplicate event ids —
   checked on the *shipped bank bytes*, not just the in-memory bank.
3. `Stinger` rules with missing/empty `stinger_cue_id`, stinger ids on
   non-Stinger rules, RTPC bindings to unknown `target_node:target_param`
   (real = device params + track `volume`/`pan`).
4. Loop ranges (`0 <= start < end` for music, `0, 0` for one-shots), every
   stem file present and parsing as WAV at the export rate, music lengths
   matching their beat loops at the cue tempo.
5. The first resolvable stem re-renders byte-identical under the export
   seed — flip one sample byte and the package goes red.

## 5. Rules (frozen schema — read before extending)

- Never add a field to `ExportPackage`/`ExportStem`/`SfxBank` in place.
  `ExportRequest`/`ExportOptions` are caller-side inputs (never serialized)
  precisely so render settings can evolve without a contract version bump.
- The manifest has no seed field: reproducible exports use
  `DEFAULT_EXPORT_SEED` unless the caller passes an explicit seed, and the
  validator takes the same seed back. A seed nobody records is a seed
  nobody can verify.
- No new types were added to the typegen surface, so
  `ui/src/generated/*.ts` is untouched by this track: `bun run typegen --
  --check` stays green. If you add a serialized contract type later, extend
  `core/src`, regenerate, and confirm the v0 diff is purely additive.
- Breaking change = new version + migration note, per every v1 contract.
