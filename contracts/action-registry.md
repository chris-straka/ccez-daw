# v0 action registry stub (frozen ids)

One action id runs from the command palette, the vim layer, the TS scripting
API, NL commands, and MCP tools alike. Stub mirror:
`ui/src/actions/registry.ts` (Track K owns the full registry).

| Action id | IPC command | Notes |
|---|---|---|
| `transport.play` | `engine_play` | |
| `transport.stop` | `engine_stop` | |
| `project.new` | `project_new` | |
| `project.get` | `project_get` | programmatic (palette-optional) |
| `project.open` | `project_open` | |
| `project.save` | `project_save` | |
| `project.undo` | `op_undo` | |
| `project.redo` | `op_redo` | |
| `op.apply` | `op_apply` | programmatic (all AI/script output flows here) |
| `track.add` | `track_add` | |
| `track.list` | — (reads `project_get`) | MCP listing view |
| `clip.add` | `clip_add` | |
| `param.set` | `param_set` | |
| `engine.set_tempo` | `engine_set_tempo` | programmatic in v0 |
| `palette.open` | — (local) | UI-only; Track K |
| `vim.mode.*` | — (local) | UI-only; Track K |

## Additive menu-gap rows (Agent 1, native menu bar)

Same contract as above: one action id runs everywhere (native menu via the
`menu-action` event, palette, vim layer, scripting, MCP). All `— (local)`;
mirrored in `ui/src/actions/registry.ts`. Unregistered local handlers resolve
to descriptors until Track K wires real behavior.

| Action id | IPC command | Notes |
|---|---|---|
| `app.about` | — (local) | About dialog; native About panel on macOS |
| `app.quit` | — (local) | Quit; native role on macOS, `app.exit(0)` elsewhere |
| `app.preferences` | — (local) | Preferences dialog (Track K) |
| `edit.cut` | — (local) | Native OS cut + sync event |
| `edit.copy` | — (local) | Native OS copy + sync event |
| `edit.paste` | — (local) | Native OS paste + sync event |
| `edit.select_all` | — (local) | Native OS select-all + sync event |
| `view.zoom.in` | — (local) | Window zoom in (Track K) |
| `view.zoom.out` | — (local) | Window zoom out (Track K) |
| `view.zoom.reset` | — (local) | Window zoom reset (Track K) |
| `view.fullscreen` | — (local) | Toggle fullscreen (Track K) |
| `help.open_docs` | — (local) | Open docs (Track K) |
| `help.show_shortcuts` | — (local) | Shortcut cheatsheet (Track K) |
| `track.delete` | — (local) | Delete selected track (Track K) |
| `clip.delete` | — (local) | Delete clip (Track K) |
| `clip.duplicate` | — (local) | Duplicate clip (Track K) |
| `vim.motion.*` | — (local) | UI-only; Track K |
| `view.focusArrangement` | — (local) | UI-only; shortcut hardening (`ga`) |
| `view.focusPianoRoll` | — (local) | UI-only; shortcut hardening (`gp`) |
| `view.focusMixer` | — (local) | UI-only; shortcut hardening (`gm`) |
| `section.goto.chorus` | — (local) | UI-only; shortcut hardening (`gc` chord) |
| `section.goto.verse` | — (local) | UI-only; shortcut hardening (`gv` chord) |
| `mixer.muteSelected` | — (local) | UI-only; shortcut hardening (mixer `m`) |
| `mixer.soloSelected` | — (local) | UI-only; shortcut hardening (mixer `M`) |
