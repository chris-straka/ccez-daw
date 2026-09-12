// Print-friendly staff layout (pure, headless).
//
// One clip lays out as measures left-to-right, wrapped into systems
// (rows) of `measuresPerSystem` measures. Coordinates are SVG pixels on a
// white page: x grows with beats, y grows downward from each system's top
// staff line. Everything here inverts cleanly (`beatForX`, `stepForY`) so
// pointer hits and tests can round-trip back to the piano-roll model.

import type { MidiClip, MidiNote } from "../pianoroll/model";
import { clefForPitch, staffStep, type Clef } from "./pitch";

export interface NotationLayoutOpts {
  /** Beats per measure (default 4 = 4/4). */
  beatsPerMeasure?: number;
  /** Measures per system/row (default 4). */
  measuresPerSystem?: number;
  /** Page content width in px (default 640). */
  pageWidth?: number;
  /** Left/right page margins in px (default 48 / 16). */
  marginLeft?: number;
  marginRight?: number;
  /** Gap between adjacent staff lines in px (default 10). */
  lineGap?: number;
  /** Vertical gap between systems in px (default 72). */
  systemGap?: number;
  /** Top of the first system in px (default 40). */
  topMargin?: number;
}

export interface NotationLayout {
  beatsPerMeasure: number;
  measuresPerSystem: number;
  pageWidth: number;
  marginLeft: number;
  marginRight: number;
  lineGap: number;
  systemGap: number;
  topMargin: number;
  /** Usable width of one measure in px. */
  measureWidth: number;
  /** Height of one 5-line staff in px (4 gaps). */
  staffHeight: number;
}

export function resolveLayout(opts?: NotationLayoutOpts): NotationLayout {
  const beatsPerMeasure = opts?.beatsPerMeasure ?? 4;
  const measuresPerSystem = opts?.measuresPerSystem ?? 4;
  const pageWidth = opts?.pageWidth ?? 640;
  const marginLeft = opts?.marginLeft ?? 48;
  const marginRight = opts?.marginRight ?? 16;
  const lineGap = opts?.lineGap ?? 10;
  const systemGap = opts?.systemGap ?? 72;
  const topMargin = opts?.topMargin ?? 40;
  const measureWidth = (pageWidth - marginLeft - marginRight) / measuresPerSystem;
  return {
    beatsPerMeasure,
    measuresPerSystem,
    pageWidth,
    marginLeft,
    marginRight,
    lineGap,
    systemGap,
    topMargin,
    measureWidth,
    staffHeight: lineGap * 4,
  };
}

export function measureOf(beat: number, layout: NotationLayout): number {
  return Math.floor(Math.max(0, beat) / layout.beatsPerMeasure);
}

export function systemOf(beat: number, layout: NotationLayout): number {
  return Math.floor(measureOf(beat, layout) / layout.measuresPerSystem);
}

/** Y of a staff step: step 8 (top line) sits on `systemTop`. */
export function yForStep(step: number, systemTop: number, layout: NotationLayout): number {
  return systemTop + ((8 - step) * layout.lineGap) / 2;
}

/** Inverse of `yForStep`, snapped to the nearest step. */
export function stepForY(y: number, systemTop: number, layout: NotationLayout): number {
  return Math.round(8 - ((y - systemTop) * 2) / layout.lineGap);
}

export function systemTop(system: number, layout: NotationLayout): number {
  return layout.topMargin + system * (layout.staffHeight + layout.systemGap);
}

/** X of a beat inside its measure cell (bar pad keeps bar lines clear). */
export function xForBeat(beat: number, layout: NotationLayout): number {
  const m = measureOf(Math.max(0, beat), layout);
  const inSystem = m % layout.measuresPerSystem;
  const beatInMeasure = Math.max(0, beat) - m * layout.beatsPerMeasure;
  const frac = Math.min(1, beatInMeasure / layout.beatsPerMeasure);
  const barPad = 8;
  const inner = layout.measureWidth - barPad * 2;
  return layout.marginLeft + inSystem * layout.measureWidth + barPad + frac * inner;
}

/** Inverse of `xForBeat` for pointer hits (system row must match). */
export function beatForX(x: number, measureInSystem: number, layout: NotationLayout): number {
  const barPad = 8;
  const inner = layout.measureWidth - barPad * 2;
  const frac = Math.min(1, Math.max(0, (x - layout.marginLeft - measureInSystem * layout.measureWidth - barPad) / inner));
  return (measureInSystem % layout.measuresPerSystem) * layout.beatsPerMeasure +
    frac * layout.beatsPerMeasure;
}

export interface PlacedNote {
  note: MidiNote;
  clef: Clef;
  step: number;
  measure: number;
  system: number;
  x: number;
  y: number;
}

/** Lay out every note of a clip; clef follows each note's pitch. */
export function layoutClip(clip: MidiClip, opts?: NotationLayoutOpts): { layout: NotationLayout; notes: PlacedNote[] } {
  const layout = resolveLayout(opts);
  const notes = clip.notes.map((note) => {
    const { clef, step } = staffStep(note.pitch, clefForPitch(note.pitch));
    const system = systemOf(note.start_beats, layout);
    return {
      note,
      clef,
      step,
      measure: measureOf(note.start_beats, layout),
      system,
      x: xForBeat(note.start_beats, layout),
      y: yForStep(step, systemTop(system, layout), layout),
    };
  });
  return { layout, notes };
}

/** How many systems the clip's length spans (at least 1). */
export function pageSystems(lengthBeats: number, opts?: NotationLayoutOpts): number {
  const layout = resolveLayout(opts);
  return Math.max(1, systemOf(Math.max(0, lengthBeats - 1e-9), layout) + 1);
}

/** Full page height in px for a clip length (print sizing). */
export function pageHeight(lengthBeats: number, opts?: NotationLayoutOpts): number {
  const layout = resolveLayout(opts);
  const n = pageSystems(lengthBeats, opts);
  return layout.topMargin + n * layout.staffHeight + (n - 1) * layout.systemGap + 24;
}

export type DisplayDuration = "whole" | "half" | "quarter" | "eighth" | "16th";

/** Nearest display glyph for a length in beats (4/4 note values). */
export function displayDuration(lenBeats: number): DisplayDuration {
  const table: Array<[number, DisplayDuration]> = [
    [4, "whole"],
    [2, "half"],
    [1, "quarter"],
    [0.5, "eighth"],
    [0.25, "16th"],
  ];
  let best: DisplayDuration = "16th";
  let bestDist = Infinity;
  for (const [beats, name] of table) {
    const d = Math.abs(lenBeats - beats);
    if (d < bestDist) {
      bestDist = d;
      best = name;
    }
  }
  return best;
}

/** True when the glyph gets a stem (everything shorter than a whole note). */
export function hasStem(d: DisplayDuration): boolean {
  return d !== "whole";
}

/** True when the stem points down (notes on/above the middle line). */
export function stemDown(step: number): boolean {
  return step >= 4;
}
