import type {
  AdaptiveCue,
  CueLayer,
  GameStateParam,
  GameStateSnapshot,
  RtpcBinding,
  SfxBank,
  TransitionKind,
  TransitionRule,
} from "../generated/project";

/**
 * GA-3: game-state audition simulator model (pure TypeScript, no DOM).
 *
 * The simulator answers "what would the player hear if the game posted
 * *this* snapshot now?" without touching audio hardware: layer audibility
 * mirrors `AdaptiveCue.layers_for_state`, transitions mirror
 * `transition_for` (no rule = Cut), RTPC mirrors the GA-2
 * clamp -> normalize -> linear-map rule, and event firing mirrors the
 * pool / cooldown / polyphony throttle shape with a local seeded RNG.
 * The Solid view (`Audition.tsx`) renders this model onto a hot canvas
 * driven by `requestAnimationFrame`; `bun test` pins the model below.
 */

export interface ResolvedRtpc {
  target_node: string;
  target_param: string;
  value: number;
  min: number;
  max: number;
  normalized: number;
}

export function clamp(v: number, lo: number, hi: number): number {
  const a = Math.min(lo, hi);
  const b = Math.max(lo, hi);
  if (!Number.isFinite(v)) return a;
  return Math.min(b, Math.max(a, v));
}

/** Declared param range normalized to 0-1 (degenerate range reads as 0). */
export function normalizeParam(param: GameStateParam, raw: number): number {
  const lo = Math.min(param.min, param.max);
  const hi = Math.max(param.min, param.max);
  if (!(hi - lo > 0)) return 0;
  return (clamp(raw, lo, hi) - lo) / (hi - lo);
}

/** Live value for one param: snapshot value, or the declared default. */
export function valueFor(param: GameStateParam, snapshot: GameStateSnapshot): number {
  const hit = snapshot.values.find((v) => v.param === param.id);
  return hit ? hit.value : param.default;
}

/**
 * Resolve one trigger's RTPC bindings. Unknown param names are skipped
 * quietly (the game ships params the mix doesn't use yet); declared-but-
 * missing values read as the param default.
 */
export function resolveRtpc(
  bindings: RtpcBinding[],
  snapshot: GameStateSnapshot,
  declared: GameStateParam[],
): ResolvedRtpc[] {
  const out: ResolvedRtpc[] = [];
  for (const b of bindings) {
    const param = declared.find((p) => p.id === b.param);
    if (!param) continue;
    const t = normalizeParam(param, valueFor(param, snapshot));
    const value = b.min + t * (b.max - b.min);
    const span = b.max - b.min;
    out.push({
      target_node: b.target_node,
      target_param: b.target_param,
      value,
      min: b.min,
      max: b.max,
      normalized: Math.abs(span) < Number.EPSILON ? 0 : clamp((value - b.min) / span, 0, 1),
    });
  }
  return out;
}

/** Layers audible in `state` (empty `states` = always-on bed). */
export function layersForState(cue: AdaptiveCue, state: string): CueLayer[] {
  return cue.layers.filter((l) => l.states.length === 0 || l.states.includes(state));
}

/** First rule matching a state change, if any. */
export function transitionFor(
  cue: AdaptiveCue,
  from: string,
  to: string,
): TransitionRule | undefined {
  return cue.transitions.find((t) => t.from_state === from && t.to_state === to);
}

/** Effective transition kind: authored rule, or the contract Cut fallback. */
export function effectiveKind(cue: AdaptiveCue, from: string, to: string): TransitionKind {
  return transitionFor(cue, from, to)?.kind ?? "Cut";
}

/** BarWait/Stinger need the transport slice; the simulator plays a cut. */
export function isDegraded(kind: TransitionKind): boolean {
  return kind === "BarWait" || kind === "Stinger";
}

export function beatsToSeconds(beats: number, tempo: number): number {
  if (!(tempo > 0) || !Number.isFinite(beats)) return 0;
  return (beats * 60) / tempo;
}

export function secondsToBeats(seconds: number, tempo: number): number {
  if (!(tempo > 0) || !Number.isFinite(seconds)) return 0;
  return (seconds * tempo) / 60;
}

/** Whole beats -> whole samples at `tempo` BPM (non-positive beats -> 0). */
export function beatsToSamples(beats: number, tempo: number, sampleRate: number): number {
  if (!(beats > 0) || !(tempo > 0) || !(sampleRate > 0) || !Number.isFinite(sampleRate)) return 0;
  return Math.max(1, Math.round(((beats * 60) / tempo) * sampleRate));
}

/** Linear ramp gain for one layer across `[startBeat, endBeat)`. */
export function rampGainAt(
  from: number,
  to: number,
  startBeat: number,
  endBeat: number,
  atBeat: number,
): number {
  if (atBeat < startBeat) return from;
  if (atBeat >= endBeat || !(endBeat - startBeat > 0)) return to;
  const k = (atBeat - startBeat) / (endBeat - startBeat);
  return from + (to - from) * k;
}

/**
 * Preview gains for every layer mid-transition: outgoing ramps 1->0,
 * incoming 0->1 over `fadeBeats` from `atBeat`; beds hold at 1.
 * `progressBeat` is the audition clock in beats (transport beat).
 */
export function previewGains(
  cue: AdaptiveCue,
  fromState: string,
  toState: string,
  transBeat: number,
  fadeBeats: number,
  progressBeat: number,
): Record<string, number> {
  const from = new Set(layersForState(cue, fromState).map((l) => l.id));
  const to = new Set(layersForState(cue, toState).map((l) => l.id));
  const out: Record<string, number> = {};
  for (const layer of cue.layers) {
    const wasIn = from.has(layer.id);
    const isIn = to.has(layer.id);
    if (wasIn && isIn) out[layer.id] = 1;
    else if (!wasIn && isIn) out[layer.id] = rampGainAt(0, 1, transBeat, transBeat + fadeBeats, progressBeat);
    else if (wasIn && !isIn) out[layer.id] = rampGainAt(1, 0, transBeat, transBeat + fadeBeats, progressBeat);
    else out[layer.id] = 0;
  }
  return out;
}

// -- transport sync --------------------------------------------------------

export interface SimTransport {
  playing: boolean;
  tempo: number;
  /** Audition clock in beats at the cue tempo. */
  beat: number;
}

export function makeTransport(tempo: number): SimTransport {
  return { playing: false, tempo: tempo > 0 ? tempo : 120, beat: 0 };
}

/** Advance the audition clock; paused transport holds its beat. */
export function advanceTransport(t: SimTransport, dtSeconds: number): SimTransport {
  if (!t.playing || !(dtSeconds > 0)) return t;
  return { ...t, beat: t.beat + secondsToBeats(dtSeconds, t.tempo) };
}

export function transportSeconds(t: SimTransport): number {
  return beatsToSeconds(t.beat, t.tempo);
}

// -- state timeline (evaluated, never stored as ops) -----------------------

export interface TimelineEntry {
  id: string;
  /** Beat on the audition clock where this snapshot takes effect. */
  beat: number;
  snapshot: GameStateSnapshot;
}

/** Insert keeping entries sorted by `(beat, id)`; replaces a matching id. */
export function addTimelineEntry(entries: TimelineEntry[], entry: TimelineEntry): TimelineEntry[] {
  const rest = entries.filter((e) => e.id !== entry.id);
  rest.push(entry);
  rest.sort((a, b) => a.beat - b.beat || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  return rest;
}

/** Snapshot in effect at `beat`: latest entry at or before it, else default. */
export function snapshotAt(
  entries: TimelineEntry[],
  defaultSnapshot: GameStateSnapshot,
  beat: number,
): GameStateSnapshot {
  let best: TimelineEntry | null = null;
  for (const e of entries) {
    if (e.beat <= beat && (!best || e.beat >= best.beat)) best = e;
  }
  return best ? best.snapshot : defaultSnapshot;
}

// -- event firing (audition mirror of the GA-2 throttle shape) -------------

export type RejectKind = "UnknownEvent" | "EmptyPool" | "Cooldown" | "PolyphonyBlocked";

export interface FireRecord {
  event_id: string;
  clip_id: string;
  gain: number;
  pitch_semitones: number;
  beat: number;
  started_ms: number;
  rtpc: ResolvedRtpc[];
}

export interface FireRuntime {
  seed: number;
  lastAcceptedMs: Map<string, number>;
  active: FireRecord[];
}

/** Local deterministic RNG (LCG; UI-side stand-in, not cross-language parity). */
function nextRand(runtime: FireRuntime): number {
  runtime.seed = (runtime.seed * 1664525 + 1013904223) >>> 0;
  return runtime.seed / 4294967296;
}

export function makeFireRuntime(seed: number): FireRuntime {
  return { seed: seed >>> 0 || 0x9e3779b9, lastAcceptedMs: new Map(), active: [] };
}

export type FireResult =
  | { ok: true; voice: FireRecord }
  | { ok: false; event_id: string; kind: RejectKind };

export function fireEvent(
  bank: SfxBank,
  runtime: FireRuntime,
  eventId: string,
  snapshot: GameStateSnapshot,
  declared: GameStateParam[],
  nowMs: number,
  beat: number,
  timingSpreadMs = 0,
): FireResult {
  const event = bank.events.find((e) => e.id === eventId);
  if (!event) return { ok: false, event_id: eventId, kind: "UnknownEvent" };
  if (event.clip_ids.length === 0) return { ok: false, event_id: eventId, kind: "EmptyPool" };
  const last = runtime.lastAcceptedMs.get(eventId);
  if (last !== undefined && nowMs - last < event.cooldown_ms) {
    return { ok: false, event_id: eventId, kind: "Cooldown" };
  }
  if (event.max_polyphony === 0) return { ok: false, event_id: eventId, kind: "PolyphonyBlocked" };

  const clip_id = event.clip_ids[Math.floor(nextRand(runtime) * event.clip_ids.length) % event.clip_ids.length];
  const gain = event.volume + (nextRand(runtime) * 2 - 1) * event.volume_random;
  const pitch_semitones = (nextRand(runtime) * 2 - 1) * event.pitch_random;
  void timingSpreadMs;
  const rtpc = resolveRtpc(event.rtpc, snapshot, declared);

  const live = runtime.active.filter((v) => v.event_id === eventId).length;
  if (live >= event.max_polyphony) {
    let oldest: FireRecord | null = null;
    for (const v of runtime.active) {
      if (v.event_id !== eventId) continue;
      if (!oldest || v.started_ms < oldest.started_ms) oldest = v;
    }
    if (oldest) runtime.active.splice(runtime.active.indexOf(oldest), 1);
  }
  const voice: FireRecord = { event_id: eventId, clip_id, gain, pitch_semitones, beat, started_ms: nowMs, rtpc };
  runtime.active.push(voice);
  runtime.lastAcceptedMs.set(eventId, nowMs);
  return { ok: true, voice };
}
