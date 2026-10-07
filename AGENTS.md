# AGENTS.md — ccez-daw working conventions

How this repo stays a working DAW instead of a wired-together demo.
Follow these over cleverness.

## Gates (run before finishing)

- `bun run check` — typegen drift + `tsc` + core tests. Zero tolerance.
- `bun run test` — ui + mcp + core suites, all green.
- `bun run test:e2e` — the Playwright specs in `ui/e2e/` against the mocked shell.
- `cargo check --manifest-path src-tauri/Cargo.toml` — after touching commands.

A confident patch you never watched pass the repo's tests is the most
common wrong answer.

## Frozen surfaces grow additively, never by edits

- Op log, project schema, and existing IPC rows are frozen. New backend
  capability = one additive command (`engine_record`, `link_toggle`):
  handler in `src-tauri/src/lib.rs`, `CommandDef` in `core/src/ipc.rs`,
  `bun run typegen --`, re-export in `ui/src/tauri/commands.ts`, one row
  in `contracts/ipc-table.md`.
- The `emit.rs` drift gate allowlists TS primitives; extend it for a new
  primitive rather than working around it.

## Every action must do something real

- A registry entry, keybinding, palette row, or menu item that resolves
  to an inert descriptor is a bug, not a placeholder. `ui/tests/view-nav.test.ts`
  fails the build if any default binding lacks a registered action; keep
  that invariant.
- Global keys (`r`, `S`, `J`, `C`, `L`, `?`, `g`-chords) reach panel-owned
  state through bump-and-show nonce props (`punchNonce`, `launchNonce`,
  …), never by lifting panel state into `App`. Nonce effects fire only on
  change and only once the project has loaded.
- New IPC returning a state the shell displays (e.g. link membership)
  gets a visible indicator, not just a return value.

## Honest sound and honest stubs

- Silence beats fake sound. A missing asset renders zeros, never a tone;
  a panel with nothing to show says so in plain text.
- The live transport loops the pre-rendered mix (`core/src/audio/live.rs`),
  so live and export agree by construction. Never push bare topology and
  call it audible.
- Mocks and green suites don't prove audibility or reachability — a pumped
  block asserted non-silent does; a keypress asserted in e2e does.

## Tests are deliverables

- Bug fix or behavior change ships with the smallest focused test in the
  repo's normal layout (bun test, cargo test, or playwright spec).
  Watch it fail first, then fix.
- Shell-visible behavior gets an e2e spec following `ui/e2e/session.spec.ts`.
- When behavior changes, update the doc that described the old behavior
  (`docs/notes/*`, `contracts/*`) in the same change.

## UI conventions

- Every interactive element gets a `data-testid`. Status lines report
  what happened (`record: take 1 → take_x`), never "done".
- Headless-safe: Tauri calls in panels fail into honest status text so
  the shell runs in a plain browser.
- Vim context follows the workspace; view-scoped keys must be reachable
  from the views they belong to.
