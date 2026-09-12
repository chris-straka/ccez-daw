/**
 * Track M: transcription sidecar types (drums / melody / chords).
 *
 * Transcription never writes project state directly. It produces ordinary
 * op-log drafts (`OpDraft`) applied with an `ai:<sidecar>` actor, so every
 * AI clip is undoable by design (`contracts/op-log-format.md`). Field names
 * mirror `Clip` / `Op` in `core/src/model.rs` (snake_case) so the MCP
 * surface cannot drift from the UI surfaces.
 */

export type TranscriptionKind = "drums" | "melody" | "chords";

export const SIDECAR_IDS: Record<TranscriptionKind, string> = {
  drums: "transcribe-drums",
  melody: "transcribe-melody",
  chords: "transcribe-chords",
};

export type OpKind =
  | "TrackAdded"
  | "ClipAdded"
  | "ClipMoved"
  | "ParamSet"
  | "TempoSet"
  | "UndoMarker";

/** One op-log entry minus `seq` (assigned by `op_apply`, never here). */
export interface OpDraft {
  kind: OpKind;
  target: string;
  valueJson: string;
}

/** Minimal `Clip` payload inside a `ClipAdded` draft. */
export interface ClipDraft {
  id: string;
  track_id: string;
  name: string;
  start_beats: number;
  length_beats: number;
  kind: "Audio" | "Midi";
  source: string;
}

/** One editable transcribed note (mirrors `MidiNote` defaults). */
export interface MidiNoteDraft {
  note_id: number;
  pitch: number;
  velocity: number;
  start_beats: number;
  len_beats: number;
  channel: number;
}

/** Result of one transcription: human summary + ordinary ops + warnings. */
export interface TranscriptionPlan {
  kind: TranscriptionKind;
  summary: string;
  ops: OpDraft[];
  warnings: string[];
  /** Editable note data behind the committed clip(s), for preview/edit. */
  notes: MidiNoteDraft[];
  /** Chord symbols, when kind === "chords". */
  labels?: string[];
}

/** Actor for AI output: `ai:<sidecar>` (mirrors `engine::valid_actor`). */
export function aiActor(sidecar: string): string {
  const name = sidecar.trim();
  if (name.length === 0) throw new Error("AI sidecar name must be non-empty");
  return `ai:${name}`;
}

/** Frozen actor rule mirror: transcription must use `ai:<x>`. */
export function validAiActor(actor: string): boolean {
  return actor.startsWith("ai:") && actor.length > 3;
}

/** Build the single `ClipAdded` draft committing one MIDI clip shell. */
export function draftMidiClip(clip: ClipDraft): OpDraft {
  return {
    kind: "ClipAdded",
    target: clip.id,
    valueJson: JSON.stringify(clip),
  };
}
