// S-2 loop-seam audition (pure TypeScript, no DOM).
//
// The in-DAW half of the loop-seam gate: gapless loop preview math for any
// cue in the simulator, plus the same boundary-discontinuity click metric
// the ship-gate enforces (`core/src/loopseam.rs` — keep the two definitions
// in sync: step = |last - first| on [-1, 1] floats, default threshold 0.02).
//
// Three jobs:
//
// - `wrapBeatIntoLoop`: fold the audition clock into the loop window so the
//   simulator preview never plays the seam as a gap.
// - `analyzeSeam` / `detectLoopClick`: the click meter over a preview
//   buffer (`LoopAudition` renders this live next to the transport).
// - Waiver helpers: the fix-or-waive file the validator accepts
//   (`loop-waivers.json`: every entry needs a non-empty reason).

/** Default click threshold in full-scale float units (mirrors Rust). */
export const DEFAULT_CLICK_THRESHOLD = 0.02;

/** Waiver filename convention the validator error text suggests. */
export const WAIVER_FILENAME = "loop-waivers.json";

/** One stem's seam report. */
export interface SeamReport {
  /** `|last - first|` on `[-1, 1]` floats. */
  step: number;
  /** Peak `|x|` over the buffer (0 when empty). */
  peak: number;
  /** The threshold this report was judged against. */
  threshold: number;
  /** `step > threshold`. */
  click: boolean;
}

/** Boundary discontinuity of one loop buffer. Empty reads as 0. */
export function seamStep(samples: ArrayLike<number>): number {
  const n = samples.length;
  if (n === 0) return 0;
  return Math.abs(samples[n - 1] - samples[0]);
}

/** Peak absolute amplitude; context for judging a step. */
export function peakOf(samples: ArrayLike<number>): number {
  let m = 0;
  for (let i = 0; i < samples.length; i++) m = Math.max(m, Math.abs(samples[i]));
  return m;
}

/** Full seam report for one preview buffer. */
export function analyzeSeam(samples: ArrayLike<number>, threshold = DEFAULT_CLICK_THRESHOLD): SeamReport {
  const step = seamStep(samples);
  const peak = peakOf(samples);
  return { step, peak, threshold, click: step > threshold };
}

/** True when the buffer would fail the ship-gate at `threshold`. */
export function detectLoopClick(samples: ArrayLike<number>, threshold = DEFAULT_CLICK_THRESHOLD): boolean {
  return analyzeSeam(samples, threshold).click;
}

/**
 * Fold an audition-clock beat into `[start, end)` so a loop preview wraps
 * gaplessly instead of running past the end and replaying the attack as a
 * gap. Degenerate windows (`end <= start`, non-finite) hold at `start`.
 */
export function wrapBeatIntoLoop(beat: number, start: number, end: number): number {
  if (!Number.isFinite(beat) || !Number.isFinite(start) || !Number.isFinite(end)) return start;
  const len = end - start;
  if (!(len > 0)) return start;
  const k = (beat - start) % len;
  return start + (k < 0 ? k + len : k);
}

/** A manifest loop window is shippable when `0 <= start < end` and finite. */
export function isLoopWindowValid(start: number, end: number): boolean {
  return Number.isFinite(start) && Number.isFinite(end) && start >= 0 && start < end;
}

/**
 * Deterministic preview tone: whole `cycles` across the buffer (the in-DAW
 * stand-in for the export renderer's whole-beat construction). Note whole
 * cycles do NOT close the seam by themselves — the endpoint still misses by
 * one sample of slope, which is exactly the click an unfaded cut ships
 * (see `applyEdgeFade` for the fix). Any cue can audition gaplessly
 * through this.
 */
export function synthesizeLoopTone(length: number, cycles: number, gain = 0.5): Float32Array {
  const n = Math.max(0, Math.floor(length));
  const out = new Float32Array(n);
  if (n === 0) return out;
  for (let t = 0; t < n; t++) {
    out[t] = gain * Math.sin((2 * Math.PI * cycles * t) / n);
  }
  return out;
}

/**
 * The one-click fix: raised-cosine fade over the first/last `fadeSamples`
 * pulls both edges to zero so the seam meets. Mirrors the export
 * renderer's 5 ms edge fade.
 */
export function applyEdgeFade(samples: Float32Array, fadeSamples: number): Float32Array {
  const n = samples.length;
  const out = Float32Array.from(samples);
  if (n <= 1) return out;
  const f = Math.max(1, Math.min(Math.floor(fadeSamples), Math.floor(n / 2)));
  const smooth = (x: number) => x * x * (3 - 2 * x);
  for (let t = 0; t < f; t++) {
    const g = smooth(t / f);
    out[t] *= g;
    out[n - 1 - t] *= g;
  }
  return out;
}

/**
 * Seeded click injection for tests: noise over the tail samples plus a
 * pinned final sample exactly `height` above the first, deterministic per
 * seed. The pin guarantees the seam step reads `height` no matter what the
 * noise draws.
 */
export function injectClick(samples: Float32Array, seed: number, len: number, height: number): Float32Array {
  const out = Float32Array.from(samples);
  let state = seed >>> 0 || 0x9e3779b9;
  const next = () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
  const start = Math.max(0, out.length - Math.max(0, Math.floor(len)));
  for (let i = start; i < out.length; i++) out[i] += (next() * 2 - 1) * height;
  if (out.length > 0) out[out.length - 1] = out[0] + height;
  return out;
}

/** One waiver entry: which stem, and why a human excused it. */
export interface LoopWaiver {
  path: string;
  reason: string;
}

/** Parse a `loop-waivers.json` body; throws on any shape violation. */
export function parseWaiverFile(json: string): LoopWaiver[] {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch (e) {
    throw new Error(`invalid waiver file: ${e instanceof Error ? e.message : e}`);
  }
  if (typeof value !== "object" || value === null || !Array.isArray((value as { waivers?: unknown }).waivers)) {
    throw new Error('invalid waiver file: want { "waivers": [{ "path", "reason" }] }');
  }
  const list = (value as { waivers: unknown[] }).waivers;
  const out: LoopWaiver[] = [];
  const seen = new Set<string>();
  for (const entry of list) {
    const rec = entry as { path?: unknown; reason?: unknown };
    if (typeof rec.path !== "string" || rec.path.length === 0) {
      throw new Error("invalid waiver file: every entry needs a non-empty string \"path\"");
    }
    if (typeof rec.reason !== "string" || rec.reason.trim().length === 0) {
      throw new Error(`invalid waiver file: '${rec.path}' needs a non-empty reason`);
    }
    if (seen.has(rec.path)) throw new Error(`invalid waiver file: '${rec.path}' waived twice`);
    seen.add(rec.path);
    out.push({ path: rec.path, reason: rec.reason });
  }
  return out;
}

/** Serialize waivers to the file format (what the "waive" button writes). */
export function waiverFileJson(waivers: LoopWaiver[]): string {
  return JSON.stringify({ waivers }, null, 2);
}

/** Fix-or-waive guidance for one clicking stem (mirrors the Rust text). */
export function clickErrorText(path: string, report: SeamReport): string {
  return (
    `stem '${path}' clicks at the loop seam: boundary step ${report.step.toFixed(4)} > ` +
    `threshold ${report.threshold.toFixed(4)} (peak ${report.peak.toFixed(3)}). ` +
    `Fix: re-export with loop-clean edges (whole-beat loop at the cue tempo with edge fades), ` +
    `or waive with a reason in ${WAIVER_FILENAME}`
  );
}
