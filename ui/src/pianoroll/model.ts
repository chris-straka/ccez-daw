// Piano-roll note model: a UI-side mirror of `core/src/midi` (Track F).
//
// The frozen v0 `Clip { kind: Midi, source }` shape carries no notes, so the
// Rust `MidiClip`/`MidiNote` live beside the schema as an opaque JSON asset.
// This file mirrors that JSON shape (snake_case fields) with Zod v4 schemas
// so the piano roll can validate, encode, and decode clips without touching
// `ui/src/generated/*` (which is typegen-owned — drift gate fails on edits).

import { z } from "zod";

export const MidiNoteSchema = z.object({
  note_id: z.number().int().nonnegative(),
  pitch: z.number().int().min(0).max(127),
  velocity: z.number().int().min(1).max(127),
  start_beats: z.number().finite(),
  len_beats: z.number().finite().positive(),
  channel: z.number().int().min(0).max(15),
  probability: z.number().min(0).max(1),
  ratchets: z.number().int().min(1),
  timing_offset_beats: z.number().finite().min(-0.25).max(0.25),
  pitch_bend: z.number().finite().min(-48).max(48),
  pressure: z.number().min(0).max(1),
  timbre: z.number().min(0).max(1),
  muted: z.boolean(),
});
export type MidiNote = z.infer<typeof MidiNoteSchema>;

export const MidiClipSchema = z.object({
  length_beats: z.number().finite().positive(),
  notes: z.array(MidiNoteSchema),
});
export type MidiClip = z.infer<typeof MidiClipSchema>;

/** Minimal note: full probability, no ratchet, no offset, flat MPE. */
export function newNote(
  note_id: number,
  pitch: number,
  velocity: number,
  start_beats: number,
  len_beats: number,
): MidiNote {
  return {
    note_id,
    pitch,
    velocity,
    start_beats,
    len_beats,
    channel: 0,
    probability: 1,
    ratchets: 1,
    timing_offset_beats: 0,
    pitch_bend: 0,
    pressure: 0,
    timbre: 0,
    muted: false,
  };
}

/** Throw a human message on the first invalid field (mirrors Rust validate). */
export function validateNote(note: MidiNote): void {
  MidiNoteSchema.parse(note);
}

/** Parse bytes previously produced by `encodeClip`, re-sorting like Rust. */
export function decodeClip(bytes: Uint8Array | string): MidiClip {
  const text = typeof bytes === "string" ? bytes : new TextDecoder().decode(bytes);
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new Error("midi decode: invalid JSON");
  }
  const clip = MidiClipSchema.parse(raw);
  const seen = new Set<number>();
  for (const n of clip.notes) {
    if (seen.has(n.note_id)) throw new Error(`duplicate note_id ${n.note_id}`);
    seen.add(n.note_id);
  }
  return { ...clip, notes: sortNotes(clip.notes) };
}

/** Serialize to the opaque bytes stored behind a clip `source` asset key. */
export function encodeClip(clip: MidiClip): string {
  const parsed = MidiClipSchema.parse(clip);
  return JSON.stringify({ ...parsed, notes: sortNotes(parsed.notes) });
}

/** Equal-tempered frequency for a MIDI pitch (A4 = 440 Hz). */
export function midiToFreq(pitch: number): number {
  return 440 * Math.pow(2, (pitch - 69) / 12);
}

export function sortNotes(notes: MidiNote[]): MidiNote[] {
  return [...notes].sort(
    (a, b) =>
      a.start_beats - b.start_beats ||
      a.pitch - b.pitch ||
      a.note_id - b.note_id,
  );
}

/** Next free `note_id` (max + 1, starting at 1 like the Rust tests). */
export function nextNoteId(clip: MidiClip): number {
  let max = 0;
  for (const n of clip.notes) max = Math.max(max, n.note_id);
  return max + 1;
}
