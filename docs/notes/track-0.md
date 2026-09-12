# Track 0 primer: what a DAW is, and what Track 0 froze

New to DAWs? Start here. This note teaches the three big ideas behind this
repo — **project**, **engine**, **scaffold** — then lists exactly what
Track 0 locked down so later tracks can build without re-deciding it.

## 1. A DAW in one paragraph

A Digital Audio Workstation does four jobs: it **stores the musical idea**
(the project document), **turns it into sound on time** (the audio engine),
**hosts instruments and effects** (plugins), and **lets the human edit fast**
(timeline, mixer, piano roll, keybindings). Everything in this repo maps
onto one of those jobs — if a new feature doesn't serve one of the four,
it probably doesn't belong yet.

## 2. The three ideas this repo is built on

**Project — the musical idea, stored as history, not state.**
Instead of saving "the current arrangement", we save every edit as an *op*
(`ClipMoved`, `ParamSet`, …) in an append-only log. Current state = replay
the log; undo = step back; crash recovery = the log is already on disk.
See `contracts/op-log-format.md` and `Op`/`OpKind` in `core/src/model.rs`.
One decision buys autosave, infinite undo, and (later) branching and merge.

**Engine — the thing that makes sound on time.**
The audio thread renders the routing graph on a hard deadline and can never
wait on the UI, so all state lives in Rust (`core/`) and the SolidJS
frontend only holds a rendered copy fetched over IPC. The mixer, piano
roll, and timeline are views over one routing graph (`Edge`), and every
knob and fader is the same abstraction — a `node:param` address with a
value — so automation and modulation write to addresses, not widgets.
Track 0 ships engine *stubs* (`Stopped`/`Playing`, tempo) with the real
audio render loop left to a later track.

**Scaffold — the contract that lets tracks work in parallel.**
`core/` (Rust) owns truth; TypeScript types are *generated from* the Rust
types by `bun run typegen`, and `bun run check` fails the build if the
committed mirrors drift. Nothing crosses the UI/Rust boundary except the
frozen tables in `contracts/` (see `contracts/README.md`). That is the
whole trick: tracks can work independently as long as they don't break
the frozen surface — or version it deliberately (breaking change = new
version + migration note).

## 3. What Track 0 froze (the v0 surface)

- **Project document** (`contracts/project-schema.md`, truth in
  `core/src/model.rs`): `NodeKind`/`Node`/`Edge`, `Param`/`ParamAddress`,
  `Track`/`Clip`/`AutomationLane`, `Project` with a `sample()` fixture.
- **Op log** (`contracts/op-log-format.md`): `Op` = sequence number +
  actor + kind + target + JSON payload; six `OpKind`s
  (`TrackAdded`, `ClipAdded`, `ClipMoved`, `ParamSet`, `TempoSet`,
  `UndoMarker`). Every edit — mouse, keybinding, script, MCP tool, AI
  sidecar — produces ordinary ops from a named actor, so AI output stays
  editable and undoable instead of flattened audio.
- **IPC table** (`contracts/ipc-table.md`, truth in `core/src/ipc.rs`):
  13 Tauri `invoke` commands and 4 `listen` events
  (`project_changed`, `engine_state_changed`, `param_changed`,
  `op_applied`), wired end-to-end in `src-tauri/src/lib.rs`.
- **Action registry** (`contracts/action-registry.md`): 10 action ids shared
  by palette, vim layer, scripting, and MCP (`ui/src/actions/registry.ts`,
  cross-checked against IPC by `ui/tests/contracts.test.ts`).
- **MCP tools** (`contracts/mcp-tools.md`): 6 frozen tools
  (`mcp/src/tools.ts`), stubs throwing until their owner track lands.
- **Pipelines** (`.github/workflows/`): `check.yml` compiles every desktop
  target on every push; `release.yml` cuts per-arch desktop bundles from a
  version tag through a strict sequential chain with a verify gate — both
  adopted from ccezstudio's hard lessons (parallel draft uploads silently
  lose files; per-target-only breakage). Details in the workflow headers.

## 4. How to verify any of this

- `bun run check` — typegen drift gate + `tsc` + core Rust tests.
- `bun run test` — UI contract tests + MCP tests + core tests.
- `cargo check --locked --workspace` — whole Rust workspace compiles.
- Push a tag like `v0.1.0` — release CI builds, verifies, and publishes.
