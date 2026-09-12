# GA-0 primer: game audio is events and states, not files and timelines

New to game audio? Start here. This note teaches the one big idea behind
game-audio v2 — **the game posts states and triggers events; the DAW authors
what those mean** — then lists exactly what GA-0 froze so later tracks can
build without forking it.

## 1. The idea

Linear music asks "what plays at 0:42?". Games can't — nobody knows what the
player will be doing at 0:42. So game audio splits into two halves:

- **Adaptive music**: vertical *layers* (drums fade in when combat starts)
  plus horizontal *transitions* (a stinger, then cut on the downbeat). The
  schema (`contracts/adaptive-cue-schema.md`) keeps these separate: `CueLayer`
  says what sounds *now*, `TransitionRule` says what happens *on change*.
  Unwritten transitions cut cleanly — authors score the moments players will
  hear, not an NxN matrix.
- **SFX events**: the game triggers a *name* (`player.footstep`), the bank
  picks a clip, detunes it slightly, and throttles bursts
  (`contracts/sfx-bank-schema.md`). This is the Wwise/FMOD event concept:
  names in the code, sound design in the data.

Both halves read one input model (`contracts/game-state.md`): a named state
plus continuous params. RTPC-style bindings map a game param onto any
`node:param` mix address — the same universal addressing v0 built for
automation, reused, not reinvented.

## 2. The pieces (all GA-0, all frozen)

- `contracts/game-state.md` + `adaptive-cue-schema.md` + `sfx-bank-schema.md`
  + `export-package.md` — the four v1 contracts. Snapshots are evaluated, not
  stored (no ops, no undo); unknown game params ignored quietly, unknown mix
  targets fail loudly.
- `core/src/game_audio.rs` — the Rust truth: cue layer/transition resolution
  (`layers_for_state`, `transition_for`), bank/package shapes, serde
  round-trip tests. References v0 clips by string id; v0 `Project` untouched.
- `core/src/emit.rs` (appended last) → `ui/src/generated/project.ts` — TS +
  Zod v4 mirrors. Regen verified purely additive: `diff` shows only `a`
  hunks, zero `c`/`d` on v0 lines.
- `.agents/plans/2026-09-12-game-audio-v2.md` — the five build tracks
  (runtimes → simulator → export → MCP).

## 3. Extend it (rules)

- Never edit a frozen v1 file in place — breaking change = new version +
  migration note. New params are additive (missing values read as `default`).
- Never hand-edit `ui/src/generated/*.ts`: extend `core/src` and run
  `bun run typegen`, then confirm the v0 diff is still purely additive.
- The engine deliverable is a directory (stems + JSON), never a project file;
  nothing ships with validator errors (`contracts/export-package.md` rule 5
  re-renders a stem for determinism).
