# Primer: shortcut hardening (`gc`, contexts, editor)

Track K owns `ui/src/input/` (`keybindings.ts`, `motions.ts`, `vim.ts`),
`ui/src/palette/`, and the headless replay tests. This note covers the
shortcut-hardening extension: multi-key chords, per-view contexts with
when-clauses, conflict detection, the shortcuts editor, and persistence.

## 1. Chords: `g` is a prefix, not a key

`matchKeys` was already prefix-aware (`gg` works because the first `g`
returns `pending`). The new defaults just use that machinery more:

- `gc` → `section.goto.chorus`, `gv` → `section.goto.verse`
- `ga` / `gp` / `gm` → `view.focusArrangement` / `view.focusPianoRoll` /
  `view.focusMixer`
- `gg` / `G` still work: one `g` waits, the second key decides.

Replay proof (`ui/tests/shortcuts.test.ts`, "multi-key chords"): feed `g`,
expect `{ kind: "pending" }`, feed the second key, expect `dispatched`. An
abandoned prefix resets on the next non-matching key (`ignored`), and there
is deliberately no chord timeout — pending waits until the next keypress.
`Esc` clears it the same way (no match → buffer reset).

## 2. Contexts and when-clauses

`KeyContext` is now `"global" | "arrangement" | "piano-roll" | "mixer"`.
`context: "all"` bindings (all motions, all chords) keep working everywhere;
the mixer owns two scoped keys that are inert elsewhere: `m` →
`mixer.muteSelected`, `M` → `mixer.soloSelected` (single chars keep their
case in `eventToToken`, so `m`/`M` are distinct).

`Binding.when` adds a per-binding predicate over `WhenFlags`
(`VimStore.flags`, kept current by the shell): `"hasSelection"`,
`"!playing"`, `"hasSelection && !playing"`, `"(a || b) && !c"`. Grammar is
`||` > `&&` > `!` with parens and `true`/`false` literals; missing flags read
as false and unparseable clauses fail closed. `handleKey` threads
`store.flags` through, so `replayKeys` with `store.flags = {...}` proves
gated behavior headlessly. Use `setVimContext(store, ctx)` to switch views —
it clears pending chord state, so a half-typed `g` never leaks across views.

## 3. Conflicts and the editor

- `findConflicts(table)` — every key sequence claimed by 2+ different
  actions in overlapping mode/context. Same-action rows (`h` in normal +
  visual) and disjoint contexts (mixer `m` vs anything elsewhere) are not
  conflicts. The default table asserts conflict-free in tests.
- `previewRemapConflicts(table, action, newKeys)` — dry-run a remap without
  mutating anything; the editor shows the result as a warning, never a
  silent steal.
- `ui/src/input/editor-model.ts` (pure) — `searchBindings` (substring over
  keys/action/mode/context), `remapAction` (uniform re-key per action, same
  semantics as `createKeymap` overrides; rejects unknown actions and empty
  keys), `resetBindings`, `formatBinding`, plus the persistence pair below.
- `ui/src/input/Editor.tsx` (thin Solid view, like `Palette.tsx`) — search
  field, row list with conflict badges, per-action remap inputs, reset button.
  Mount with `open`/`onClose`; pass `onKeymapChange` to hot-swap the live
  keymap. File naming note: the model is `editor-model.ts` (not `editor.ts`)
  because `./editor` resolves ambiguously against `Editor.tsx` in some
  bundlers, and the two names cannot coexist on case-insensitive filesystems.

## 4. Persistence

Customs persist as an `{overrides, extra}` diff under
`ccez-daw.keybindings.v1` (`localStorage` in the app, shared in-memory store
headlessly; inject `memoryBindingStore()` in tests):

- `savePersistedBindings` / `loadPersistedBindings` (corrupt JSON, wrong
  shapes, and invalid rows sanitize to empty customs — never throw),
- `loadCustomKeymap(store)` (= `createKeymap(overrides, extra)`: what the
  shell should build at startup),
- `diffBindingsAgainstDefaults` (uniformly re-keyed actions → overrides, the
  rest → extra) so future defaults still shine through; `resetEditor` clears
  storage and returns fresh defaults.

## 5. Registry additions (additive only)

New UI-local ids in `ui/src/actions/registry.ts` + rows in
`contracts/action-registry.md`: `view.focusArrangement`,
`view.focusPianoRoll`, `view.focusMixer`, `section.goto.chorus`,
`section.goto.verse`, `mixer.muteSelected`, `mixer.soloSelected`. All
`kind: "local"`, so no typegen run and no `generated/` edits. The frozen v0
ids are untouched; `contracts.test.ts` still passes.

## 6. Verify

- `cd ui && bun test tests/` — 118 pass: existing `vim-palette` replay,
  contract cross-checks, plus `tests/shortcuts.test.ts` (23: chords,
  mixer/arrangement/piano-roll contexts, when-clauses, conflicts, editor
  model, persistence round-trips).
- `cd ui && bun run check` — `tsc --noEmit` clean.
- `bun test` at `ui/` root also picks up `ui/e2e/*.spec.ts` (a sibling
  agent's Playwright specs); those 4 fail under bun and are unrelated to
  this track — run `tests/` for the unit gate.
- Context7 MCP (Tauri 2 menu + Playwright docs) was requested but is not in
  this agent's tool list, so no Context7 lookups were made; nothing here
  needed them (no Tauri menu or Playwright code was touched).
