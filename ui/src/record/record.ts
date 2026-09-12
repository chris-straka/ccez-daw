import type { Clip, ClipKind, EngineState } from "../generated/project";

/**
 * Agent 7 (record workflow): punch in/out, count-in/metronome, software
 * monitoring with latency compensation, and take lanes into the comping
 * model.
 *
 * Exact TypeScript mirror of `core/src/record.rs`. The two implementations
 * must agree on: punch-range validation and edge gating (same 1e-9 beat
 * epsilon), count-in click schedules and accents, monitor-mode truth table,
 * latency-to-beats compensation, lane ordering, and the `take:<id>` source
 * convention that `comp.ts` consumes. `ui/tests/record.test.ts` pins the
 * same punch-take behavior as the Rust `record` tests, so drift on either
 * side shows up as red.
 *
 * Like the Rust side this module adds no IPC or project-schema surface:
 * punch takes are ordinary clips applied through the frozen `ClipAdded` op.
 * Hardware inserts are explicitly out of scope — there is no code path
 * here that addresses outboard gear.
 */

const EPS = 1e-9;

export interface PunchRange {
  start_beats: number;
  end_beats: number;
}

export type PunchMode =
  | { kind: "manual" }
  | { kind: "auto"; range: PunchRange };

export interface CountInClick {
  offset_beats: number;
  accent: boolean;
}

export interface CountIn {
  bars: number;
  beats_per_bar: number;
}

export type MonitorMode = "off" | "auto" | "on";

export interface TakeLane {
  take_id: string;
  lane: number;
}

export type RecordFailure =
  | "bad-punch-range"
  | "bad-count-in"
  | "bad-tempo"
  | "bad-latency"
  | "no-takes";

export class RecordError extends Error {
  readonly failure: RecordFailure;
  constructor(failure: RecordFailure, message: string) {
    super(message);
    this.failure = failure;
  }
}

/** Validate a punch range: finite, `start >= 0`, `start < end`. */
export function validatePunch(
  startBeats: number,
  endBeats: number,
): PunchRange {
  if (!Number.isFinite(startBeats) || !Number.isFinite(endBeats)) {
    throw new RecordError("bad-punch-range", "bounds must be finite");
  }
  if (startBeats < 0) {
    throw new RecordError(
      "bad-punch-range",
      "punch cannot start before beat 0",
    );
  }
  if (!(startBeats < endBeats)) {
    throw new RecordError(
      "bad-punch-range",
      `need start < end, got [${startBeats}, ${endBeats})`,
    );
  }
  return { start_beats: startBeats, end_beats: endBeats };
}

/**
 * Should audio at `beat` be captured? Manual punches whenever the
 * transport records; auto punches only inside its range (end-exclusive,
 * with EPS tolerance so a playhead on the out-point counts as out).
 */
export function isPunching(
  beat: number,
  mode: PunchMode,
  transportRecording: boolean,
): boolean {
  if (!transportRecording) return false;
  if (mode.kind === "manual") return true;
  return (
    beat + EPS >= mode.range.start_beats && beat + EPS < mode.range.end_beats
  );
}

/** Total count-in length in beats: `bars * beatsPerBar`. Throws RecordError. */
export function countInBeats(countIn: CountIn): number {
  if (countIn.bars < 1) {
    throw new RecordError(
      "bad-count-in",
      "count-in needs at least one bar",
    );
  }
  if (countIn.beats_per_bar < 1) {
    throw new RecordError("bad-count-in", "beats per bar must be >= 1");
  }
  return countIn.bars * countIn.beats_per_bar;
}

/** Count-in length in seconds at `tempoBpm`. */
export function countInSeconds(countIn: CountIn, tempoBpm: number): number {
  if (!(tempoBpm > 0) || !Number.isFinite(tempoBpm)) {
    throw new RecordError(
      "bad-tempo",
      `tempo must be a positive finite BPM, got ${tempoBpm}`,
    );
  }
  return (countInBeats(countIn) * 60) / tempoBpm;
}

/**
 * Click schedule: one click per beat at negative offsets up to (excluding)
 * the record downbeat; the first click of each bar is accented. A 1-bar
 * 4/4 count-in clicks at `-4, -3, -2, -1` with the accent on `-4`.
 */
export function countInClicks(countIn: CountIn): CountInClick[] {
  const total = countInBeats(countIn);
  const perBar = countIn.beats_per_bar;
  const clicks: CountInClick[] = [];
  for (let i = 0; i < total; i++) {
    clicks.push({ offset_beats: i - total, accent: i % perBar === 0 });
  }
  return clicks;
}

/**
 * Should the live input be audible? `off` never, `on` whenever armed,
 * `auto` when armed and the transport is not playing back takes.
 */
export function shouldMonitor(
  mode: MonitorMode,
  trackArmed: boolean,
  transport: EngineState,
): boolean {
  switch (mode) {
    case "off":
      return false;
    case "on":
      return trackArmed;
    case "auto":
      return trackArmed && transport !== "Playing";
  }
}

/**
 * Round-trip interface latency in beats: in + out samples converted at
 * `sampleRateHz` and `tempoBpm`.
 */
export function latencyBeats(
  inputLatencySamples: number,
  outputLatencySamples: number,
  sampleRateHz: number,
  tempoBpm: number,
): number {
  if (!(sampleRateHz > 0) || !Number.isFinite(sampleRateHz)) {
    throw new RecordError(
      "bad-latency",
      `sample rate must be positive finite, got ${sampleRateHz}`,
    );
  }
  if (!(tempoBpm > 0) || !Number.isFinite(tempoBpm)) {
    throw new RecordError(
      "bad-tempo",
      `tempo must be a positive finite BPM, got ${tempoBpm}`,
    );
  }
  const total = inputLatencySamples + outputLatencySamples;
  return ((total / sampleRateHz) * tempoBpm) / 60;
}

/** Nudge a captured beat earlier by the measured latency. */
export function compensateCapture(
  capturedBeat: number,
  latencyBeatsValue: number,
): number {
  return capturedBeat - latencyBeatsValue;
}

/**
 * Stack takes into lanes: sorted by `(start_beats, id)` — the same order
 * `takesForRegion` returns — with lane index = position. Throws on empty
 * input: lanes describe recorded passes, and with no passes there is
 * nothing to comp.
 */
export function assignTakeLanes(takes: Clip[]): TakeLane[] {
  if (takes.length === 0) {
    throw new RecordError("no-takes", "no takes to lane");
  }
  const sorted = [...takes].sort(
    (a, b) =>
      a.start_beats - b.start_beats || (a.id < b.id ? -1 : 1),
  );
  return sorted.map((clip, lane) => ({ take_id: clip.id, lane }));
}

/**
 * Build the take clip for one punched pass: spans exactly the punch range
 * with `source = "take:<id>"`, the convention `buildComp` consumes.
 */
export function punchTakeClip(
  id: string,
  trackId: string,
  name: string,
  kind: ClipKind,
  punch: PunchRange,
): Clip {
  return {
    id,
    track_id: trackId,
    name,
    start_beats: punch.start_beats,
    length_beats: punch.end_beats - punch.start_beats,
    kind,
    source: `take:${id}`,
  };
}

/**
 * Captured portion of a manual-punch pass inside a window: `null` when the
 * pass misses the window, otherwise the `[start, end)` overlap in beats.
 */
export function punchOverlap(
  passStartBeats: number,
  passEndBeats: number,
  window: PunchRange,
): [number, number] | null {
  const start = Math.max(passStartBeats, window.start_beats);
  const end = Math.min(passEndBeats, window.end_beats);
  return start + EPS < end ? [start, end] : null;
}
