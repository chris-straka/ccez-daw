import type { Op } from "../generated/project";

/**
 * Automation editing helpers: pure op construction + SVG mapping for
 * `AutomationView`. Op flow mirrors the session jam path — every edit is
 * an `AutomationPointSet` op through `op_apply`, so it is undoable and
 * survives restart like every other edit.
 */

export interface LaneTarget {
  laneId: string;
  node: string;
  param: string;
}

/** Lane id convention: `<node>:<param>` reads as the address it automates. */
export function laneIdFor(node: string, param: string): string {
  return `${node}:${param}`;
}

/**
 * Build the `AutomationPointSet` op for one click: existing lanes carry
 * just beat+value; lane creation also carries the `node:param` address.
 */
export function pointSetOp(
  target: LaneTarget,
  beat: number,
  value: number,
  laneExists: boolean,
): Op {
  if (!Number.isFinite(beat) || beat < 0) {
    throw new Error(`beat ${beat} must be finite and >= 0`);
  }
  if (!Number.isFinite(value)) {
    throw new Error(`value ${value} must be finite`);
  }
  const payload: Record<string, number | string> = { beat, value };
  if (!laneExists) {
    if (!target.node || !target.param) {
      throw new Error("new lanes need a node:param address");
    }
    payload.node = target.node;
    payload.param = target.param;
  }
  return {
    seq: 0,
    actor: "ui",
    kind: "AutomationPointSet",
    target: target.laneId,
    value_json: JSON.stringify(payload),
  };
}

/** Map a click on a lane strip to beats (left = 0, right = `lengthBeats`). */
export function beatAtX(x: number, width: number, lengthBeats: number): number {
  if (!(width > 0) || !(lengthBeats > 0)) {
    throw new Error("width and lengthBeats must be positive");
  }
  const t = Math.min(1, Math.max(0, x / width));
  return Math.round(t * lengthBeats * 100) / 100;
}

/** Map a click height to a value (top = `max`, bottom = `min`). */
export function valueAtY(y: number, height: number, min: number, max: number): number {
  if (!(height > 0) || !(max > min)) {
    throw new Error("height positive and max > min required");
  }
  const t = Math.min(1, Math.max(0, y / height));
  return Math.round((max - t * (max - min)) * 1000) / 1000;
}

/** SVG polyline points for one lane over `lengthBeats` (always endpoints). */
export function lanePath(
  points: Array<{ beat: number; value: number }>,
  width: number,
  height: number,
  lengthBeats: number,
  min: number,
  max: number,
): string {
  const x = (b: number) => (Math.min(lengthBeats, Math.max(0, b)) / lengthBeats) * width;
  const y = (v: number) =>
    height - ((Math.min(max, Math.max(min, v)) - min) / (max - min)) * height;
  const sorted = [...points].sort((a, b) => a.beat - b.beat);
  const ends =
    sorted.length === 0
      ? [
          { beat: 0, value: min },
          { beat: lengthBeats, value: min },
        ]
      : [
          { beat: 0, value: sorted[0].value },
          ...sorted,
          { beat: lengthBeats, value: sorted[sorted.length - 1].value },
        ];
  return ends.map((p) => `${x(p.beat).toFixed(1)},${y(p.value).toFixed(1)}`).join(" ");
}
