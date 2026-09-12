# n-menu — native menu bar (Agent 1)

## What was built

`src-tauri/src/menu.rs` (new) + one-line wiring in `src-tauri/src/lib.rs`
(`.setup(|app| { menu::install(app)?; Ok(()) })`).

## Design: menu item id == action id

Every custom menu item uses its `contracts/action-registry.md` action id as
the Tauri menu item id. Selecting one emits a global `menu-action` event with
payload `{ "action": "<id>" }`. The Solid shell (Track K) listens once:

```ts
import { listen } from "@tauri-apps/api/event";
import { findAction } from "./actions/registry";

await listen<{ action: string }>("menu-action", (e) => {
  void findAction(e.payload.action)?.run({});
});
```

Because the palette runs the same registry by id, menu and palette stay in
sync by construction — no parallel dispatch table.

## Menus

App (macOS only) / File / Edit / View / Transport / Track / Clip / Help.
Non-macOS gets Quit (File), Preferences (Edit), About (Help) fallbacks since
there is no App menu there.

## Native roles vs custom items

- Custom (deterministic id, always emits): everything DAW-specific, plus
  Undo/Redo (so they hit the op log, not text-field undo) and About
  (`app.about`, so the frontend owns the dialog on all platforms).
- Native `PredefinedMenuItem` (OS handles; best-effort sync emission when the
  OS delivers the event): Services, Hide, Hide Others, Show All, Quit
  (macOS), Cut/Copy/Paste/Select All (real clipboard behavior).
- Services/Hide roles are swallowed by macOS with no event — documented in
  code, nothing to forward.

## Accelerators (all `CmdOrCtrl`-based, verified against muda 0.19.3 source)

Shown: `CmdOrCtrl+N/O/S/Q/Z`, `Shift+CmdOrCtrl+Z`, `X/C/V/A`, `,/K/T/L/D`,
`=/-/0`, `F1`, `F11`, `CmdOrCtrl+Backspace`. Deliberately NO bare-key
accelerators (Space/letters): native shortcuts are mode-unaware and would
fire while typing or in vim insert mode. Transport stays on the vim bindings
(`Space` / `s`) and the palette.

## Registry gaps added (frozen file extended additively)

16 `— (local)` rows appended to `contracts/action-registry.md` and mirrored
in `ui/src/actions/registry.ts`: `app.about/quit/preferences`,
`edit.cut/copy/paste/select_all`, `view.zoom.in/out/reset`, `view.fullscreen`,
`help.open_docs/show_shortcuts`, `track.delete`, `clip.delete/duplicate`.
Nothing in `ui/src/generated/*` touched. Unregistered local handlers resolve
to descriptors, so these are palette-safe until Track K wires behavior.

## Validation

- `cargo test -p ccez-daw`: 11 menu tests pass — item→action mapping against
  a registry-id mirror, submenu titles (both platform variants), id
  uniqueness, accelerator uniqueness + presence on key items, native-role
  coverage, payload shape, route precedence.
- `bun run test` (ui): 118 pass. `bun run check` (tsc): clean.
- `cargo clippy/build -p ccez-daw`: no warnings from `menu.rs`.
- NOT runtime-verified: actual menu rendering / click → event round-trip
  needs `tauri dev` (no display harness here); accelerator strings were
  verified against the muda parser source, not by execution.

## For Track K

Wire `menu-action` → registry run (snippet above) and real handlers for the
16 additive local actions. Suggested: `edit.*` → focused-field
`document.execCommand`/clipboard fallback; `view.*` → window zoom/fullscreen;
`app.*`/`help.*` → dialogs + docs URL.
