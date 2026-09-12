import type { AutomationLane, AutomationPoint, Project } from "../generated/project";

/**
 * Track H: automation + modulation model (TypeScript mirror of
 * `core/src/automation.rs`).
 *
 * Two concepts, one address. **Automation** is composed: a frozen
 * `AutomationLane` holds `(beat, value)` points read sample-accurately
 * (each sample maps to its exact beat, linear interpolation, hold outside
 * the range). An `AutomationClip` is the same shape made reusable: points
 * relative to beat 0, stamped onto any lane at any offset. **Modulation**
 * is performed: a `ModMatrix` of `ModRoute`s connects any source (LFO,
 * constant) to any destination through the frozen `node:param` address,
 * added per sample in seconds and clamped to the destination range.
 *
 * Combination rule: `out = clamp(auto(beat) + sum(depth * src(secs)))`.
 * Nothing here changes the frozen v0 schema — lanes stay on
 * `Project.automation`, clips + routes live in the `AutomationDoc`
 * sidecar.
 */

export interface ParamAddress {
  node: string;
  param: string;
}

export interface AutomationClip {
  id: string;
  name: string;
  length_beats: number;
  points: AutomationPoint[];
}

export type LfoShape = "Sine" | "Triangle" | "Saw" | "Square";

export interface Lfo {
  shape: LfoShape;
  freq_hz: number;
  phase: number;
}

export type ModSource = { kind: "Lfo"; lfo: Lfo } | { kind: "Constant"; value: number };

export interface ModRoute {
  id: string;
  source: ModSource;
  target: ParamAddress;
  depth: number;
}

export interface ModMatrix {
  routes: ModRoute[];
}

export interface AutomationDoc {
  clips: AutomationClip[];
  routes: ModRoute[];
}

/** Beats in one sample at `tempo` BPM and `sampleRate` Hz. Throws on bad input. */
export function beatsPerSample(tempo: number, sampleRate: number): number {
  if (!(tempo > 0 && Number.isFinite(tempo))) {
    throw new Error(`tempo ${tempo} must be finite and > 0`);
  }
  if (!(sampleRate > 0 && Number.isFinite(sampleRate))) {
    throw new Error(`sampleRate ${sampleRate} must be finite and > 0`);
  }
  return tempo / 60 / sampleRate;
}

/** Range-check one lane: non-empty id/target, finite strictly-ascending beats. */
export function validateLane(lane: AutomationLane): void {
  if (!lane.id) throw new Error("lane id must be non-empty");
  if (!lane.target.node || !lane.target.param) {
    throw new Error("lane target node:param must be non-empty");
  }
  if (lane.points.length === 0) throw new Error("lane needs at least one point");
  let prev = -Infinity;
  for (const p of lane.points) {
    if (!Number.isFinite(p.beat) || !Number.isFinite(p.value)) {
      throw new Error(`point (${p.beat}, ${p.value}) must be finite`);
    }
    if (p.beat <= prev) {
      throw new Error(`beats must ascend strictly (${p.beat} after ${prev})`);
    }
    prev = p.beat;
  }
}

/**
 * Evaluate one lane at one beat: linear interpolation, hold-first before
 * the first point, hold-last after the last.
 */
export function evalLane(lane: AutomationLane, beat: number): number {
  const pts = lane.points;
  if (pts.length === 0) return 0;
  if (beat <= pts[0].beat) return pts[0].value;
  if (beat >= pts[pts.length - 1].beat) return pts[pts.length - 1].value;
  for (let i = 0; i + 1 < pts.length; i++) {
    const a = pts[i];
    const b = pts[i + 1];
    if (beat >= a.beat && beat <= b.beat) {
      const span = b.beat - a.beat;
      if (span <= 0) return b.value;
      const t = (beat - a.beat) / span;
      return a.value + t * (b.value - a.value);
    }
  }
  return pts[pts.length - 1].value;
}

/** Sample-accurate lane render: one value per sample at its exact beat. */
export function renderLaneSamples(
  lane: AutomationLane,
  startBeat: number,
  tempo: number,
  sampleRate: number,
  frames: number,
): number[] {
  const bps = beatsPerSample(tempo, sampleRate);
  const out: number[] = [];
  for (let i = 0; i < frames; i++) out.push(evalLane(lane, startBeat + i * bps));
  return out;
}

/** Range-check one reusable clip (points inside `0..length_beats`). */
export function validateClip(clip: AutomationClip): void {
  if (!clip.id) throw new Error("clip id must be non-empty");
  if (!(Number.isFinite(clip.length_beats) && clip.length_beats > 0)) {
    throw new Error(`length_beats ${clip.length_beats} must be finite and > 0`);
  }
  if (clip.points.length === 0) throw new Error("clip needs at least one point");
  let prev = -Infinity;
  for (const p of clip.points) {
    if (!Number.isFinite(p.beat) || !Number.isFinite(p.value)) {
      throw new Error(`point (${p.beat}, ${p.value}) must be finite`);
    }
    if (p.beat < 0 || p.beat > clip.length_beats) {
      throw new Error(`point beat ${p.beat} outside clip 0..${clip.length_beats}`);
    }
    if (p.beat <= prev) {
      throw new Error(`beats must ascend strictly (${p.beat} after ${prev})`);
    }
    prev = p.beat;
  }
}

/** Stamp a clip at an absolute offset: relative beats become absolute. */
export function instantiateClip(clip: AutomationClip, startBeat: number): AutomationPoint[] {
  validateClip(clip);
  if (!(Number.isFinite(startBeat) && startBeat >= 0)) {
    throw new Error(`startBeat ${startBeat} must be finite and >= 0`);
  }
  return clip.points.map((p) => ({ beat: startBeat + p.beat, value: p.value }));
}

/** Merge stamped points into a lane: same-beat replaced, order restored. */
export function mergePoints(
  lane: AutomationLane,
  stamped: AutomationPoint[],
): AutomationPoint[] {
  const pts = lane.points.map((p) => ({ ...p }));
  for (const p of stamped) {
    const i = pts.findIndex((q) => q.beat === p.beat);
    if (i >= 0) pts[i] = { ...p };
    else pts.push({ ...p });
  }
  pts.sort((a, b) => a.beat - b.beat);
  return pts;
}

/** Bipolar (-1..+1) LFO value at absolute time `tSecs`. */
export function lfoValueAt(lfo: Lfo, tSecs: number): number {
  const cycles = ((lfo.freq_hz * tSecs + lfo.phase) % 1 + 1) % 1;
  switch (lfo.shape) {
    case "Sine":
      return Math.sin(cycles * Math.PI * 2);
    case "Triangle":
      return 4 * Math.abs(cycles - 0.25 * Math.floor(2 * cycles) - 0.25) - 1;
    case "Saw":
      return 2 * cycles - 1;
    case "Square":
      return cycles < 0.5 ? 1 : -1;
  }
}

export function modSourceValueAt(source: ModSource, tSecs: number): number {
  return source.kind === "Lfo" ? lfoValueAt(source.lfo, tSecs) : source.value;
}

/** Range-check one mod route. */
export function validateRoute(route: ModRoute): void {
  if (!route.id) throw new Error("route id must be non-empty");
  if (!route.target.node || !route.target.param) {
    throw new Error("route target node:param must be non-empty");
  }
  if (!Number.isFinite(route.depth)) throw new Error(`depth ${route.depth} must be finite`);
  if (route.source.kind === "Lfo") {
    const lfo = route.source.lfo;
    if (!(Number.isFinite(lfo.freq_hz) && lfo.freq_hz >= 0)) {
      throw new Error(`lfo freq ${lfo.freq_hz} must be finite and >= 0`);
    }
    if (!Number.isFinite(lfo.phase)) throw new Error("lfo phase must be finite");
  }
}

/** Contribution of one route at absolute time `tSecs`. */
export function routeContributionAt(route: ModRoute, tSecs: number): number {
  return route.depth * modSourceValueAt(route.source, tSecs);
}

/** Summed modulation for one target at one absolute time. */
export function modSumAt(matrix: ModMatrix, target: ParamAddress, tSecs: number): number {
  return matrix.routes
    .filter((r) => r.target.node === target.node && r.target.param === target.param)
    .reduce((acc, r) => acc + routeContributionAt(r, tSecs), 0);
}

export interface ParamBaseAndRange {
  base: number;
  min: number;
  max: number;
}

/**
 * Current value + range of one address, mirroring the engine's `ParamSet`
 * semantics (volume [0, 1.5], pan [-1, 1], muted/solo flags, device min/max).
 */
export function paramBaseAndRange(project: Project, target: ParamAddress): ParamBaseAndRange {
  const track = project.tracks.find((t) => t.id === target.node);
  if (track) {
    switch (target.param) {
      case "volume":
        return { base: track.volume, min: 0, max: 1.5 };
      case "pan":
        return { base: track.pan, min: -1, max: 1 };
      case "muted":
        return { base: track.muted ? 1 : 0, min: 0, max: 1 };
      case "solo":
        return { base: track.solo ? 1 : 0, min: 0, max: 1 };
      default:
        throw new Error(`unknown param \`${target.node}:${target.param}\``);
    }
  }
  const dev = project.devices.find((d) => d.id === target.node);
  const param = dev?.params.find((p) => p.id === target.param);
  if (dev && param) return { base: param.value, min: param.min, max: param.max };
  throw new Error(`unknown param \`${target.node}:${target.param}\``);
}

function clamp(v: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, v));
}

/**
 * Sample-accurate combined render for one destination:
 * `out[i] = clamp(auto(beat_i) + modSum(secs_i))`. No lane = live base
 * value under modulation; no routes = pure automation.
 */
export function resolveParamSamples(
  project: Project,
  target: ParamAddress,
  lane: AutomationLane | null,
  matrix: ModMatrix,
  startBeat: number,
  tempo: number,
  sampleRate: number,
  frames: number,
): number[] {
  const { base, min, max } = paramBaseAndRange(project, target);
  const bps = beatsPerSample(tempo, sampleRate);
  const secsPerBeat = 60 / tempo;
  const out: number[] = [];
  for (let i = 0; i < frames; i++) {
    const beat = startBeat + i * bps;
    const auto = lane ? evalLane(lane, beat) : base;
    out.push(clamp(auto + modSumAt(matrix, target, beat * secsPerBeat), min, max));
  }
  return out;
}

/** Demo sidecar: a 4-beat swell clip plus one sine→volume route. */
export function sampleAutomationDoc(): AutomationDoc {
  return {
    clips: [
      {
        id: "clip_swell",
        name: "Swell",
        length_beats: 4,
        points: [
          { beat: 0, value: 0 },
          { beat: 4, value: 1 },
        ],
      },
    ],
    routes: [
      {
        id: "route_trem",
        source: { kind: "Lfo", lfo: { shape: "Sine", freq_hz: 5, phase: 0 } },
        target: { node: "trk_music", param: "volume" },
        depth: 0.1,
      },
    ],
  };
}
