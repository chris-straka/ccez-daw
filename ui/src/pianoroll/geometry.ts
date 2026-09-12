// Pure piano-roll geometry: beats <-> x, pitch <-> y, snap, hit-testing.
//
// Framework-free on purpose: the canvas component and the scripted test
// share these functions with no DOM, no Solid, no rAF involved.

import type { MidiNote } from "./model";

export interface ViewConfig {
  /** Canvas CSS size in pixels. */
  width: number;
  height: number;
  /** Beats visible across `width` (zoom). */
  beatsVisible: number;
  /** First visible beat (scroll). */
  scrollBeats: number;
  /** Highest visible pitch (default 127); always shows 128 rows down. */
  topPitch?: number;
}

export const MIN_PITCH = 0;
export const MAX_PITCH = 127;
/** Right-edge grab zone width in px for resize vs move disambiguation. */
export const RESIZE_HANDLE_PX = 6;

export function rowH(view: ViewConfig): number {
  return view.height / 128;
}

export function pxPerBeat(view: ViewConfig): number {
  return view.width / view.beatsVisible;
}

export function beatToX(beat: number, view: ViewConfig): number {
  return (beat - view.scrollBeats) * pxPerBeat(view);
}

export function xToBeat(x: number, view: ViewConfig): number {
  return view.scrollBeats + x / pxPerBeat(view);
}

/** Pitch rows run top (127) to bottom (0): row `p` occupies `[y, y+rowH)`. */
export function pitchToY(pitch: number, view: ViewConfig): number {
  return (MAX_PITCH - pitch) * rowH(view);
}

export function yToPitch(y: number, view: ViewConfig): number {
  const p = MAX_PITCH - Math.floor(y / rowH(view));
  return Math.min(MAX_PITCH, Math.max(MIN_PITCH, p));
}

export function snapBeat(beat: number, snap: number): number {
  if (!(snap > 0) || !Number.isFinite(beat)) return beat;
  return Math.round(beat / snap) * snap;
}

export interface NoteRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function noteRect(note: MidiNote, view: ViewConfig): NoteRect {
  return {
    x: beatToX(note.start_beats, view),
    y: pitchToY(note.pitch, view),
    w: Math.max(2, note.len_beats * pxPerBeat(view)),
    h: Math.max(2, rowH(view)),
  };
}

function insideRect(x: number, y: number, r: NoteRect): boolean {
  return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
}

export type Hit =
  | { kind: "resize"; note: MidiNote }
  | { kind: "body"; note: MidiNote }
  | { kind: "empty" };

/**
 * FL-style disambiguation at one pointer position: the right `RESIZE_HANDLE_PX`
 * of a note resizes it, anywhere else on the note moves it, anywhere off all
 * notes is empty (left-click creates there). Topmost = last in sorted order.
 */
export function hitTest(notes: MidiNote[], x: number, y: number, view: ViewConfig): Hit {
  for (let i = notes.length - 1; i >= 0; i--) {
    const note = notes[i];
    const r = noteRect(note, view);
    if (!insideRect(x, y, r)) continue;
    if (x >= r.x + r.w - RESIZE_HANDLE_PX) return { kind: "resize", note };
    return { kind: "body", note };
  }
  return { kind: "empty" };
}

/** All notes under a right-button sweep point (for continuous deletion). */
export function notesAt(notes: MidiNote[], x: number, y: number, view: ViewConfig): MidiNote[] {
  return notes.filter((n) => insideRect(x, y, noteRect(n, view)));
}
