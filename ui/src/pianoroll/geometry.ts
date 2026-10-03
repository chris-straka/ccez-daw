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
  /** Highest visible pitch (default 127). */
  topPitch?: number;
  /** Rows in the window (default 128 = the whole range). */
  rowsVisible?: number;
}

export const MIN_PITCH = 0;
export const MAX_PITCH = 127;
/** Narrowest window: two octaves minus a fifth still shows chord shapes. */
export const MIN_ROWS = 12;
/** Right-edge grab zone width in px for resize vs move disambiguation. */
export const RESIZE_HANDLE_PX = 6;

/** Clamped row count for the window (default: the whole range). */
export function visibleRows(view: ViewConfig): number {
  const r = Math.floor(view.rowsVisible ?? 128);
  return Math.min(128, Math.max(MIN_ROWS, r));
}

/** Clamped top pitch: the window always stays inside 0..127. */
export function visibleTop(view: ViewConfig): number {
  const rows = visibleRows(view);
  const t = Math.floor(view.topPitch ?? MAX_PITCH);
  return Math.min(MAX_PITCH, Math.max(rows - 1, t));
}

/** Lowest visible pitch (derived, clamped). */
export function visibleBottom(view: ViewConfig): number {
  return Math.max(MIN_PITCH, visibleTop(view) - visibleRows(view) + 1);
}

/** Scroll the window by whole rows (positive = toward higher pitches). */
export function scrollTop(top: number, rows: number, deltaRows: number): number {
  const r = Math.min(128, Math.max(MIN_ROWS, Math.floor(rows)));
  return Math.min(MAX_PITCH, Math.max(r - 1, Math.floor(top) + Math.round(deltaRows)));
}

/** Zoom the window: +1 doubles rows (out), -1 halves (in). */
export function zoomRows(rows: number, dir: 1 | -1): number {
  const r = Math.min(128, Math.max(MIN_ROWS, Math.floor(rows)));
  return dir > 0 ? Math.min(128, r * 2) : Math.max(MIN_ROWS, Math.floor(r / 2));
}

/** Top pitch that keeps `pitch` under cursor height `y` after a zoom. */
export function anchorTop(pitch: number, cursorY: number, rowH2: number, rows: number): number {
  const r = Math.min(128, Math.max(MIN_ROWS, Math.floor(rows)));
  const top = Math.round(pitch + cursorY / rowH2);
  return Math.min(MAX_PITCH, Math.max(r - 1, top));
}

const NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/** Scientific pitch name with C4 = MIDI 60 (C-1 .. G9). */
export function pitchName(pitch: number): string {
  const p = Math.min(MAX_PITCH, Math.max(MIN_PITCH, Math.round(pitch)));
  return `${NAMES[p % 12]}${Math.floor(p / 12) - 1}`;
}

export function rowH(view: ViewConfig): number {
  return view.height / visibleRows(view);
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

/** Pitch rows run top (`topPitch`) to bottom: row `p` occupies `[y, y+rowH)`. */
export function pitchToY(pitch: number, view: ViewConfig): number {
  return (visibleTop(view) - pitch) * rowH(view);
}

export function yToPitch(y: number, view: ViewConfig): number {
  const p = visibleTop(view) - Math.floor(y / rowH(view));
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
