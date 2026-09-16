import type { Clip, Op, OpKind } from "../generated/project";
import { OpSchema } from "../generated/project";

/** Actor prefix every script op carries (see contracts/op-log-format.md). */
export const SCRIPT_ACTOR_PREFIX = "script:";

/** Keep script names filesystem/log safe: letters, digits, `_`, `-`. */
export function sanitizeScriptName(name: string): string {
  const clean = name.trim().replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64);
  return clean || "unnamed";
}

export function scriptActor(name: string): string {
  return `${SCRIPT_ACTOR_PREFIX}${sanitizeScriptName(name)}`;
}

/** Minimal `op_apply` shape. Scripts apply ops one at a time; each call */
/** returns the op's sequence number (undoable by design via `op_undo`). */
export type OpRunner = (op: Op) => Promise<number>;

export interface CompiledScript {
  name: string;
  actor: string;
  ops: Op[];
}

function makeOp(
  actor: string,
  kind: OpKind,
  target: string,
  payload: unknown,
): Op {
  if (!target || typeof target !== "string") {
    throw new Error(`op target must be a non-empty string (kind ${kind})`);
  }
  const value_json = JSON.stringify(payload);
  // seq is assigned by `op_apply` in Rust; scripts ship seq 0 as placeholder.
  return OpSchema.parse({ seq: 0, actor, kind, target, value_json });
}

function requireFiniteNumber(value: number, what: string): void {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${what} must be a finite number, got ${String(value)}`);
  }
}

/** Build a `TrackAdded` op whose payload is the new track's name. */
export function trackAddedOp(actor: string, trackName: string): Op {
  if (!trackName || typeof trackName !== "string") {
    throw new Error("track name must be a non-empty string");
  }
  return makeOp(actor, "TrackAdded", trackName, trackName);
}

/** Build a `ClipAdded` op carrying the full Clip payload. */
export function clipAddedOp(actor: string, clip: Clip): Op {
  if (!clip || typeof clip.track_id !== "string" || !clip.track_id) {
    throw new Error("clip.track_id must be a non-empty string");
  }
  requireFiniteNumber(clip.start_beats, "clip.start_beats");
  requireFiniteNumber(clip.length_beats, "clip.length_beats");
  if (clip.length_beats <= 0) throw new Error("clip.length_beats must be > 0");
  return makeOp(actor, "ClipAdded", clip.id, clip);
}

/** Build a `ClipMoved` op carrying the new `{ startBeats }` payload. */
export function clipMovedOp(actor: string, clipId: string, startBeats: number): Op {
  requireFiniteNumber(startBeats, "startBeats");
  return makeOp(actor, "ClipMoved", clipId, { startBeats });
}

/** Build a `ParamSet` op for a `node:param` address. */
export function paramSetOp(actor: string, node: string, param: string, value: number): Op {
  if (!node || !param) throw new Error("param address needs node and param names");
  requireFiniteNumber(value, "param value");
  return makeOp(actor, "ParamSet", `${node}:${param}`, value);
}

/**
 * Build an `AutomationPointSet` op (the one additive `OpKind` past the v0
 * freeze): upsert one `(beat, value)` point on `laneId`. Lane creation
 * (`laneExists: false`) also carries the `node:param` address, per
 * `contracts/op-log-format.md`; updates carry just beat+value.
 */
export function automationPointSetOp(
  actor: string,
  laneId: string,
  beat: number,
  value: number,
  node?: string,
  param?: string,
  laneExists = false,
): Op {
  requireFiniteNumber(beat, "beat");
  if (beat < 0) throw new Error(`beat must be >= 0, got ${beat}`);
  requireFiniteNumber(value, "automation value");
  const payload: Record<string, number | string> = { beat, value };
  if (!laneExists) {
    if (!node || !param) throw new Error("new lanes need a node:param address");
    payload.node = node;
    payload.param = param;
  } else if (node && param) {
    payload.node = node;
    payload.param = param;
  }
  return makeOp(actor, "AutomationPointSet", laneId, payload);
}

/** Build a `TempoSet` op (BPM, must be positive and finite). */
export function tempoSetOp(actor: string, tempo: number): Op {
  requireFiniteNumber(tempo, "tempo");
  if (tempo <= 0) throw new Error(`tempo must be > 0, got ${tempo}`);
  return makeOp(actor, "TempoSet", "transport", tempo);
}
