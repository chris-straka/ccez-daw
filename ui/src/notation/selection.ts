// Notation selection + edits, bidirectional with the piano-roll model.
//
// The piano-roll store (`../pianoroll/store`) owns every mutation; this file
// only maps staff gestures to it. Selection is a plain set of `note_id`s, so
// a selection made by clicking SVG noteheads works unchanged on the canvas
// piano roll and vice versa — both views read one `MidiClip`.

import type { MidiClip } from "../pianoroll/model";
import { createNote, moveNote, resizeNote, sweepDelete } from "../pianoroll/store";
import { naturalPitchAtStep, staffStep, type Clef } from "./pitch";

export type NoteSelection = ReadonlySet<number>;

export function emptySelection(): NoteSelection {
  return new Set<number>();
}

export function selectOnly(ids: Iterable<number>): NoteSelection {
  return new Set(ids);
}

export function toggleSelect(sel: NoteSelection, id: number): NoteSelection {
  const next = new Set(sel);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** Keep only ids still present in the clip (call after any edit). */
export function pruneSelection(clip: MidiClip, sel: NoteSelection): NoteSelection {
  const live = new Set(clip.notes.map((n) => n.note_id));
  return new Set([...sel].filter((id) => live.has(id)));
}

/** Move every selected note by semitones (staff vertical drag / arrows). */
export function transposeSelection(clip: MidiClip, sel: NoteSelection, semitones: number): MidiClip {
  let next = clip;
  for (const id of [...sel].sort((a, b) => a - b)) {
    const n = next.notes.find((m) => m.note_id === id);
    if (n) next = moveNote(next, id, n.pitch + semitones, n.start_beats);
  }
  return next;
}

/** Shift every selected note in time by beats (staff horizontal drag). */
export function shiftSelectionBeats(clip: MidiClip, sel: NoteSelection, deltaBeats: number): MidiClip {
  let next = clip;
  for (const id of [...sel].sort((a, b) => a - b)) {
    const n = next.notes.find((m) => m.note_id === id);
    if (n) next = moveNote(next, id, n.pitch, n.start_beats + deltaBeats);
  }
  return next;
}

/** Set one selected note's length (duration palette / handle). */
export function setSelectedDuration(clip: MidiClip, noteId: number, lenBeats: number): MidiClip {
  return resizeNote(clip, noteId, lenBeats);
}

/** Delete every selected note (Delete key / eraser). */
export function deleteSelection(clip: MidiClip, sel: NoteSelection): MidiClip {
  return sweepDelete(clip, [...sel]);
}

export interface StepNoteOpts {
  snap?: number;
  lenBeats?: number;
  velocity?: number;
  /** Chromatic offset from the natural step pitch (default 0; +1 = sharp). */
  accidental?: number;
}

/**
 * Create a note from a staff click: step -> natural pitch (+ accidental),
 * then the same `createNote` the piano roll uses (same ids, same snap).
 */
export function createNoteAtStep(
  clip: MidiClip,
  step: number,
  clef: Clef,
  beat: number,
  opts?: StepNoteOpts,
): MidiClip {
  const pitch = naturalPitchAtStep(step, clef) + (opts?.accidental ?? 0);
  return createNote(clip, pitch, beat, opts);
}

/** Staff step of a clip note (for re-render after piano-roll edits). */
export function stepOfNote(clip: MidiClip, noteId: number): { clef: Clef; step: number } | null {
  const n = clip.notes.find((m) => m.note_id === noteId);
  return n ? staffStep(n.pitch) : null;
}
