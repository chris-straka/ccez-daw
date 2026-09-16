# v0 op-log format (frozen)

The project document is event-sourced: an append-only op log plus snapshots.
Machine truth: `Op` / `OpKind` in `core/src/model.rs`.

## Entry

`Op { seq: u64, actor: string, kind: OpKind, target: string, value_json: string }`

- `seq` — monotonic per project, assigned by `op_apply` (the return value).
- `actor` — who produced the op: `ui`, `mcp`, `ai:<sidecar>`, `script:<name>`.
  All AI output lands as ordinary ops from an `ai:*` actor (undoable by design).
- `kind` — `TrackAdded | ClipAdded | ClipMoved | ParamSet | TempoSet | UndoMarker | AutomationPointSet` (`AutomationPointSet` is the one additive extension past the v0 freeze: upsert one automation point; replay of older logs is unaffected).
- `target` — node id, track id, clip id, automation lane id, or param address (`node:param`).
- `value_json` — JSON payload, e.g. `"0.5"`, `{"startBeats": 8}`, `{"beat": 4, "value": 0.5}` (plus `"node"`/`"param"` when the op creates its lane).

## Rules (Track A implements)

- Instant autosave: every applied op is durable without an explicit save.
- Infinite cross-session undo: undo history survives restart (`UndoMarker`
  keeps redoable branches addressable).
- Snapshots bound log replay; branches compare/merge at op granularity.
