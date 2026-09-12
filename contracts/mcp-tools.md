# v0 MCP tool list stub (frozen)

Stub mirror: `mcp/src/tools.ts` (Bun/TS on the official MCP TS SDK, stdio;
Track L wires handlers to the op log). Each tool mirrors one registry action.

| Tool | Action | Input |
|---|---|---|
| `project_get` | `project.get` | `{ projectId }` |
| `project_list_tracks` | `track.list` | `{ projectId }` |
| `project_add_clip` | `clip.add` | `{ trackId, name, startBeats, lengthBeats, kind }` |
| `param_set` | `param.set` | `{ node, param, value }` |
| `transport_play` | `transport.play` | `{ projectId }` |
| `transport_stop` | `transport.stop` | `{ projectId }` |

Validation (Track L): an MCP client lists tracks and adds a clip, then undoes
it — the op appears in the log with actor `mcp`.
