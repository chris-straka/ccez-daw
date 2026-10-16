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
`cargo run`) for validation, rendering and export. A composition opens in
the DAW as a project (`compose_open_in_daw`, or the shell's `compose_open`
IPC / "Composition: Open in the DAW"). Every mutation returns
`{ ok, errors }` from the Rust validator.

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
| `compose_write_notes` | `{ section, layer, instrument, notes: [{ pitch? \| pitches?, start, length, velocity? }], mode? }` | `{ notes, written, ok, errors }` |
| `compose_get` | `{}` | summary + document + validation |
| `compose_preview` | `{ out: .wav\|.mp3, plan?: [{ section, bars?, intensity? }] }` | `{ out, plan, stats }` (stereo; peak/rms, per-step layers, `instrument_rms_dbfs`, `loop_seams`, warnings) |
| `compose_export_audioforge` | `{ dir, plan? }` | `{ dir, files, stems, trim_db, loop_seams, warnings }` (`contracts/audioforge-music.md`) |
| `compose_save` / `compose_open` | `{ path }` | saved path / summary + validation |
| `compose_listen` | `{ audio?, reference?, spectrogram?, plan? }` | the current composition (or `audio`) heard: `summary` lines; `preview.features` (tempo, key, onsets/s, brightness, LUFS, loudness curve and range, width); `composition.sections` (each at low and full intensity), `layers` (`lufs`, `adds_lu`, notes/bar, register), `similarity` between sections, `clashes`, `out_of_key`, `out_of_range`; `reference.comparison` (harmony, timbre, rhythm, brightness, loudness, `differences`) |
| `compose_compare` | `{ a, b }` | both files' features and the comparison of B against A |
| `compose_motif` | `{ name, motif? \| text? \| notes? }` | `{ motif, notes, beats, text }` (kept in the document's `motifs`) |
| `compose_vary` | `{ motif, ops: [{ op: transpose\|chromatic\|invert\|retrograde\|augment\|sequence\|fragment\|displace\|octave\|dynamics, ... }], key?, saveAs?, write?: { section, layer, instrument, start?, mode? } }` | `{ key, notes, text, saved?, part? }` |
| `compose_progression` | `{ key?, chords? \| preset?, every?, voicing?, pattern?, bassPattern?, subdivision?, velocity?, write? }` | `{ key, chords: [{ symbol, numeral, bass, voices }], parallel_fifths_octaves, upperText, bassText, written? }`; no chords or preset = the preset list |
| `compose_reharmonise` | `{ motif? \| text? \| notes?, key?, every?, palette: plain\|colour, voicing?, pattern?, write? }` | `{ key, chords, voiced, parallel_fifths_octaves, written? }` |
| `compose_arrange` | `{ section, melody?, fill?, chords?, every?, palette?, key?, ensemble?: [{ instrument, role, layer, octave?, velocity?, subdivision?, density?, keys? }], musicMd?, layerIntensity? }` | `{ key, chords, parts, added, notes, ok, errors }`; roles: melody, double, sparkle, counter, bass, pad, arp, pulse, broken, drone, percussion |
| `compose_set_layer_fx` | `{ layer, eq?: { lowCutHz?, highCutHz?, lowDb?, midDb?, midHz?, highDb? }, reverb?, room? }` | `{ layer, ok, errors }` |
| `compose_set_master` | `{ lufs?, ceilingDbtp?, off? }` | `{ master, ok, errors }` (previews: full chain; stems: one static gain) |
| `compose_master_audio` | `{ in, out, lufs?, ceilingDbtp?, loop? }` | `{ out, master: { lufs_before, lufs_after, true_peak_before_dbtp, true_peak_after_dbtp, gain_db, max_reduction_db, seamless? } }` |
| `compose_transcribe` | `{ audio, out?, midi?, tempo?, key?, bars?, startBar?, from?, to?, name? }` | `{ transcription: { tempo, key, chords, melody_notes, bass_notes, downbeat_secs }, heard, composition }` (becomes the current composition) |
| `compose_open_in_daw` | `{ dir }` | `{ dir, tracks, clips, audio_tracks, midi_tracks, beats, warnings }` (a project bundle the DAW opens) |

