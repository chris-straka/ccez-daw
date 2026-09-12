/**
 * Track M: transcription sidecars — public entry point.
 *
 * Transcription compiles audio/pitch/chroma evidence into ordinary
 * undoable ops (sidecar pattern, models unpinned): run a provider in a
 * background `TranscriptionJob`, then apply the resulting plan with an
 * `ai:<sidecar>` actor through `op_apply` (production) or `NlOpStore`
 * (tests). Nothing here invents an op kind — every draft reuses a frozen
 * `OpKind` from `contracts/op-log-format.md`.
 */
export type {
  ClipDraft,
  MidiNoteDraft,
  OpDraft,
  OpKind,
  TranscriptionKind,
  TranscriptionPlan,
} from "./types.js";
export {
  aiActor,
  draftMidiClip,
  SIDECAR_IDS,
  validAiActor,
} from "./types.js";
export {
  detectDrums,
  drumsFromOnsets,
  localChordsPlan,
  localDrumsPlan,
  localMelodyPlan,
  NOTE_NAMES,
  quantize16th,
  recognizeChord,
  transcribeChordNotes,
  transcribeMelodyNotes,
  voiceFor,
  type DrumOnset,
  type PitchFrame,
} from "./local.js";
export {
  HttpTranscriptionSidecar,
  LocalTranscriptionProvider,
  transcribeWith,
  UnreachableProvider,
  type TranscriptionInput,
  type TranscriptionProvider,
  type TranscriptionSidecarOptions,
} from "./sidecar.js";
export {
  submitTranscription,
  TranscriptionJob,
  type JobProgress,
  type JobStatus,
} from "./jobs.js";
