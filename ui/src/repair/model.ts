import type { MidiClip } from "../pianoroll/model";

/**
 * Track I: per-clip time/pitch + repair model (TS mirror of
 * `core/src/timepitch.rs` + `core/src/repair.rs`).
 *
 * Framework-free like the piano-roll geometry: the repair panel and the
 * scripted test share these functions with no DOM involved. Nothing here
 * touches the frozen v0 schema — pitch/time edits interpret the timeline
 * `ClipProps` sidecar (`pitch_semitones` ±24, `time_ratio` > 0), and repair
 * transforms live on engine assets beside the project.
 */

export const MAX_TRANSPOSE_ST = 24;

/** Twelve-tone equal temperament: semitones -> playback-rate ratio. */
export function semitonesToRatio(semitones: number): number {
  return Math.pow(2, semitones / 12);
}

/** Inverse of `semitonesToRatio`. */
export function ratioToSemitones(ratio: number): number {
  return 12 * Math.log2(ratio);
}

/** Integer note shift plus fractional bend remainder in [-0.5, 0.5]. */
export function splitPitch(semitones: number): { whole: number; bend: number } {
  const whole = Math.round(semitones);
  return { whole, bend: semitones - whole };
}

/** Throw unless a transpose shift is in the ±24 sidecar range. */
export function validateTranspose(semitones: number): void {
  if (!Number.isFinite(semitones) || Math.abs(semitones) > MAX_TRANSPOSE_ST) {
    throw new Error(`transpose ${semitones} out of range ±${MAX_TRANSPOSE_ST} semitones`);
  }
}

/** Throw unless a time ratio is a usable playback rate. */
export function validateRatio(ratio: number): void {
  if (!(ratio > 0 && Number.isFinite(ratio))) {
    throw new Error(`time ratio ${ratio} must be finite and > 0`);
  }
}

/** Transpose every note; errors when a result would leave 0..=127. */
export function transposeClip(clip: MidiClip, semitones: number): MidiClip {
  validateTranspose(semitones);
  const notes = clip.notes.map((n) => {
    const shifted = Math.round(n.pitch + semitones);
    if (shifted < 0 || shifted > 127) {
      throw new Error(`note ${n.note_id} would leave 0..=127 (${n.pitch} -> ${shifted})`);
    }
    return { ...n, pitch: shifted };
  });
  return { ...clip, notes: sortNotes(notes) };
}

/** Scale clip time by playback-rate ratio (faster = shorter). Invertible. */
export function timeScaleClip(clip: MidiClip, ratio: number): MidiClip {
  validateRatio(ratio);
  return {
    length_beats: clip.length_beats / ratio,
    notes: sortNotes(
      clip.notes.map((n) => ({
        ...n,
        start_beats: n.start_beats / ratio,
        len_beats: n.len_beats / ratio,
        timing_offset_beats: n.timing_offset_beats / ratio,
      })),
    ),
  };
}

/** Full per-clip edit: integer transpose + fractional bend, then time scale. */
export function applyPropsToMidi(
  clip: MidiClip,
  pitchSemitones: number,
  timeRatio: number,
): MidiClip {
  validateTranspose(pitchSemitones);
  validateRatio(timeRatio);
  const { whole, bend } = splitPitch(pitchSemitones);
  let out = transposeClip(clip, whole);
  if (bend !== 0) {
    out = {
      ...out,
      notes: out.notes.map((n) => {
        const b = n.pitch_bend + bend;
        if (Math.abs(b) > 48) throw new Error(`note ${n.note_id} bend ${b} would leave ±48 st`);
        return { ...n, pitch_bend: b };
      }),
    };
  }
  return timeScaleClip(out, timeRatio);
}

export interface WarpMarker {
  at_beats: number;
  shift_beats: number;
}

/** Shift applied at one beat: linear between pins, flat past the ends. */
export function warpBeat(markers: WarpMarker[], beat: number): number {
  const ms = markers.slice().sort((a, b) => a.at_beats - b.at_beats);
  if (ms.length === 0) return beat;
  if (beat <= ms[0].at_beats) return beat + ms[0].shift_beats;
  const last = ms[ms.length - 1];
  if (beat >= last.at_beats) return beat + last.shift_beats;
  for (let i = 0; i + 1 < ms.length; i++) {
    const a = ms[i];
    const b = ms[i + 1];
    if (beat >= a.at_beats && beat <= b.at_beats) {
      const span = b.at_beats - a.at_beats;
      const t = span === 0 ? 0 : (beat - a.at_beats) / span;
      return beat + a.shift_beats + t * (b.shift_beats - a.shift_beats);
    }
  }
  return beat;
}

/** Warp a clip: onsets move through the map, lengths preserved. */
export function applyWarpToMidi(clip: MidiClip, markers: WarpMarker[]): MidiClip {
  return {
    ...clip,
    notes: sortNotes(
      clip.notes.map((n) => {
        const onset = n.start_beats + n.timing_offset_beats;
        const warped = Math.max(0, warpBeat(markers, onset));
        return { ...n, start_beats: Math.max(0, warped - n.timing_offset_beats) };
      }),
    ),
  };
}

export interface SpectralEdit {
  /** Band center index into the magnitude frame. */
  band: number;
  /** Neighbors each side included in the cut. */
  width: number;
  /** 0 = full cut, 1 = untouched. */
  cut: number;
}

/** Validate a spectral notch without touching audio (loud errors). */
export function validateSpectralEdit(edit: SpectralEdit, bandCount: number): void {
  if (!Number.isInteger(edit.band) || edit.band < 0 || edit.band >= bandCount) {
    throw new Error(`band ${edit.band} out of range (0..${bandCount - 1})`);
  }
  if (!(edit.cut >= 0 && edit.cut <= 1)) {
    throw new Error(`notch cut ${edit.cut} must be in 0..=1`);
  }
}

/** Apply a validated notch to magnitude frames (returns zeroed count via gate). */
export function notchSpectrum(
  frames: number[][],
  edit: SpectralEdit,
): number[][] {
  validateSpectralEdit(edit, frames[0]?.length ?? 0);
  return frames.map((frame) => {
    const out = frame.slice();
    const lo = Math.max(0, edit.band - edit.width);
    const hi = Math.min(frame.length - 1, edit.band + edit.width);
    for (let b = lo; b <= hi; b++) out[b] *= edit.cut;
    return out;
  });
}

export interface CleanupOptions {
  /** Snap starts to this grid (beats); <= 0 disables. */
  quantizeGrid: number;
  /** Drop muted notes. */
  dropMuted: boolean;
  /** Truncate same-pitch overlaps. */
  fixOverlaps: boolean;
}

/** MIDI tidy: quantize (grid wins over feel past the ±0.25 lane), drop muted, fix overlaps. */
export function cleanupMidi(clip: MidiClip, opts: CleanupOptions): MidiClip {
  let notes = clip.notes.slice();
  if (opts.quantizeGrid > 0) {
    if (!Number.isFinite(opts.quantizeGrid)) throw new Error(`grid ${opts.quantizeGrid} must be finite`);
    notes = notes.map((n) => {
      const onset = n.start_beats + n.timing_offset_beats;
      const snapped = Math.round(onset / opts.quantizeGrid) * opts.quantizeGrid;
      let remainder = onset - snapped;
      if (Math.abs(remainder) > 0.25) remainder = 0;
      return {
        ...n,
        start_beats: Math.max(0, snapped),
        timing_offset_beats: snapped === 0 && remainder < 0 ? 0 : remainder,
      };
    });
  }
  if (opts.dropMuted) notes = notes.filter((n) => !n.muted);
  if (opts.fixOverlaps) {
    const TICK = 1 / 480;
    const byKey = notes.slice().sort((a, b) => a.pitch - b.pitch || a.channel - b.channel || a.start_beats - b.start_beats);
    for (let i = 1; i < byKey.length; i++) {
      const prev = byKey[i - 1];
      const cur = byKey[i];
      if (prev.pitch === cur.pitch && prev.channel === cur.channel) {
        const end = prev.start_beats + prev.len_beats;
        if (cur.start_beats < end) prev.len_beats = Math.max(TICK, cur.start_beats - prev.start_beats);
      }
    }
    notes = byKey;
  }
  return { ...clip, notes: sortNotes(notes) };
}

type Note = MidiClip["notes"][number];

function sortNotes(notes: Note[]): Note[] {
  return notes
    .slice()
    .sort((a, b) => a.start_beats - b.start_beats || a.pitch - b.pitch || a.note_id - b.note_id);
}

/** Demo phrase: C–E–G quarter notes (mirrors Rust `sample_phrase`). */
export function samplePhrase(): MidiClip {
  const pitches = [60, 64, 67];
  return {
    length_beats: 4,
    notes: pitches.map((pitch, i) => ({
      note_id: i + 1,
      pitch,
      velocity: 100,
      start_beats: i,
      len_beats: 0.9,
      channel: 1,
      probability: 1,
      ratchets: 1,
      timing_offset_beats: 0,
      pitch_bend: 0,
      pressure: 0,
      timbre: 0,
      muted: false,
    })),
  };
}
