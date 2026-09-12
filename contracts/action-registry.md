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
