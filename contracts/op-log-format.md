# v0 op-log format (frozen)

The project document is event-sourced: an append-only op log plus snapshots.
Machine truth: `Op` / `OpKind` in `core/src/model.rs`.

## Entry

`Op { seq: u64, actor: string, kind: OpKind, target: string, value_json: string }`

- `seq` — monotonic per project, assigned by `op_apply` (the return value).
- `actor` — who produced the op: `ui`, `mcp`, `ai:<sidecar>`, `script:<name>`,
  or (additive, post-freeze) `gameaudio:<tool>` — the game-audio MCP tools'
  records, today only `gameaudio:export`. The suffix must be non-empty.
  All AI output lands as ordinary ops from an `ai:*` actor (undoable by design).
- `kind` — `TrackAdded | ClipAdded | ClipMoved | ParamSet | TempoSet | UndoMarker | AutomationPointSet | GameAudioExported`. Two additive extensions past the v0 freeze; replay of older logs is unaffected by either:
  - `AutomationPointSet` — upsert one automation point.
  - `GameAudioExported` — log-only record of a green `gameaudio_export`
    (MCP, actor `gameaudio:export`). `target` is the package name; payload
    `{"packageName": <non-empty string>, "stemCount": <n >= 0>}`. It changes
    no project state: apply/replay validate the payload and do nothing else,
    undo marks it undone. Branch compare classifies it as a `marker` row.
- `target` — node id, track id, clip id, automation lane id, or param address (`node:param`).
- `value_json` — JSON payload, e.g. `"0.5"`, `{"startBeats": 8}`, `{"beat": 4, "value": 0.5}` (plus `"node"`/`"param"` when the op creates its lane), `{"packageName": "demo-v1", "stemCount": 2}`.

## Rules (Track A implements)

- Instant autosave: every applied op is durable without an explicit save.
- Infinite cross-session undo: undo history survives restart (`UndoMarker`
  keeps redoable branches addressable).
- Snapshots bound log replay; branches compare/merge at op granularity.
