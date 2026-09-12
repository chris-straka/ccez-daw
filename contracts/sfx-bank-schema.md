# v1 SFX bank schema (frozen)

Machine truth: `core/src/game_audio.rs` (`SfxBank`, `SfxEvent`,
`RtpcBinding`). Generated TS + Zod v4: `ui/src/generated/project.ts`.
`schema_version` is always `1` in v1.

## Idea

The game never triggers audio files — it triggers **named events**
(`ui.click`, `player.footstep`, `door.creak`), and the bank decides what
sounds. This is the Wwise/FMOD event concept in minimal form: one event id,
a pool of clips, humanization so repeats don't machine-gun, throttles so
bursts don't stack, and RTPC-style bindings so game parameters bend the mix
live.

## Shapes

- `RtpcBinding { param, target_node, target_param, min, max }` — game param
  `param` (declared in `game-state.md`) drives the universal address
  `target_node:target_param` (same `node:param` shape as v0 `ParamAddress`,
  reusing the anything-modulates-anything seam). The game value is clamped to
  the declared param range, normalized to 0–1, then mapped linearly onto
  `[min, max]`.
- `SfxEvent { id, name, clip_ids, volume, volume_random, pitch_random,
  cooldown_ms, max_polyphony, rtpc }`:
  - **Pool**: each trigger picks a `clip_ids` entry uniformly at random
    (round-robin is a later version, not a silent default change).
  - **Randomization**: `volume_random` is a +/- linear range around `volume`;
    `pitch_random` is +/- semitones (0 = no detune). Both evaluated per
    trigger from a seeded RNG so exports are reproducible.
  - **Throttles**: triggers inside `cooldown_ms` of the last accepted trigger
    are dropped; voices beyond `max_polyphony` steal the oldest (never stack
    unboundedly).
- `SfxBank { schema_version, id, name, events }` — one shippable bank;
  serialized to JSON verbatim as the engine's event bank (see
  `export-package.md`). Event `id`s are unique within a bank (validator
  rejects duplicates).

## Rules (GA-2 implements, GA-4 validates, GA-5 exposes)

- Unknown game params in `rtpc` are ignored per trigger (same tolerance as
  snapshots); unknown `target_node:target_param` addresses fail the export
  validator (a typo'd mix target must be loud, a future game param quiet).
- Empty `clip_ids` fails validation — an event with no sound is an authoring
  bug, not silence.
- No new op kinds in v1: authoring a bank lands as ordinary v0 ops in a later
  track only via a new contract version; GA-2's runtime reads banks as data.
