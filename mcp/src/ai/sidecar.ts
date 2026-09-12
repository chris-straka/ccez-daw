/**
 * Track M: transcription sidecar pattern (models unpinned).
 *
 * `TranscriptionProvider.transcribe` is the only seam a neural model
 * touches. The local provider (today's default) runs the deterministic
 * baselines; a future HTTP sidecar may serve any model — the model id is
 * a plain string, nothing pins a version. Swapping models changes plans,
 * never op shapes or UI.
 */
import {
  localChordsPlan,
  localDrumsPlan,
  localMelodyPlan,
  transcribeChordNotes,
  transcribeMelodyNotes,
  detectDrums,
  type DrumOnset,
  type PitchFrame,
} from "./local.js";
import type { TranscriptionKind, TranscriptionPlan } from "./types.js";

export type TranscriptionInput =
  | { kind: "drums"; frames: number[]; framesPerBeat: number; threshold: number; lengthBeats: number; clipId: string; trackId: string }
  | { kind: "melody"; frames: PitchFrame[]; minLenBeats: number; lengthBeats: number; clipId: string; trackId: string }
  | { kind: "chords"; chromas: number[][]; beatsPerBar: number; clipId: string; trackId: string };

export function inputKind(input: TranscriptionInput): TranscriptionKind {
  return input.kind;
}

export interface TranscriptionProvider {
  /** Sidecar name; becomes the op actor `ai:<id>`. */
  readonly id: string;
  transcribe(input: TranscriptionInput): Promise<TranscriptionPlan>;
}

export class LocalTranscriptionProvider implements TranscriptionProvider {
  readonly id: string;

  constructor(sidecarId = "transcribe-local") {
    this.id = sidecarId;
  }

  async transcribe(input: TranscriptionInput): Promise<TranscriptionPlan> {
    switch (input.kind) {
      case "drums": {
        const notes = detectDrums(
          input.frames,
          input.framesPerBeat,
          input.threshold,
          input.lengthBeats,
        );
        return localDrumsPlan(notes, input);
      }
      case "melody": {
        const notes = transcribeMelodyNotes(input.frames, input.minLenBeats, input.lengthBeats);
        return localMelodyPlan(notes, input);
      }
      case "chords": {
        const { labels, notes } = transcribeChordNotes(input.chromas, input.beatsPerBar);
        return localChordsPlan(labels, notes, {
          ...input,
          lengthBeats: input.beatsPerBar * input.chromas.length,
        });
      }
    }
  }
}

export interface TranscriptionSidecarOptions {
  endpoint: string;
  /** Unpinned model id, e.g. "whisper-large-v3" — a plain string, never enforced. */
  model: string;
  sidecarId?: string;
  timeoutMs?: number;
}

export class HttpTranscriptionSidecar implements TranscriptionProvider {
  readonly id: string;
  readonly model: string;
  readonly endpoint: string;
  readonly timeoutMs: number;

  constructor(opts: TranscriptionSidecarOptions) {
    this.endpoint = opts.endpoint;
    this.model = opts.model;
    this.id = opts.sidecarId ?? "transcribe-sidecar";
    this.timeoutMs = opts.timeoutMs ?? 30_000;
  }

  async transcribe(input: TranscriptionInput): Promise<TranscriptionPlan> {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), this.timeoutMs);
    try {
      const res = await fetch(`${this.endpoint}/transcribe`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ model: this.model, kind: input.kind, input }),
        signal: ctrl.signal,
      });
      if (!res.ok) throw new Error(`transcription sidecar ${res.status}`);
      const plan = (await res.json()) as TranscriptionPlan;
      if (!Array.isArray(plan.ops)) {
        throw new Error("transcription sidecar returned no ops[]");
      }
      return plan;
    } finally {
      clearTimeout(timer);
    }
  }
}

/** Transcribe via the given provider (default: deterministic local). */
export function transcribeWith(
  input: TranscriptionInput,
  provider: TranscriptionProvider = new LocalTranscriptionProvider(),
): Promise<TranscriptionPlan> {
  return provider.transcribe(input);
}

/** Test helper: a provider that must never be called (asserts offline use). */
export class UnreachableProvider implements TranscriptionProvider {
  readonly id = "unreachable";
  async transcribe(_input: TranscriptionInput): Promise<TranscriptionPlan> {
    throw new Error("unreachable provider must not be called");
  }
}

export type { DrumOnset, PitchFrame };
