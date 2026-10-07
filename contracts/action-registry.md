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
| `app.update.check` | — (local → `app_update_check`) | ask the release feed for a newer signed build; status line shows the answer |
| `app.update.install` | — (local → `app_update_install`) | download, verify, install the newer build and restart |
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
| `view.focusSession` | — (local) | UI-only; Session View focus (`gs`, View menu) |
| `section.goto.chorus` | — (local) | UI-only; shortcut hardening (`gc` chord) |
| `section.goto.verse` | — (local) | UI-only; shortcut hardening (`gv` chord) |
| `mixer.muteSelected` | — (local) | UI-only; shortcut hardening (mixer `m`) |
| `mixer.soloSelected` | — (local) | UI-only; shortcut hardening (mixer `M`) |

## Additive post-v0 coverage rows

Everything added since the v0 freeze, same contract: one action id runs
from the palette, the vim layer, the scripting API, and MCP alike.
`automation.point_set` is IPC-backed (`op_apply` carries the
`AutomationPointSet` op); the rest are `— (local)` pure
computations / ordinary-op builders over the frozen v0 model, so no new
IPC surface was needed.

| Action id | IPC command | Notes |
|---|---|---|
| `automation.point_set` | `op_apply` | Upsert one automation point (`{ lane, beat, value, node?, param?, laneExists? }`); undoable |
| `session.launch` | — (local) | Quantized slot/scene launch plan (`S`, View menu) |
| `session.jam_record` | — (local) | Jam → `ClipAdded` ops (`J`, View menu) |
| `comp.commit` | — (local) | Composite take → `ClipAdded` (`C`, Clip menu) |
| `groove.apply` | — (local) | Grooved clip → `ClipAdded` (programmatic/palette, Clip menu) |
| `branch.merge` | — (local) | Merge preview/apply → ops (programmatic/palette, File menu) |
| `record.punch` | — (local) | Punched take → `ClipAdded` (`r`, Transport menu) |
| `link.join` | — (local) | Join Link session clock (`L`, Transport menu; writes no ops) |
