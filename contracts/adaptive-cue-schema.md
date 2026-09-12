# v1 adaptive cue schema (frozen)

Machine truth: `core/src/game_audio.rs` (`AdaptiveCue`, `CueLayer`,
`TransitionRule`, `TransitionKind`). Generated TS + Zod v4:
`ui/src/generated/project.ts`. `schema_version` is always `1` in v1.

## Idea

Adaptive music has two axes, and the schema keeps them separate:

- **Vertical layering** — which stacks of clips sound together *right now*.
- **Horizontal resequencing + transitions** — what happens *when the game
  state changes*.

A cue references v0 clips only by string `clip_ids` (same opaque-source
model as `Clip.source`); the frozen v0 `Project`, timeline, and mixer shapes
are untouched.

## Shapes

- `CueLayer { id, name, clip_ids, states, volume }` — one vertical stack.
  Audible when any entry of `states` equals the current snapshot state; an
  **empty `states` means always-on bed** (underscore that never drops).
  `clip_ids` in one layer render to one stem (see `export-package.md`).
- `TransitionKind`: `Cut | Fade | BarWait | Stinger`.
  - `Cut` — switch layers immediately.
  - `Fade` — crossfade over `fade_beats`.
  - `BarWait` — hold the old layers until the next bar line, then cut.
  - `Stinger` — play `stinger_cue_id` once, then cut on its downbeat.
- `TransitionRule { id, from_state, to_state, kind, fade_beats,
  stinger_cue_id }` — exact-match on `(from_state, to_state)`.
  `stinger_cue_id` must be non-empty iff `kind` is `Stinger` (validator
  rejects otherwise). `fade_beats` is ignored unless `kind` is `Fade`.
- `AdaptiveCue { schema_version, id, name, tempo, default_state, layers,
  transitions }` — one cue. `tempo` is the cue's own BPM (the engine
  converts beat loop points with it); `default_state` sounds before the game
  posts its first snapshot.

## Rules (GA-1 implements, GA-3 simulates, GA-4 validates)

- No rule for a state change = `Cut` to the target state's layers. The graph
  is sparse by design; authors only write the transitions players will hear.
- Referenced `clip_ids` must exist in the project; dangling ids fail the
  export validator (never silently dropped).
- Layer `volume` multiplies the clip renders (linear, like track volume).
