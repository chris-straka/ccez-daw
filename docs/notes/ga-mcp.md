# GA-5 primer: your game talks to the DAW through four verbs

New to MCP (Model Context Protocol)? Start here. This note teaches the one
big idea behind GA-5 — **the game (or an agent driving it) posts states and
triggers events by name; it never touches DAW internals** — then lists
exactly what GA-5 added so later tracks can extend it without forking it.

## 1. The idea

A game engine cannot ask the DAW "what plays at 0:42?" — it doesn't know
what the player will be doing at 0:42. So the MCP surface speaks the same
language the rest of game-audio v2 speaks:

- **States, not timelines.** `gameaudio_audition` posts a snapshot
  (`{ state: "combat", values: [{ param: "threat", value: 0.8 }]}`) and gets
  back which layers are audible in every cue, plus which transition rule
  fired. No rule written for that change? Clean cut — sparse graphs, not NxN
  matrices (`contracts/adaptive-cue-schema.md`).
- **Events, not files.** `gameaudio_trigger_sfx` names an event
  (`player.footstep`); the bank picks the clip, detunes it, and throttles the
  burst. Names in the code, sound design in the data — the Wwise/FMOD shape
  (`contracts/sfx-bank-schema.md`).
- **Reads, not writes.** Audition and trigger are *evaluation-only*: they
  never touch the project, never append ops, never need undo. Snapshots are
  evaluated, not stored (`contracts/game-state.md`).
- **Packages, not projects.** `gameaudio_export` builds the engine
  deliverable (stems + bank JSON + manifest) and returns the five-rule
  validator report *as data* — red builds are readable errors, never silent
  in-game bugs (`contracts/export-package.md`).

`gameaudio_list_cues` is the front door: what cues exist, what layers each
has, which states wake them.

## 2. The pieces (all GA-5, all additive)

- `mcp/src/gameaudio.ts` — the store + evaluation: `GameAudioStore`
  (seeded demo cue `cue_demo` + bank `bank_demo` so the tools answer out of
  the box), `layersForState` / `transitionFor` (mirroring
  `core/src/game_audio.rs`), seeded-RNG trigger path (uniform pick,
  humanization, cooldown drop, oldest-steal polyphony, RTPC onto
  `node:param`), `buildPackage`, and the five-rule `validatePackage`.
- `mcp/src/tools.ts` — four appended rows (`TOOLS` positions 7–10; the
  frozen v0 six stay first and byte-identical in order). Tool names are the
  frozen `export-package.md` rows; each `action` id uses the
  `gameaudio.*` verb family (`list_cues`, `audition_cue`, `trigger_event`,
  `export_bank`).
- `mcp/src/backend.ts` — one additive op kind (`GameAudioExported`) plus
  `logGameAudioExport()`: a *green* export lands one ordinary op under actor
  `gameaudio:export`. Red reports return as data and write nothing.
- `mcp/src/index.ts` — four appended input schemas + one shared
  `GameAudioStore` per server (including the per-request servers on the
  stateless HTTP path, so audition state survives across requests).
- `mcp/tests/gameaudio.test.ts` — the track validation: client lists cues,
  auditions `explore`→`combat`→`explore` (bed only → bed+drums + 4-beat fade
  → clean cut), triggers with RTPC extremes, exports green (op actor
  asserted), and fails red on dangling clips, empty pools, duplicate events,
  bad stingers/targets, and bad loop ranges.

## 3. Tolerance asymmetry (the rule you'll trip over)

Unknown **game** params are ignored quietly — games outgrow mixes, and an
audition with a param nobody bound yet must still play. Unknown **mix**
targets (`node:param` nothing owns) fail the validator loudly — that's
almost always a typo, and typos must be red. Decided at contract level, so
every track behaves the same: `trigger` skips unbound params without a
word; `validatePackage` rule 3 names the bad address.

## 4. Honest boundaries (what GA-5 does NOT do)

- **No render path in MCP.** `buildPackage` emits stem *paths* with `0, 0`
  one-shot loop markers; rule 4's file-existence check reports `na`
  ("GA-4 CLI owns it") instead of a false green, and rule 5 checks manifest
  determinism (rebuild byte-equality) while stem-byte re-render belongs to
  GA-4. Rule 4 still enforces loop ranges (`0 <= start < end`, `0, 0` =
  one-shot).
- **No authoring tools.** Cues/banks are authored via `GameAudioStore`
  `addCue`/`addBank` (the test/authoring seam, like calling
  `backend.undo()` directly in Track L tests) — there is deliberately no
  MCP tool that rewrites game-audio data yet. If one lands later, its
  mutations must land as ordinary `gameaudio:*` ops, per the v2 success
  criteria.
- **No contract edits.** `contracts/mcp-tools.md` (v0 six) and
  `contracts/export-package.md` (v1, incl. the four tool rows) are frozen
  and untouched — this track only implements the rows GA-0 already froze.

## 5. Try it (no game build needed)

Through any MCP client over the `ccez-daw` server:

1. `gameaudio_list_cues { projectId }` → see `cue_demo` (bed always on,
   drums on `combat`).
2. `gameaudio_audition { state: "combat", values: [{ param: "threat",
   value: 0.8 }] }` → audible `["bed", "drums"]`, transition `Fade/4`.
3. `gameaudio_trigger_sfx { eventId: "player.footstep" }` → pool pick +
   humanized volume/pitch + `bus_sfx:volume` from `threat`.
4. Author a cue/bank against real project clips, then `gameaudio_export
   { name, cueIds, bankIds }` → read the per-rule report; green lands a
   `GameAudioExported` op you can find in the op log.
