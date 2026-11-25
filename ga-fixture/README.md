# ga-fixture — demo TP-like GA-4 export package builder

Owns the S-1 fixture: a demo Twilight-Princess-like package (field/combat/
boss cues, `threat`/`time_of_day`/`health_low`/`mounted` params, four SFX
events) built through the frozen GA-4 export path and handed to the
template track at `godot-template/demo-package/`.

## Layout

- `src/main.rs` — `ccez-fixture` binary: assembles authoring data and calls
  `gaexport::build_package` + `write_package`. No audio math, no contract
  shapes — rendering and validation stay owned by `core/src/gaexport.rs`.
- `context.json` — committed authoring side (`{ project, cues, banks,
  params }`); the validator's second argument and the diff base.
- Regeneration command (see `godot-template/demo-package/README.md`).

## Invariants

- Same inputs + default seed = byte-identical package (rule 5).
- `Cargo.toml` carries its own `[workspace]` so this crate never edits the
  root or core manifests — parallel-track safe.
- TS mirror: `ui/src/gameaudio/fixture.ts` (same ids/bindings) with
  `ui/tests/gameaudio-fixture.test.ts`. Keep both sides in sync by hand.
