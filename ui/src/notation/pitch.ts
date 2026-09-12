// Pitch <-> staff-step mapping (pure, headless).
//
// A staff *step* is a diatonic index offset from the clef's bottom line:
// step 0 = bottom line, 1 = first space, ..., 8 = top line, 9+ / negative =
// ledger territory above / below. Steps are enharmonic-blind by design: C#
// and Db share one step, and `accidentalFor` recovers the spelling side.

export type Clef = "treble" | "bass";

export const PITCH_CLASS_NAMES = [
  "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
] as const;

/** Semitone class -> diatonic degree (C=0 .. B=6); accidentals share degree. */
const DEGREE = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6] as const;

/** True for the five black-key classes (rendered with a sharp sign). */
const SHARP_CLASS = new Set([1, 3, 6, 8, 10]);

/** Scientific pitch name, MIDI 60 = C4. */
export function pitchName(pitch: number): string {
  const p = Math.round(pitch);
  return `${PITCH_CLASS_NAMES[((p % 12) + 12) % 12]}${Math.floor(p / 12) - 1}`;
}

/** Absolute diatonic index of a pitch (C-1 = 0, sharps share the natural). */
export function diatonicIndex(pitch: number): number {
  const p = Math.round(pitch);
  const octave = Math.floor(p / 12) - 1;
  return octave * 7 + DEGREE[((p % 12) + 12) % 12];
}

/** Default clef split at middle C (60): sings treble, bass below. */
export function clefForPitch(pitch: number): Clef {
  return pitch >= 60 ? "treble" : "bass";
}

/** Bottom-line pitch per clef: E4 (64) treble, G2 (43) bass. */
export function bottomLinePitch(clef: Clef): number {
  return clef === "treble" ? 64 : 43;
}

function bottomDiatonic(clef: Clef): number {
  return diatonicIndex(bottomLinePitch(clef));
}

/** Staff step of a pitch on a clef (0 = bottom line, 8 = top line). */
export function staffStep(pitch: number, clef?: Clef): { clef: Clef; step: number } {
  const c = clef ?? clefForPitch(pitch);
  return { clef: c, step: diatonicIndex(pitch) - bottomDiatonic(c) };
}

/** Natural (accidental-free) pitch sitting on a step of a clef. */
export function naturalPitchAtStep(step: number, clef: Clef): number {
  const base = bottomLinePitch(clef);
  const baseClass = ((base % 12) + 12) % 12;
  const baseOctave = Math.floor(base / 12);
  // Degree of the bottom line within its octave (E=2, G=4).
  const baseDeg = DEGREE[baseClass];
  const target = baseDeg + step;
  // Floor-divide degrees into octaves so negative steps work below the staff.
  const octShift = Math.floor(target / 7);
  const deg = ((target % 7) + 7) % 7;
  const NATURAL_SEMI = [0, 2, 4, 5, 7, 9, 11];
  return (baseOctave + octShift) * 12 + NATURAL_SEMI[deg];
}

/** Sharp sign needed to spell this pitch ("#"), else "". */
export function accidentalFor(pitch: number): string {
  return SHARP_CLASS.has(((Math.round(pitch) % 12) + 12) % 12) ? "#" : "";
}

/** Sharp glyph text for SVG ("#"), else "". Spelling stays sharp-only in v1. */
export function accidentalGlyph(pitch: number): string {
  return SHARP_CLASS.has(((Math.round(pitch) % 12) + 12) % 12) ? "♯" : "";
}

/** Ledger-line count for a step (0 while 0..8, i.e. inside the staff). */
export function ledgerLines(step: number): number {
  if (step > 8) return Math.ceil((step - 8) / 2);
  if (step < 0) return Math.ceil((0 - step) / 2);
  return 0;
}

/** True when the step sits on a line (even steps), else a space. */
export function isLine(step: number): boolean {
  return step % 2 === 0;
}
