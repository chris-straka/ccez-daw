# v0 project schema (frozen)

Machine truth: `core/src/model.rs`. Generated TS + Zod v4:
`ui/src/generated/project.ts`. `schema_version` is always `0` in v0.

## Universal node model

Clips, tracks, devices, and modulators share one addressable node model.
Audio, MIDI, automation, and modulation are views over it.

- `NodeKind`: `Track | Clip | Device | Modulator | Bus`
- `Node { id, kind: NodeKind, name, params: Param[] }`
- `Param { id, label, value, min, max, default, unit }`
- `ParamAddress { node, param }` — e.g. `trk_music:volume`. Any lane, device,
  or modulator addresses any parameter through this shape (the
  anything-modulates-anything seam).

## Routing

- `EdgeKind`: `Audio | Midi | Modulation | Sidechain`
- `Edge { id, from_node, from_port, to_node, to_port, kind }`
- The mixer is a view over `Audio` edges; Track G owns the visual graph over
  the same list.

## Timeline

- `Track { id, name, volume, pan, muted, solo, clip_ids, device_ids }`
- `ClipKind`: `Audio | Midi`
- `Clip { id, track_id, name, start_beats, length_beats, kind, source }`
  (`source` is opaque: sample path, take id, or MIDI blob key.)
- `AutomationPoint { beat, value }`,
  `AutomationLane { id, target: ParamAddress, points }`

## Project

`Project { schema_version, id, name, tempo, time_sig_num, time_sig_den,
tracks, clips, devices: Node[], routing: Edge[], automation }`
