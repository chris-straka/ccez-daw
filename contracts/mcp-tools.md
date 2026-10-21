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

## Additive game-audio tools (MCP-only)

`gameaudio_list_cues`, `gameaudio_audition`, `gameaudio_trigger_sfx` and
`gameaudio_export` (inputs and semantics: `contracts/export-package.md`,
"Game-audio MCP tools") sit between the v0 six and the coverage rows. Like
the `compose_*` tools below they have no action-registry entry: their
`gameaudio.*` action ids are MCP-side names only, with no palette row,
keybinding, or menu item.

## Registry actions without an MCP tool

Not every registry action is reachable over MCP. `project.new`,
`project.open`, `project.save`, `project.undo`, `project.redo`,
`track.add`, `track.delete`, `engine.set_tempo`, `op.apply`,
`clip.delete` and `clip.duplicate` (and UI-only local rows such as
`app.*`, `edit.*`, `view.*`, `vim.*`) run from the palette, vim layer, scripting API or
menu only. MCP output stays undoable through the op log (`UndoMarker`),
but there is no undo tool.

## Additive post-v0 coverage tools

Each mirrors one action-registry entry above; every mutation lands in the
op log with actor `mcp` (undoable by design). `session_launch` and
`link_join` are evaluation-only (a launch plan / a session clock read —
never writes, like `gameaudio_audition`).

Semantics match core: `automation_set_point` upserts (an exact-beat point
is replaced, points stay ascending, `apply_automation_point`);
`session_jam_record` and `branch_merge` validate the whole batch (shape
and parseable `value_json`) before appending, so a bad entry writes
nothing; `groove_apply` names the copy `<clipId>~groove-<name>` (the UI
`grooveCommitOp` id), sources it `groove:<name>+<key>` where `<key>` is
the clip's source or `<clipId>.mid` (the UI's `midiAssetKey`), and rejects
a repeat while that copy is live (core `DuplicateTarget`); `link_join` validates `tempo` before it creates the
session. Undo picks the newest op not already undone and reverses
`ClipAdded` and `AutomationPointSet` alike (`Engine::undo`).

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
project. Every mutation returns `{ ok, errors, pending }` from the Rust
validator. Mutations are transactional: pitches are checked against core's
`parse_pitch` rules (names like `F#3`, integers 0..=127) before the
document changes, and a mutation (or `compose_open`) whose validation
throws restores the previous document, so one bad call never leaves the
session holding a document the CLI cannot load.

| Tool | Input | Returns |
|---|---|---|
| `compose_new` | `{ name, tempo, beatsPerBar?, key? }` | summary |
| `compose_patches` | `{ family? }` | `[{ id, kind: synth\|sampled, family, description, range, pan, licence, cached, source?, download_mb? }]` |
| `compose_instrument` | `{ id }` | `{ id, keys_with_recordings, velocity_layers, round_robins, licence, hits? }` (downloads if needed) |
| `compose_fetch_instruments` | `{ ids: [id...] \| ["all"] }` | `{ cache, fetched: [{ id, downloaded_bytes }] }` |
| `compose_set_room` | `{ decaySecs?, predelayMs?, damping? }` | `{ room, ok, errors }` |
| `compose_add_instrument` | `{ id, patch, gainDb?, reverb?, pan? }` | `{ added\|updated, ok, errors }` |
| `compose_add_layer` | `{ id, minIntensity?, gainDb? }` | `{ added\|updated, ok, errors }` |
| `compose_add_section` | `{ id, role: intro\|loop\|outro, bars, next? }` | `{ added\|updated, beats, ok, errors }` |
| `compose_write_notes` | `{ section, layer, instrument, notes?: [{ pitch? \| pitches?, start, length, velocity? }], text?, start?, repeat?, mode?: replace\|append }` (give `notes` or `text`; `text` is `PITCH[/LEN][@VEL]` tokens from beat `start`, chords `D3+F3/2`, rests `r/2`, written `repeat` times) | `{ notes, written, ok, errors }` |
| `compose_get` | `{}` | summary + document + validation |
| `compose_preview` | `{ out: .wav\|.mp3, plan?: [{ section, bars?, intensity? }] }` | `{ out, plan, stats }` (stereo; peak/rms, per-step layers, `instrument_rms_dbfs`, `loop_seams`, warnings) |
| `compose_export_audioforge` | `{ dir, plan? }` | `{ dir, files, stems, trim_db, loop_seams, warnings }` (`contracts/audioforge-music.md`) |
| `compose_save` / `compose_open` | `{ path }` | saved path / summary + validation |

