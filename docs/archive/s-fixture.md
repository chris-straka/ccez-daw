# S-1 fixture — a demo TP-like package the template can trust

## The problem in one paragraph

A Godot playback template with no content to load is a player with no
record: every layer switch, slider mapping, and loop wrap stays untested
until a composer authors a real score — weeks later. The fixture track
fixes that by shipping a small but *real* GA-4 export package on day one:
field/combat/boss cues plus a `threat` param and a few SFX events, built
through the same frozen export path a real score will use, approved by the
same validator. If the template plays this package gaplessly, it will play
the real score gaplessly; the bytes only get bigger.

## The mental model: the fixture is a contract example, not a soundtrack

Forget orchestration. The fixture's job is covering the *shape space* the
template must handle, one instance per branch:

- an always-on bed *and* state-gated layers (field/village share strings,
  combat adds drums, night swaps in a pad);
- every transition kind the template implements on at least one rule
  (`Fade` three times, `Stinger` once, `Cut` by sparse fallback);
- a one-shot SFX pool *and* RTPC bindings (`threat` → SFX bus,
  `mounted` → SFX track);
- loop lengths that differ per layer (8 vs 4 vs 2 beats) so beats→samples
  math is exercised, not just copied.

If you add fixture content later, add it to cover an uncovered branch —
not because the demo "needs more music".

## The pieces

- `ga-fixture/src/main.rs` — the builder. Assembles authoring data only
  (v0 `Project`, three `AdaptiveCue`s, one `SfxBank`, four
  `GameStateParam`s) and calls `gaexport::build_package` +
  `write_package`. No audio math lives here; rendering, manifest, and all
  five validator rules stay owned by `core/src/gaexport.rs`.
- `ga-fixture/context.json` — the authoring side the package was exported
  from (`{ project, cues, banks, params }`). Committed, so anyone can
  re-validate or diff what changed since the approved bytes.
- `godot-template/demo-package/` — the handoff: 8 music stems + 7 SFX
  one-shots + `package.json` + `bank_bank_tp.json`, plus a `README.md`
  with the state/param/layer map. Regenerate, never hand-edit.
- `ui/src/gameaudio/fixture.ts` — the in-DAW mirror: same ids/layers/
  bindings as TS builders, plus `validateFixture` (validator rules 2–3 in
  the DAW: dangling clips, empty pools, duplicate events, unknown stinger
  refs, typo'd `node:param` targets) and `describeFixture` for the
  authoring panel. Pinned by `ui/tests/gameaudio-fixture.test.ts`.
- This note's sibling `docs/notes/s-godot.md` §Fixture carries the
  template-side view (what the package proves about the player).

## Regenerate and re-validate (the whole workflow in two lines)

```sh
cargo run --manifest-path ga-fixture/Cargo.toml -- \
  --out godot-template/demo-package --context ga-fixture/context.json
scripts/validate-export.sh godot-template/demo-package ga-fixture/context.json
# export-validator: godot-template/demo-package approved (validator version 1)
```

Same inputs + default seed (2026) = byte-identical package (validator
rule 5 re-renders the first stem and compares). A seed nobody records is
a seed nobody can verify — so the fixture never passes a custom seed.

## Rules (frozen schema — read before extending)

- Never add a field to `ExportPackage`/`ExportStem`/`SfxBank` in place.
  New fixture content only uses existing fields; breaking change = new
  version + migration note, per every v1 contract.
- Never hand-edit `ui/src/generated/*`; the TS mirror builds plain values
  that validate against the generated Zod schemas.
- RTPC targets must name a real `target_node:target_param` in the fixture
  project (devices `bus_sfx`/`bus_music`, tracks `trk_music`/`trk_sfx`);
  unknown *game params* stay quiet per the trigger contract, but a typo'd
  mix target is a red build — `validateFixture` takes `mixTargets` to
  check this in-DAW before export.
- Keep the Rust builder and `fixture.ts` in sync by hand: same cue/layer/
  event/param ids, same states, same bindings. The bun tests pin the TS
  side; the Rust `fixture_builds_and_validates_clean` test pins the
  rendered side (15 stems, validator-ok).
