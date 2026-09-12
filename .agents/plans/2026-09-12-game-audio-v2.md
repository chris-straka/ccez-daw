## Goal

Ship interactive/adaptive game audio for the user's game ("game-audio v2") as five tracks against four newly frozen v1 contracts, executed by ~9 agents. The DAW becomes the authoring tool and the game ships data: adaptive music (vertical layering + horizontal resequencing + transition rules on game states), an SFX event system (named events, RTPC-style params, humanization), an in-DAW game-state audition simulator, an engine export package (stems + JSON event bank + validator), and MCP tools for game audio. The user learns game-audio thinking (events not files, states not timelines) as the build progresses.

## Success Criteria

- A demo cue switches layers on a posted `GameStateSnapshot` (`explore` bed → `combat` drums) with a 4-beat fade, per a `TransitionRule`; no rule = clean cut.
- An SFX event (`player.footstep`) plays a random pool clip with per-trigger pitch/volume humanization, drops triggers inside `cooldown_ms`, and bends a mix param via an `RtpcBinding` driven by a game param.
- The audition simulator posts snapshots and triggers events without a game build: state timeline + param sliders + audible result through the real engine graph.
- `gameaudio_export` produces a package directory (stems + `package.json` + bank JSON) that passes all five validator rules; a broken fixture (dangling clip, empty pool, bad loop) fails red with a named rule.
- MCP client lists cues, posts a snapshot audition, triggers an SFX event, and exports a package — mutations that author data land as ordinary ops with a `gameaudio:*` actor.
- `bun run check` stays green throughout; every v0 emitted line byte-identical (additive only); each track ships a `docs/notes/ga-*.md` primer.
- The user's game imports one package and hears adaptive music + SFX driven by its own state posts.

## Context And Current Facts

- Workspace `/Users/c/Swe/ccez-daw`, v1 done and green (14 tracks, frozen v0 contracts in `contracts/`, Rust-to-TS typegen with drift gate via `bun run check`, `docs/notes` primers). GA-0 (this plan) already landed: four frozen v1 contracts + `core/src/game_audio.rs` + appended `emit.rs` entries + regenerated `ui/src/generated/project.ts` (diff verified purely additive — `diff` shows only `a` hunks, zero `c`/`d`).
- V1 contracts (all frozen, `schema_version` 1, machine truth `core/src/game_audio.rs`):
  - `contracts/game-state.md` — named states + continuous params (`GameStateParam/Value/Snapshot`); snapshots are evaluated, not stored (no ops, no undo).
  - `contracts/adaptive-cue-schema.md` — `AdaptiveCue` (vertical `CueLayer`s with empty-`states` = always-on bed; horizontal `TransitionRule`s with `TransitionKind` Cut/Fade/BarWait/Stinger; no rule = cut).
  - `contracts/sfx-bank-schema.md` — `SfxBank`/`SfxEvent` (uniform pool pick, seeded RNG, volume/pitch randomization, cooldown + oldest-steal polyphony, `RtpcBinding` onto `node:param`).
  - `contracts/export-package.md` — package layout + five validator rules + the four additive game-audio MCP tool rows (v0 `mcp-tools.md` untouched).
- Seams v1 reuses: universal `node:param` addressing (RTPC targets need no new addressing), `Clip.source` opaque clip refs (cues/banks reference clips by string id), Track B render path (stems are renders), Track L op-log pattern (authoring mutations as ops).
- Evidence inspected: v0 plan (`.agents/plans/2026-09-12-tauri-daw.md` — style template for this file); all five v0 contracts; `core/src/model.rs`, `ipc.rs`, `emit.rs`, `bin/typegen.rs` (append-last pattern per Track J precedent); `mcp/src/tools.ts` + `backend.ts` (op-log actor pattern); `docs/notes/track-l.md` (primer style template).
- User's first real use case was always music/SFX/stems for their game (v1 plan Open Question 2, default "WAV stems + loop points") — v2 makes that game audio *adaptive* instead of linear.

## Constraints And Non-goals

- Constraints: v0 contracts frozen (breaking change = new version + migration note, never silent drift); never hand-edit `ui/src/generated/*.ts` (extend `core/src` + regenerate); snapshots evaluated not stored; same-render + seed = byte-identical stems; game never touches DAW internals (posts snapshots, triggers event ids).
- Non-goals for v2: networked/multiplayer audio state; DSP-code export to the game engine (stems + JSON data only, no WASM-device shipping); round-robin pool selection (uniform random only); audition-take recording; surround/Atmos beds; baked-in ML (sidecar pattern only, per v1).

## Key Decisions

1. **Separate documents, string-id references.** Cues/banks/packages are top-level v1 documents pointing at v0 clips by id — the frozen v0 `Project`, op log, and IPC table never move. Linking them into `Project` would rewrite a frozen shape; referencing keeps v2 purely additive.
2. **Sparse transition graphs with cut fallback.** Authors write only the transitions players will hear; anything uncovered cuts cleanly. (Rejected: requiring a full NxN matrix, which punishes small games.)
3. **Wwise/FMOD-shaped, not clone-shaped.** Named events, RTPC bindings, cooldown/polyphony throttles — the industry's proven vocabulary in minimal form, so skills transfer both ways.
4. **Tolerance asymmetry.** Unknown *game* params are ignored quietly (games outgrow mixes); unknown *mix* targets fail the validator loudly (typos must be red). Decided at contract level so tracks can't diverge.
5. **Validator as the engine contract.** The five export rules are the real game-engine API: red build beats in-game silence. Determinism rule (re-render one stem) makes exports testable.
6. **MCP tools additive, audition-safe.** New tool names only; `gameaudio_trigger_sfx` and `gameaudio_audition` never write the project (evaluation only), `gameaudio_export` returns the validator report as data either way.

## Recommended Approach

GA-0 (done, this plan + frozen v1 contracts + model + regen) unblocks five parallel tracks GA-1..GA-5 (~9 agents). Rust owns `core/src/game_audio*` (evaluation, runtime, render, validator); Solid owns the simulator UI; `mcp/` bridges. Every track ships a vertical slice plus a `docs/notes/ga-*.md` primer. Rejected: folding cues into v0 `Project`; full transition matrices; engine-side DSP.

## Work Plan

- **Track GA-1 — Adaptive music runtime (2 agents).** Evaluate `GameStateSnapshot`s against `AdaptiveCue`s: `layers_for_state` + `transition_for` (done in GA-0 model), layer gain automation on the Track B graph (Fade over `fade_beats`, BarWait quantize, Stinger one-shot then cut), tempo-per-cue transport. Validation: snapshot-sequence test (`explore`→`combat`→`explore` asserts audible-layer sets + fade ramp samples); no-rule cut test.
- **Track GA-2 — SFX event runtime (2 agents).** Trigger path: uniform pool pick (seeded RNG), per-trigger volume/pitch humanization, cooldown drop, oldest-steal polyphony, per-trigger RTPC evaluation onto `node:param`. Validation: seeded determinism test (same seed = same clip/pitch sequence); throttle test (burst of N triggers inside cooldown yields 1 voice); RTPC mapping test (param extremes hit `[min, max]`).
- **Track GA-3 — Game-state audition simulator (2 agents).** Solid UI: named-state timeline lane, param sliders (from declared `GameStateParam`s), event trigger pads, audible-layer/event readout — posting snapshots through the real GA-1/GA-2 runtimes, no game build needed. Validation: scripted audition test (post snapshot → assert audible set; move slider → assert RTPC target value).
- **Track GA-4 — Export package + validator (2 agents).** Per-layer/per-event stem renders (48 kHz/24-bit WAV, beat loop points at cue tempo), `package.json` + bank JSON writer, five-rule validator CLI with named-rule failures, determinism re-render check. Validation: golden package test on the demo cue+bank; one failing fixture per validator rule (5 red tests).
- **Track GA-5 — MCP tools for game audio (1 agent).** Four additive tools per `export-package.md` (`gameaudio_list_cues`, `gameaudio_audition`, `gameaudio_trigger_sfx`, `gameaudio_export`) on the Track L handler pattern (plain args in, JSON out; authoring mutations as `gameaudio:*` ops; v0 six tools untouched + migration note for the additive rows). Validation: MCP client lists cues, auditions a snapshot, triggers an event, exports a package and reads the validator report.

Dependencies: GA-0 → all; GA-3 needs GA-1+GA-2 runtimes; GA-4 needs GA-1+GA-2 shapes (renders via Track B path); GA-5 needs GA-1+GA-2 (audition/trigger) + GA-4 (export). Seams: game posts snapshots/event ids only; UI/Rust boundary still generated types + contracts.

## Validation Plan

- Per track: its listed tests plus `cargo test -p ccez-core game_audio` (or full `bun run check`); all green required.
- Integration: author demo cue + bank → audition state change in simulator → export package → validator green → load stems + bank JSON in the user's game, hear layers switch on real game state.
- Highest-risk validation: **GA-1 glitch-free layer switching under snapshot bursts + GA-4 validator catching all five breakage classes** — proven before simulator polish.
- Contract validation: `bun run typegen -- --check` (drift gate) + `diff` showing zero `c`/`d` hunks on v0 emitted lines after every regen.

## Risks / Rollback

- BarWait needs transport/quantize truth from Track B/F — if the bar grid isn't queryable, ship Cut+Fade+Stinger first, BarWait behind a flag.
- Seeded determinism across render paths (native vs bounce) may wobble — validator re-renders through one path only; cross-path equality is explicitly out of scope.
- Game-engine loop-point interpretation (beats→samples at cue tempo) is an engine-side concern — package includes cue tempo per stem metadata; drift there is a game bug with a readable manifest, not silent audio.
- Rollback: feature flags per track; v1 contracts versioned (`1`); breaking change = new version + migration note.

## Open Questions

1. Which engine + audio middleware (if any) does the user's game use — raw stems/JSON, or must the bank also export FMOD/Wwise-compatible.decode? Default: raw WAV + JSON first, middleware mapping later.
2. What are the game's real states and params (names, ranges)? Default: demo `explore`/`combat` + `threat` ships first; real names land as additive params (missing = `default`).

## Sources

- `.agents/plans/2026-09-12-tauri-daw.md` (style template + v1 context)
- `contracts/README.md`, `contracts/game-state.md`, `contracts/adaptive-cue-schema.md`, `contracts/sfx-bank-schema.md`, `contracts/export-package.md` (frozen v1)
- `core/src/game_audio.rs`, `core/src/emit.rs`, `core/src/model.rs`, `core/src/ipc.rs` (machine truth + typegen)
- `docs/notes/track-l.md` (primer style template)
