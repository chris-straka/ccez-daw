// Pure note-edit operations behind the FL-style mouse gestures.
//
// Every function is immutable (returns a new clip) and headless, so the
// scripted `bun test` drives the exact same transitions the canvas commits.
// Persistence (engine `midi`-kind asset save) is the caller's job: these
// helpers never touch IPC, keeping the DSP/IO path out of the edit path.

import type { MidiClip, MidiNote } from "./model";
import { nextNoteId, sortNotes, validateNote } from "./model";
import { snapBeat } from "./geometry";

export const DEFAULT_VELOCITY = 100;
export const MIN_LEN_BEATS = 1 / 16;

/** Left-click on empty space: create one snapped note (FL click-create). */
export function createNote(
  clip: MidiClip,
  pitch: number,
  beat: number,
  opts?: { snap?: number; lenBeats?: number; velocity?: number },
): MidiClip {
  const snap = opts?.snap ?? 0.25;
  const lenBeats = opts?.lenBeats ?? snap;
  const note: MidiNote = {
    note_id: nextNoteId(clip),
    pitch: Math.min(127, Math.max(0, Math.round(pitch))),
    velocity: opts?.velocity ?? DEFAULT_VELOCITY,
    start_beats: snapBeat(beat, snap),
    len_beats: Math.max(MIN_LEN_BEATS, lenBeats),
    channel: 0,
    probability: 1,
    ratchets: 1,
    timing_offset_beats: 0,
    pitch_bend: 0,
    pressure: 0,
    timbre: 0,
    muted: false,
  };
  validateNote(note);
  return { ...clip, notes: sortNotes([...clip.notes, note]) };
}

/** Right-sweep delete: remove one note by id. Returns true when removed. */
export function deleteNote(clip: MidiClip, noteId: number): { clip: MidiClip; removed: boolean } {
  const before = clip.notes.length;
  const notes = clip.notes.filter((n) => n.note_id !== noteId);
  return { clip: { ...clip, notes }, removed: notes.length !== before };
}

/** Right-sweep across many ids in one gesture (dedupes ids). */
export function sweepDelete(clip: MidiClip, noteIds: number[]): MidiClip {
  const doomed = new Set(noteIds);
  if (doomed.size === 0) return clip;
  return { ...clip, notes: clip.notes.filter((n) => !doomed.has(n.note_id)) };
}

/** Drag a note body: move pitch + start (both snapped by the caller). */
export function moveNote(
  clip: MidiClip,
  noteId: number,
  pitch: number,
  startBeats: number,
): MidiClip {
  let found = false;
  const notes = clip.notes.map((n) => {
    if (n.note_id !== noteId) return n;
    found = true;
    const moved: MidiNote = {
      ...n,
      pitch: Math.min(127, Math.max(0, Math.round(pitch))),
      start_beats: startBeats,
    };
    validateNote(moved);
    return moved;
  });
  if (!found) throw new Error(`unknown note_id ${noteId}`);
  return { ...clip, notes: sortNotes(notes) };
}

/** Drag the right edge: resize length (FL drag-length), clamped positive. */
export function resizeNote(clip: MidiClip, noteId: number, lenBeats: number): MidiClip {
  let found = false;
  const notes = clip.notes.map((n) => {
    if (n.note_id !== noteId) return n;
    found = true;
    const resized: MidiNote = { ...n, len_beats: Math.max(MIN_LEN_BEATS, lenBeats) };
    validateNote(resized);
    return resized;
  });
  if (!found) throw new Error(`unknown note_id ${noteId}`);
  return { ...clip, notes: sortNotes(notes) };
}
