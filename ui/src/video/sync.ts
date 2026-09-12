/**
 * Agent 2 (video track): engine-clock -> `<video>` sync decisions.
 *
 * The engine owns the transport clock (beats at a tempo, Playing/Stopped);
 * the preview is a plain HTML video element the engine never sees. This
 * module is the pure policy between them so it can be unit-tested without
 * a DOM: given the transport position and the element's current time,
 * decide `seek` (jump), `coast` (leave it playing), or `hold` (paused:
 * pin to the exact frame). The Solid component in `Preview.tsx` just
 * executes the decision against a real `HTMLVideoElement`.
 *
 * Rule of thumb (standard AV-sync practice): while playing, small drift
 * (< SEEK_THRESHOLD_S) is left alone — seeking every frame stutters — and
 * only large drift seeks. While paused, always pin to the exact frame so
 * stepping the transport steps the picture.
 */

import { videoTimeForTransport } from "./model";
import type { VideoDoc } from "./model";

export type PlayState = "Playing" | "Stopped";

export type SyncAction =
  | { kind: "seek"; toSeconds: number }
  | { kind: "coast" }
  | { kind: "hold"; toSeconds: number | null }
  | { kind: "blank" };

/** Drift above this while playing forces a re-seek. */
export const SEEK_THRESHOLD_S = 0.12;
/** Drift above this while playing also re-asserts play() (element stalled). */
export const STALL_THRESHOLD_S = 0.5;

export interface SyncInput {
  doc: VideoDoc;
  transportBeats: number;
  tempo: number;
  state: PlayState;
  /** Element's currentTime when sampled (null = no element/mounted yet). */
  elementTime: number | null;
  /** True when the element reports paused/ended while transport plays. */
  elementPaused: boolean;
}

/**
 * Decide what the preview element should do this tick. Pure and total:
 * no DOM, no clock reads — the caller samples those into `SyncInput`.
 */
export function syncDecision(input: SyncInput): SyncAction {
  const target = videoTimeForTransport(input.doc, input.transportBeats, input.tempo);
  if (target === null) {
    // No picture under the playhead: pause-side holds (blank poster),
    // play-side seeks nothing — caller should pause the element.
    return input.state === "Playing" ? { kind: "blank" } : { kind: "hold", toSeconds: null };
  }
  if (input.state !== "Playing") return { kind: "hold", toSeconds: target };
  if (input.elementTime === null) return { kind: "seek", toSeconds: target };
  const drift = Math.abs(input.elementTime - target);
  if (input.elementPaused && drift > SEEK_THRESHOLD_S)
    return { kind: "seek", toSeconds: target };
  if (drift > SEEK_THRESHOLD_S) return { kind: "seek", toSeconds: target };
  return { kind: "coast" };
}

/**
 * Execute one decision against a video-like element. Split out so the
 * component stays thin; tested with a stub element (see video-sync test).
 */
export async function applySyncDecision(
  el: {
    currentTime: number;
    paused: boolean;
    play(): Promise<void> | void;
    pause(): void;
  },
  decision: SyncAction,
  playing: boolean,
): Promise<void> {
  switch (decision.kind) {
    case "seek":
      el.currentTime = decision.toSeconds;
      if (playing && el.paused) await el.play();
      break;
    case "coast":
      if (playing && el.paused) await el.play();
      break;
    case "hold":
      if (decision.toSeconds !== null) el.currentTime = decision.toSeconds;
      if (!el.paused) el.pause();
      break;
    case "blank":
      if (!el.paused) el.pause();
      break;
  }
}
