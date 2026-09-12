# v1 game-state param model (frozen)

Machine truth: `core/src/game_audio.rs` (`GameStateParam`, `GameStateValue`,
`GameStateSnapshot`). Generated TS + Zod v4: `ui/src/generated/project.ts`
(appended after the v0 shapes; v0 lines byte-identical).
`schema_version` on game-audio documents is always `1` in v1.

## Idea

The game speaks to audio in two ways: a **named state** (`explore`,
`combat`, `menu` — an open string set the game defines) plus **continuous
parameters** (`threat` 0–1, `speed` in m/s, `health` 0–100). Every adaptive
behavior in v1 (cue layers, SFX RTPC) reads this model and nothing else, so
the game never touches DAW internals — it posts snapshots, audio reacts.

## Shapes

- `GameStateParam { id, label, min, max, default, unit }` — the declared
  input surface, mirroring `Param`'s range metadata without sharing its type
  (v0 never moves). `unit` is free text (`""`, `"m/s"`, `"hp"`).
- `GameStateValue { param, value }` — one live value naming a declared param.
- `GameStateSnapshot { state, values }` — one posted frame: the named state
  plus all continuous values. Values outside `[min, max]` are clamped by the
  consumer (GA-1/GA-2 own clamping tests); unknown `param` names are ignored,
  never an error (the game ships params the mix doesn't use yet).

## Rules (GA-1/GA-2 implement, GA-3 simulates)

- Snapshots are evaluated, not stored: no op-log entries, no undo. (Audition
  recordings, if wanted, are a later version, not silent scope creep.)
- Normalization is the consumer's job: RTPC maps `[min, max]` onto its output
  range; layers match on the `state` string exactly.
- New params are additive: declare a new `GameStateParam`; existing snapshots
  keep validating (missing values read as `default`).
