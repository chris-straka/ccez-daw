// Score panel model: which frozen clip the staff shows, and how its notes
// travel through the existing MidiClip asset read/write path.
//
// A v0 `Clip` with `kind: Midi` carries only an opaque `source` string; the
// note bytes live behind that key (`core/src/midi/clip.rs` asset API, mirrored
// by `encodeClip`/`decodeClip`). The panel never touches IPC itself: it reads
// bytes with `readScoreClip`, edits the same `MidiClip` the piano roll owns
// (same `note_id` sequence via `createNoteAtStep`, no schema change), reports
// the edited clip through `onEdit`, and persists the timeline side as one
// ordinary frozen `ClipAdded` op (`scoreCommitOp`, undoable via `op_undo`).

import { OpSchema, type Clip, type Op, type OpKind, type Project } from "../generated/project";
import { decodeClip, encodeClip, MidiClipSchema, type MidiClip } from "../pianoroll/model";

/** Frozen MIDI clips of a project, in document order. */
export function midiClips(project: Project | null): Clip[] {
  if (!project) return [];
  return project.clips.filter((c) => c.kind === "Midi");
}

/** The clip the score shows: the picked id, else the first MIDI clip. */
export function resolveScoreClip(project: Project | null, clipId?: string | null): Clip | null {
  const all = midiClips(project);
  if (all.length === 0) return null;
  if (clipId) return all.find((c) => c.id === clipId) ?? null;
  return all[0];
}

/**
 * Read path: decode the opaque `MidiClip` bytes stored behind a clip
 * `source` key. Returns `null` when nothing has been saved yet (the panel
 * shows an empty staff); throws on corrupt payload, like `decodeClip`.
 */
export function readScoreClip(
  source: string,
  assets: Record<string, string>,
): MidiClip | null {
  const raw = assets[source];
  if (raw == null) return null;
  return decodeClip(raw);
}

/** Write path: encode the working clip to the opaque bytes for its key. */
export function writeScoreClip(clip: MidiClip): string {
  MidiClipSchema.parse(clip);
  return encodeClip(clip);
}

/** Empty staff for a clip with no saved bytes yet (same length, no notes). */
export function emptyStaffFor(clip: Clip): MidiClip {
  return { length_beats: clip.length_beats, notes: [] };
}

/**
 * Commit score edits as one frozen `ClipAdded` op: `seq: 0` is a placeholder
 * the engine replaces; `target` names the clip and `value_json` carries the
 * full frozen clip. The encoded `MidiClip` bytes travel with the panel's
 * `onEdit`; the op itself stays ordinary — exact mirror of `compCommitOp`.
 */
export function scoreCommitOp(actor: string, clip: Clip): Op {
  return OpSchema.parse({
    seq: 0,
    actor,
    kind: "ClipAdded" as OpKind,
    target: clip.id,
    value_json: JSON.stringify(clip),
  });
}
