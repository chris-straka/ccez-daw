# n-e2e primer: Playwright web-mode e2e harness

New to the e2e harness? Start here. This note covers **what the harness
proves**, **how the backend mock works**, and **how to add a spec** — the
three things every later track needs when touching UI behavior.

## 1. What it proves (and what it does not)

Four spec files (`ui/e2e/*.spec.ts`, five tests — `shell` carries two), run with `bun run test:e2e`
(root script; deliberately excluded from default `bun run test` so unit
runs stay fast):

- `shell.spec.ts` — the shell loads all five regions (Timeline, Mixer,
  Browser, Piano roll, Command palette) and the Timeline row renders the
  canned `project_get` payload (proves `invoke` is live, not just markup).
- `palette.spec.ts` — Ctrl+K opens the command palette, typing
  `Transport: Play` + Enter runs it through the action registry, and the
  backend saw `engine_play`.
- `pianoroll.spec.ts` — clicking empty canvas space in the isolated
  `PianoRoll` fixture click-creates one note (same component + pointer
  flow as the real editor; the shell only shows a stub panel).
- `gameaudio.spec.ts` — clicking `combat` in the isolated `Audition`
  fixture logs `explore -> combat: Fade` and flips the audible readout to
  the combat stack.

Web-mode only: Playwright drives the Vite dev server on port 1420
(`playwright.config.ts` `webServer` block, Chromium project). No
tauri-driver, no WebView, no Rust backend — so these specs prove
frontend wiring, never DSP/IO correctness (that stays with
`cargo test` + `bun test`).

## 2. How the backend mock works

The UI talks to Rust only via `window.__TAURI_INTERNALS__.invoke`
(`@tauri-apps/api@2` `core.js`); events ride the `plugin:event|*`
commands. `ui/e2e/tauri-mock.ts` installs a replacement with
`page.addInitScript` BEFORE navigation:

- `installInBrowser(canned)` (serialized into the page — keep it
  self-contained, JSON-safe args only) serves canned responses:
  `project_get` et al. return a minimal valid `Project`, `engine_play`
  returns `"Playing"`, `engine_stop` returns `"Stopped"`.
- Event shim: `plugin:event|listen` registers callback ids,
  `plugin:event|emit` fans out to them, plus a `window.__e2eEmit`
  helper so specs can push backend events (`emitToPage`).
- Inspection: `window.__e2eIpcCalls` records every invoke in order
  (`ipcCalls(page)` reads it back) — assert backend effects through
  this, never through Rust state.

A legacy `window.__TAURI__` alias object is also created in case a
layer probes for its presence, but the real transport mock is
`__TAURI_INTERNALS__`.

## 3. How to add a spec

1. If the component already mounts in `App.tsx`, write the spec against
   `/` with `mockTauri(page)` in `beforeEach` and role/text locators
   (panels are `h2` headings; the palette is `role="dialog"` named
   `Command palette`; canvases use `aria-label`).
2. If the component does NOT mount in the shell (real PianoRoll,
   Audition, …), add an isolated fixture: `ui/e2e/fixtures/<name>.html`
   + `<name>-fixture.tsx` that renders the component with sample data
   and exposes `data-testid` counters, then `page.goto` the html path —
   the Vite dev server serves and transforms it (Solid JSX included).
3. Keep specs to user-visible behavior + `ipcCalls` assertions; keep
   file names `*.spec.ts` so `bun test` (unit) never picks them up.
4. Run `bun run test:e2e` from the repo root. First run needs
   `bunx playwright install chromium` (browsers are not vendored).

## 4. Files

- `ui/e2e/playwright.config.ts` — testDir/testMatch, baseURL
  `http://127.0.0.1:1420`, `webServer` pins vite to `--host 127.0.0.1
  --port 1420` (vite binds `::1` only on some machines, which breaks the
  IPv4 `url` poll), Chromium only.
- `ui/e2e/tauri-mock.ts` — `mockTauri`, `ipcCalls`, `emitToPage`,
  `installInBrowser`, `defaultCanned`.
- `ui/e2e/fixtures/` — `pianoroll.html/.tsx`, `audition.html/.tsx`.
- `ui/e2e/*.spec.ts` — the four smoke specs.
- `ui` `devDependencies` gains `@playwright/test`; `ui/tsconfig.json`
  includes `e2e` so `bun run check` typechecks the harness.
