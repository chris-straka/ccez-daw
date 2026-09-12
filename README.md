# ccez-daw

A fully featured desktop DAW as a Tauri 2 app (Rust backend + SolidJS/TypeScript
frontend), with native plugin support (CLAP + VST3 + AU), an MCP server,
vim-style keybindings, and AI-assisted editing.

Track 0 scaffold. The approved plan lives at
`.agents/plans/2026-09-12-tauri-daw.md`. Frozen v0 contracts live in
[`contracts/`](./contracts/README.md).

## Layout

- `src-tauri/` — Tauri 2 app shell (Rust). Thin: invokes into `core`.
- `core/` — Rust crate owning project/engine truth (project model, op log, IPC
  shapes) plus the Rust→TS typegen step.
- `ui/` — SolidJS + TypeScript (strict) + Vite + Bun frontend. Never hand-writes
  contract types; imports them from `ui/src/generated/`.
- `mcp/` — Bun/TS MCP server stub on the official MCP TS SDK. Tools mirror the
  action registry.
- `contracts/` — frozen v0 human-readable contracts (IPC table, op-log format,
  action registry, MCP tool list).

## Prerequisites

- Bun >= 1.1, Rust stable, Tauri 2 system deps
  ([prerequisites](https://v2.tauri.app/start/prerequisites/)).

## Commands

```sh
bun install            # install ui + mcp workspaces
bun run typegen        # regenerate TS bindings from Rust (core is source of truth)
bun run check          # typegen drift gate + tsc + core tests (FAILS on contract drift)
bun run test           # all TS + Rust tests
bun run tauri:dev      # launch the DAW shell (proves Solid-in-Tauri cycle)
cargo test --manifest-path core/Cargo.toml   # Rust core tests
```

`bun run check` regenerates the bindings from `core/` and diffs them against the
committed files: if Rust types and TS bindings drift, the build fails.
