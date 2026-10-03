# v0 IPC command/event table (frozen)

Nothing crosses the UI/Rust boundary except these commands and events.
Machine truth: `core/src/ipc.rs`. Generated TS: `ui/src/generated/ipc.ts`.

## Commands (`invoke`)

| Command | Args | Returns | Notes |
|---|---|---|---|
| `project_new` | `{ name: string }` | `Project` | create empty project |
| `project_get` | `{}` | `Project` | currently open project |
| `project_save` | `{ path: string }` | `void` | instant-autosave path (Track A engine) |
| `project_open` | `{ path: string }` | `Project` | open from disk |
| `op_apply` | `{ op: Op }` | `number` | append op, return seq |
| `op_undo` | `{}` | `number` | undone seq |
| `op_redo` | `{}` | `number` | redone seq |
| `track_add` | `{ name: string }` | `string` | new track id |
| `clip_add` | `{ clip: Clip }` | `string` | new clip id |
| `param_set` | `{ target: ParamAddress; value: number }` | `void` | universal param addressing |
| `engine_play` | `{}` | `EngineState` | start transport |
| `engine_stop` | `{}` | `EngineState` | stop transport |
| `engine_set_tempo` | `{ tempo: number }` | `void` | BPM |

## Additive extensions (post-freeze)

Like the `AutomationPointSet` op before them, these commands were added
after the v0 freeze without changing any frozen row. The shell implements
them for real (the pre-wiring stubs are gone):

| Command | Args | Returns | Notes |
|---|---|---|---|
| `asset_store` | `{ key: string; kind: string; bytes: number[] }` | `void` | persist opaque blob (MIDI note bytes, audio, preset) |
| `asset_load` | `{ key: string }` | `number[]` | load blob by key |
| `asset_list` | `{}` | `AssetEntry[]` | list stored assets without reading bytes |
| `engine_position` | `{}` | `number` | transport position in beats |
| `engine_record` | `{}` | `EngineState` | start a recording pass (transport runs, state reads Recording) |
| `link_toggle` | `{}` | `boolean` | join/leave the Link session clock; returns the new membership |

## Events (`listen`)

| Event | Payload | When |
|---|---|---|
| `project_changed` | `Project` | after applied/undone/redone op |
| `engine_state_changed` | `EngineState` | transport state change |
| `param_changed` | `ParamAddress` | after `param_set` (`node:param`) |
| `op_applied` | `Op` | every op-log append |
