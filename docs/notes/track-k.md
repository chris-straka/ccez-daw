# Track K primer: vim layer + command palette

New to modal editing? Start here. This note teaches the two big ideas behind
Track K — **everything is an action** and **keys are just a remappable view
over actions** — then lists exactly what Track K added so later tracks can
extend it without forking it.

## 1. The two ideas

**Everything is an action.**
One action id (`transport.play`, `project.undo`, `vim.motion.left`, …) runs
from the command palette, a vim keybinding, the TS scripting API, an NL
command, and an MCP tool alike. The single table both UIs read is
`ui/src/actions/registry.ts`, and it mirrors the frozen id list in
`contracts/action-registry.md`. If you need a new user-triggerable behavior,
add an id there — additively, never by renaming — with `kind: "ipc"` (runs a
Tauri command from `ui/src/generated/ipc.ts`) or `kind: "local"` (runs a
UI-local handler registered via `registerLocalActionHandler`; the frozen
contract marks these `— (local)`). Local ids never touch `core/`, so they
need no typegen run and no migration note.

**Keys are a remappable view over actions.**
`ui/src/input/keybindings.ts` maps key sequences (`"j"`, `"gg"`,
`"ctrl+k"`) in a vim mode (`normal`/`insert`/`visual`) and UI context
(`global`/`arrangement`/`piano-roll`) to action ids. There is no hardcoded
key handling anywhere else: `createKeymap(overrides)` replaces the sequence
for any action id, and `matchKeys` resolves multi-key sequences with
prefix-aware `match` / `pending` / `none` states (that is what makes `gg`
work — the first `g` waits instead of firing).

## 2. The pieces

- `ui/src/input/keybindings.ts` — `Binding` table, `DEFAULT_BINDINGS`,
  `createKeymap(overrides, extra)`, `matchKeys`/`resolveAction`,
  `eventToToken` (normalizes `KeyboardEvent` → token).
- `ui/src/input/motions.ts` — pure 2D cursor (`{track, step}`) shared by the
  arrangement and piano roll: `moveCursor` (`h j k l` + arrows, clamped),
  `w`/`b` clip-jump via `nextClipStart`/`prevClipStart`, `0`/`$`, `gg`/`G`.
- `ui/src/input/vim.ts` — DOM-free state machine: `createVimStore`,
  `handleKey`, `replayKeys`, `stubEffects`, plus `attachVimLayer` (the one
  DOM seam; the App shell wires it). Mode actions flip the mode, `vim.motion.*`
  moves the cursor, `palette.open` opens the palette, everything else goes to
  `effects.runAction`.
- `ui/src/palette/model.ts` — pure palette logic: `filterActions`
  (subsequence fuzzy match over title + id) and `runPaletteAction` (runs ANY
  registry action by id). `Palette.tsx` is a thin Solid view (fuzzy input,
  ↑/↓/Enter/Esc) over these.
- `ui/src/App.tsx` — wires it together: registers the real local handlers
  (mode label, palette open), attaches one global `keydown` listener that
  yields to the palette and to text inputs, and shows `-- NORMAL --` plus the
  `track:step` cursor in the header.

## 3. Default bindings (all remappable)

Global: `Ctrl+K` palette, `Esc` normal mode, `i`/`v` insert/visual,
`Space` play, `s` stop, `u` undo, `Ctrl+R` redo, `Ctrl+S` save.
Motions (normal + visual, arrangement + piano roll): `h j k l` + arrows,
`w`/`b` next/previous clip, `0`/`$` line start/end, `gg`/`G` first/last.

## 4. How to verify any of this

- `cd ui && bun test` — headless key-sequence replay
  (`tests/vim-palette.test.ts`: motions, modes, remaps, palette runs every
  registry action with a stubbed runner) plus the contract cross-checks.
- `cd ui && bun run check` — `tsc --noEmit` over the new modules.
- `bun run check` (repo root) — typegen drift gate + core tests: proves Track K
  added no drift (`generated/` untouched, `core/` untouched).
- By hand: `bun run tauri:dev`, press `Ctrl+K`, type `pla`, run it; press `j`
 /`k` and watch the header cursor; `i` then `Esc` flips the mode label.
