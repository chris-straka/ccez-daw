# Frozen v0 contracts

These files are the machine-checked (where possible) source of truth every
track builds against. Breaking change = new version + migration note.

- `ipc-table.md` — Tauri `invoke` commands and `listen` events. Machine truth:
  `core/src/ipc.rs`; TS mirror: `ui/src/generated/ipc.ts`.
- `project-schema.md` — project document shape. Machine truth:
  `core/src/model.rs`; TS mirror: `ui/src/generated/project.ts` (Zod v4).
- `op-log-format.md` — event-sourced op log entry format.
- `action-registry.md` — action ids shared by palette, vim layer, scripting,
  and MCP. Stub mirror: `ui/src/actions/registry.ts`.
- `mcp-tools.md` — MCP tool list. Stub mirror: `mcp/src/tools.ts`.

`bun run check` regenerates the TS mirrors from Rust and fails on drift.

## v1 game-audio contracts (frozen, additive over v0)

Separate top-level documents with their own schema version (`1`). They
reference v0 project data only by string id, so every v0 file above is
untouched. Machine truth: `core/src/game_audio.rs`; TS mirror (appended
last, v0 lines byte-identical): `ui/src/generated/project.ts`.

- `game-state.md` — named states + continuous params (RTPC inputs).
- `adaptive-cue-schema.md` — vertical layers + transition rules per cue.
- `sfx-bank-schema.md` — named SFX events (pools, humanization, throttles, RTPC).
- `export-package.md` — engine deliverable layout + validator rules +
  additive game-audio MCP tool rows (v0 `mcp-tools.md` untouched).
