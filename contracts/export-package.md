# v1 export package format (frozen)

Machine truth: `core/src/game_audio.rs` (`ExportPackage`, `ExportStem`,
`StemKind`). Generated TS + Zod v4: `ui/src/generated/project.ts`.
`schema_version` is always `1` in v1.

## Idea

The engine deliverable is a **directory**, not a project file: rendered WAV
stems the engine loops/one-shots, plus one JSON event bank describing what to
play when. The DAW stays the authoring tool; the game ships data. A versioned
validator approves every package before it leaves the DAW, so a broken export
(a missing loop, a dangling clip, a typo'd address) is a red build, not a
silent in-game bug.

## Layout (all paths package-relative)

```text
<package>/
  package.json        # ExportPackage serialized verbatim
  bank_<id>.json      # one SfxBank per bank_ids entry, serialized verbatim
  stems/<cue>_<layer>.wav
  sfx/<event>_<n>.wav
```

## Shapes

- `StemKind`: `MusicLayer | SfxClip` — what the stem was rendered from.
- `ExportStem { path, source_id, source_layer_id, kind, loop_start_beats,
  loop_end_beats }` — one rendered file. `source_id` names the cue
  (`MusicLayer`) or event (`SfxClip`); `source_layer_id` names the layer
  (empty for SFX). Loop points are in **beats** at the cue tempo (the engine
  converts to samples); `0, 0` means one-shot, not a loop. Renders are
  48 kHz / 24-bit WAV (the game-audio default; other formats are a later
  version).
- `ExportPackage { schema_version, name, cue_ids, bank_ids, stems,
  event_bank_path, validator_version }` — the manifest (`package.json`).
  `validator_version` records which validator approved it (`"1"` in v1).

## Validator rules (GA-4 implements; every rule is a failing test)

1. Every `cue_ids`/`bank_ids` entry resolves to a real cue/bank.
2. Every layer's `clip_ids` and every event's `clip_ids` resolve to real
   clips; no event has an empty pool; no bank has duplicate event ids.
3. Every `Stinger` rule names a real `stinger_cue_id`; every `RtpcBinding`
   names a real `target_node:target_param`.
4. Loop ranges satisfy `0 <= start < end` for loops, `0, 0` for one-shots;
   every listed stem file exists and parses as WAV.
5. Re-render determinism: same project + seed = byte-identical stems
   (seeded RNG for SFX humanization; validator re-renders one stem).

## Game-audio MCP tools (GA-5 implements against these shapes)

The frozen v0 `contracts/mcp-tools.md` list is untouched. GA-5 adds rows
**additively** (new names only, existing six frozen) plus a migration note:

| Tool | Reads/writes | Input |
|---|---|---|
| `gameaudio_list_cues` | cues in project | `{ projectId }` |
| `gameaudio_audition` | posts a `GameStateSnapshot`, returns audible layer/event ids | `{ state, values }` |
| `gameaudio_trigger_sfx` | triggers one `SfxEvent` by id (audition only) | `{ eventId }` |
| `gameaudio_export` | builds + validates an `ExportPackage` | `{ name, cueIds, bankIds }` |

No export ships with validator errors; `gameaudio_export` returns the
validator report either way (errors as data, not silence).
