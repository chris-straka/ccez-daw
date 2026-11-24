# n-integration primer: final verification + release gate

Final integration agent's note. Everything below was observed green on the
integration pass; re-run the gate before release.

## 1. The full gate (run in this order)

From repo root:

1. `cargo test --workspace` — 214 core + 15 integration + 11 Tauri lib tests
2. `bun run test` — ui (118) + mcp (18) + core smoke
3. `bun run check` — typegen `--check` + `tsc --noEmit` (ui) + core tests
4. `cd ui && bun run build` — vite production build
5. `bun run test:e2e` — Playwright web-mode, 4 spec files / 5 tests, chromium

## 2. e2e browser setup

Playwright browsers live outside the repo (`~/.cache/ms-playwright`);
`bunx playwright install chromium` (no `--with-deps`, no sudo needed on
macOS). System Chrome at `/Applications/Google Chrome.app` is the fallback
(`channel: "chrome"` in a local config override — do not commit that).
The e2e `webServer` block auto-starts vite on 127.0.0.1:1420; nothing to
tear down manually.

## 3. Frozen contracts

- `contracts/action-registry.md` is append-only; regenerate types via
  `bun run typegen`, never hand-edit `ui/src/generated/*`.
- Track K owns `ui/src/input/` + `ui/src/palette/` + headless replay tests.
- Integration glue only: if a unit suite fails, the fix belongs to the
  owning track, not here — report file + error and stop.
