/**
 * Track M: deterministic local transcription baselines (the floor, not
 * the ceiling). Offline, dependency-free, fully deterministic — the layer
 * that stays true regardless of which neural model a sidecar serves.
 * Mirrors `core/src/ai/{drums,melody,chords}.rs`; keep the two in sync.
 */
import {
  draftMidiClip,
  SIDECAR_IDS,
  type ClipDraft,
  type MidiNoteDraft,
  type TranscriptionPlan,
} from "./types.js";

export interface DrumOnset {
  beat: number;
  energy: number;
}

export interface PitchFrame {
  beat: number;
  /** Estimated MIDI pitch, or null for unvoiced / silent frames. */
  midi: number | null;
}

const DRUM_PITCH = { kick: 36, snare: 38, hat: 42 } as const;

function requireFinite(v: number, what: string): void {
  if (typeof v !== "number" || !Number.isFinite(v)) {
    throw new Error(`${what} must be a finite number`);
  }
}

export function voiceFor(onset: DrumOnset): "kick" | "snare" | "hat" {
  const downbeat = Math.abs(onset.beat % 4) < 1e-9;
  const e = downbeat ? Math.min(1, onset.energy + 0.34) : onset.energy;
  if (e >= 0.66) return "kick";
  if (e >= 0.33) return "snare";
  return "hat";
}

export function quantize16th(beat: number): number {
  return Math.round(beat * 4) / 4;
}

function toNotes(
  hits: { pitch: number; start: number; len: number; velocity: number; channel: number }[],
): MidiNoteDraft[] {
  return hits.map((h, i) => ({
    note_id: i,
    pitch: h.pitch,
    velocity: Math.max(1, Math.min(127, Math.round(h.velocity))),
    start_beats: h.start,
    len_beats: h.len,
    channel: h.channel,
  }));
}

/** Build editable drum notes from onsets (16th-note hits, accent velocity). */
export function drumsFromOnsets(
  onsets: DrumOnset[],
  lengthBeats: number,
): MidiNoteDraft[] {
  requireFinite(lengthBeats, "lengthBeats");
  if (lengthBeats <= 0) throw new Error("lengthBeats must be > 0");
  return toNotes(
    onsets.map((o) => {
      requireFinite(o.beat, "onset beat");
      if (o.beat < 0) throw new Error("onset beat must be >= 0");
      if (o.beat > lengthBeats) {
        throw new Error(`onset beat ${o.beat} past clip length ${lengthBeats}`);
      }
      if (!(o.energy >= 0 && o.energy <= 1)) {
        throw new Error(`onset energy ${o.energy} must be in 0..=1`);
      }
      return {
        pitch: DRUM_PITCH[voiceFor(o)],
        start: quantize16th(o.beat),
        len: 0.25,
        velocity: o.energy * 126 + 1,
        channel: 9,
      };
    }),
  );
}

/** Peak-pick onsets from an energy envelope (one-frame refractory). */
export function detectDrums(
  frames: number[],
  framesPerBeat: number,
  threshold: number,
  lengthBeats: number,
): MidiNoteDraft[] {
  requireFinite(framesPerBeat, "framesPerBeat");
  if (framesPerBeat <= 0) throw new Error("framesPerBeat must be > 0");
  if (!(threshold >= 0 && threshold <= 1)) {
    throw new Error("threshold must be in 0..=1");
  }
  const onsets: DrumOnset[] = [];
  let armed = true;
  frames.forEach((energy, i) => {
    if (!(energy >= 0 && energy <= 1)) {
      throw new Error(`frame ${i} energy ${energy} must be in 0..=1`);
    }
    if (armed && energy >= threshold) {
      onsets.push({ beat: i / framesPerBeat, energy });
      armed = false;
    } else if (energy < threshold) {
      armed = true;
    }
  });
  return drumsFromOnsets(onsets, lengthBeats);
}

/** Segment a monophonic pitch track into sustained notes. */
export function transcribeMelodyNotes(
  frames: PitchFrame[],
  minLenBeats: number,
  lengthBeats: number,
): MidiNoteDraft[] {
  requireFinite(lengthBeats, "lengthBeats");
  if (lengthBeats <= 0) throw new Error("lengthBeats must be > 0");
  if (!(minLenBeats >= 0)) throw new Error("minLenBeats must be >= 0");
  const out: MidiNoteDraft[] = [];
  let run: { pitch: number; start: number } | null = null;
  let noteId = 0;
  const flush = (pitch: number, start: number, end: number) => {
    if (end - start >= minLenBeats && end > start) {
      out.push({
        note_id: noteId++,
        pitch,
        velocity: 96,
        start_beats: start,
        len_beats: end - start,
        channel: 0,
      });
    }
  };
  for (let i = 0; i < frames.length; i++) {
    const f = frames[i];
    requireFinite(f.beat, `frame ${i} beat`);
    if (f.beat < 0) throw new Error(`frame ${i} beat must be >= 0`);
    if (i > 0 && f.beat < frames[i - 1].beat) {
      throw new Error(`frame ${i} beat out of order`);
    }
    if (f.midi !== null && (f.midi < 0 || f.midi > 127)) {
      throw new Error(`pitch ${f.midi} out of range 0..=127`);
    }
    if (run && f.midi !== run.pitch) {
      flush(run.pitch, run.start, f.beat);
      run = f.midi === null ? null : { pitch: f.midi, start: f.beat };
    } else if (!run && f.midi !== null) {
      run = { pitch: f.midi, start: f.beat };
    }
  }
  if (run) {
    const end = frames.length > 0 ? frames[frames.length - 1].beat : run.start;
    flush(run.pitch, run.start, Math.max(end, run.start + Number.EPSILON));
  }
  return out;
}

export const NOTE_NAMES = [
  "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
] as const;

function triadScore(chroma: number[], root: number, minor: boolean): number {
  const third = minor ? 3 : 4;
  const tones = new Set([0, third, 7]);
  let score = 0;
  chroma.forEach((e, pc) => {
    const rel = (((pc - root) % 12) + 12) % 12;
    score += tones.has(rel) ? e : -0.5 * e;
  });
  return score;
}

export function recognizeChord(
  chroma: number[],
  bar: number,
): { root: number; minor: boolean; name: string } {
  if (chroma.length !== 12 || chroma.some((e) => !(e >= 0))) {
    throw new Error(`bar ${bar}: chroma must be twelve values >= 0`);
  }
  if (chroma.every((e) => e === 0)) {
    throw new Error(`bar ${bar}: empty chroma (silent bar)`);
  }
  let best = { score: -Infinity, root: 0, minor: false };
  for (let root = 0; root < 12; root++) {
    for (const minor of [false, true]) {
      const score = triadScore(chroma, root, minor);
      if (score > best.score) best = { score, root, minor };
    }
  }
  const base = NOTE_NAMES[best.root];
  return { ...best, name: best.minor ? `${base}m` : base };
}

/** Recognize one triad per bar; render winners as block-chord pads. */
export function transcribeChordNotes(
  chromas: number[][],
  beatsPerBar: number,
): { labels: string[]; notes: MidiNoteDraft[] } {
  requireFinite(beatsPerBar, "beatsPerBar");
  if (beatsPerBar <= 0) throw new Error("beatsPerBar must be > 0");
  if (chromas.length === 0) throw new Error("need at least one bar of chroma");
  const labels: string[] = [];
  const notes: MidiNoteDraft[] = [];
  let noteId = 0;
  chromas.forEach((chroma, bar) => {
    if (chroma.every((e) => e === 0)) return; // rest bar keeps alignment
    const { root, minor, name } = recognizeChord(chroma, bar);
    labels.push(name);
    const third = minor ? 3 : 4;
    for (const interval of [0, third, 7]) {
      notes.push({
        note_id: noteId++,
        pitch: 48 + root + interval,
        velocity: 80,
        start_beats: bar * beatsPerBar,
        len_beats: beatsPerBar * 0.95,
        channel: 0,
      });
    }
  });
  return { labels, notes };
}

function clipShell(
  id: string,
  trackId: string,
  name: string,
  startBeats: number,
  lengthBeats: number,
): ClipDraft {
  return {
    id,
    track_id: trackId,
    name,
    start_beats: startBeats,
    length_beats: lengthBeats,
    kind: "Midi",
    source: `take:${id}`,
  };
}

/** Local drums plan: one `ClipAdded` op + editable notes. */
export function localDrumsPlan(
  notes: MidiNoteDraft[],
  opts: { clipId: string; trackId: string; lengthBeats: number },
): TranscriptionPlan {
  const clip = clipShell(opts.clipId, opts.trackId, "AI drums", 0, opts.lengthBeats);
  return {
    kind: "drums",
    summary: `Transcribe drums: ${notes.length} hits onto ${opts.trackId} (sidecar ${SIDECAR_IDS.drums})`,
    ops: [draftMidiClip(clip)],
    warnings: [],
    notes,
  };
}

/** Local melody plan: one `ClipAdded` op + editable notes. */
export function localMelodyPlan(
  notes: MidiNoteDraft[],
  opts: { clipId: string; trackId: string; lengthBeats: number },
): TranscriptionPlan {
  const clip = clipShell(opts.clipId, opts.trackId, "AI melody", 0, opts.lengthBeats);
  return {
    kind: "melody",
    summary: `Transcribe melody: ${notes.length} notes onto ${opts.trackId} (sidecar ${SIDECAR_IDS.melody})`,
    ops: [draftMidiClip(clip)],
    warnings: [],
    notes,
  };
}

/** Local chords plan: one `ClipAdded` op + labels + editable pad notes. */
export function localChordsPlan(
  labels: string[],
  notes: MidiNoteDraft[],
  opts: { clipId: string; trackId: string; lengthBeats: number },
): TranscriptionPlan {
  const clip = clipShell(
    opts.clipId,
    opts.trackId,
    `AI chords (${labels.join(", ")})`,
    0,
    opts.lengthBeats,
  );
  return {
    kind: "chords",
    summary: `Transcribe chords: ${labels.join(", ")} onto ${opts.trackId} (sidecar ${SIDECAR_IDS.chords})`,
    ops: [draftMidiClip(clip)],
    warnings: [],
    notes,
    labels,
  };
}
