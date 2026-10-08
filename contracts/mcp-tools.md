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

## Additive post-v0 coverage tools

Each mirrors one action-registry entry above; every mutation lands in the
op log with actor `mcp` (undoable by design). `session_launch` and
`link_join` are evaluation-only (a launch plan / a session clock read —
never writes, like `gameaudio_audition`).

| Tool | Action | Input |
|---|---|---|
| `automation_set_point` | `automation.point_set` | `{ lane, beat, value, node?, param? }` |
| `session_launch` | `session.launch` | `{ pressBeat, gridBeats }` |
| `session_jam_record` | `session.jam_record` | `{ clips: ClipInput[] }` |
| `comp_commit` | `comp.commit` | `{ trackId, name, startBeats, lengthBeats, kind, takes }` |
| `groove_apply` | `groove.apply` | `{ clipId, groove, amount }` |
| `branch_merge` | `branch.merge` | `{ source, target, sourceOps }` |
| `record_punch` | `record.punch` | `{ trackId, name, kind, startBeats, endBeats }` |
| `link_join` | `link.join` | `{ session, peer, tempo? }` |

## Additive game-music composer tools (MCP-only)

Headless composer for game music: the server holds one composition
document (`core/src/compose`, JSON) and runs core's `ccez-compose` CLI
(`$CCEZ_COMPOSE_BIN`, else `target/{release,debug}/ccez-compose`, else
`cargo run`) for validation, rendering and export. There is no UI panel
or registry action for these yet; the composition is not a timeline
project. Every mutation returns `{ ok, errors }` from the Rust validator.

| Tool | Input | Returns |
|---|---|---|
| `compose_new` | `{ name, tempo, beatsPerBar?, key? }` | summary |
| `compose_patches` | `{}` | `[{ id, description, range }]` |
| `compose_add_instrument` | `{ id, patch, gainDb?, reverb? }` | `{ added\|updated, ok, errors }` |
| `compose_add_layer` | `{ id, minIntensity?, gainDb? }` | `{ added\|updated, ok, errors }` |
| `compose_add_section` | `{ id, role: intro\|loop\|outro, bars, next? }` | `{ added\|updated, beats, ok, errors }` |
| `compose_write_notes` | `{ section, layer, instrument, notes: [{ pitch? \| pitches?, start, length, velocity? }], mode? }` | `{ notes, written, ok, errors }` |
| `compose_get` | `{}` | summary + document + validation |
| `compose_preview` | `{ out: .wav\|.mp3, plan?: [{ section, bars?, intensity? }] }` | `{ out, plan, stats }` (peak/rms, per-step layers, warnings) |
| `compose_export_audioforge` | `{ dir, plan? }` | `{ dir, files, stems, trim_db }` (`contracts/audioforge-music.md`) |
| `compose_save` / `compose_open` | `{ path }` | saved path / summary + validation |

