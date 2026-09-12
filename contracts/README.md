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
