import { z } from "zod";
import type { Clip, Op, OpKind } from "../generated/project";
import { OpSchema } from "../generated/project";
import type { MidiClip } from "../pianoroll/model";
import { MidiClipSchema, sortNotes } from "../pianoroll/model";

/**
 * Groove pool: extract timing/velocity feel from any MIDI clip into a named
 * library, then preview + apply it to another clip.
 *
 * Exact TypeScript mirror of `core/src/midi/groove.rs`. The two
 * implementations must agree on: nearest-step bucketing (including the
 * note's own feel lane in the learned offset), neutral empty steps, the
 * ±0.25 microtiming lane, step lookup from `start_beats` only, and the
 * quantize → timing → velocity order of `applyGroove`. `ui/tests/groove.test.ts`
 * pins the same extract/apply behavior as the Rust groove tests, so drift
 * on either side shows up as red.
 *
 * Nothing here touches the frozen v0 schema or adds IPC: applying a groove
 * produces a new `MidiClip` the host reports through `onEdit` (like the
 * repair panel), and the timeline commit is an ordinary frozen `ClipAdded`
 * op (`grooveCommitOp`) whose opaque `source` names the pool entry — it
 * undoes/redoes like any op via `op_undo`.
 */

export const GrooveTemplateSchema = z.object({
  steps_per_beat: z.number().int().min(1),
  offsets: z.array(z.number().finite()),
  vel_scale: z.array(z.number().finite()),
});
export type GrooveTemplate = z.infer<typeof GrooveTemplateSchema>;

export const ApplyParamsSchema = z.object({
  quantize: z.number().min(0).max(1),
  timing: z.number().min(0).max(1),
  velocity: z.number().min(0).max(1),
});
export type ApplyParams = z.infer<typeof ApplyParamsSchema>;

/** Flat template: no swing, no accent (identity transfer). */
export function flatGroove(stepsPerBeat: number): GrooveTemplate {
  validateSteps(stepsPerBeat);
  const n = stepsPerBeat;
  return {
    steps_per_beat: stepsPerBeat,
    offsets: new Array<number>(n).fill(0),
    vel_scale: new Array<number>(n).fill(1),
  };
}

function validateSteps(stepsPerBeat: number): void {
  if (!Number.isInteger(stepsPerBeat) || stepsPerBeat < 1) {
    throw new Error(`steps_per_beat must be an integer >= 1, got ${stepsPerBeat}`);
  }
}

/** Throw unless a template is internally consistent (mirrors Rust). */
export function validateTemplate(template: GrooveTemplate): void {
  const parsed = GrooveTemplateSchema.parse(template);
  if (
    parsed.offsets.length !== parsed.steps_per_beat ||
    parsed.vel_scale.length !== parsed.steps_per_beat
  ) {
    throw new Error("template offsets/vel_scale length mismatch");
  }
}

/** Throw unless every depth is a finite 0..=1 blend (mirrors Rust). */
export function validateParams(params: ApplyParams): void {
  ApplyParamsSchema.parse(params);
  for (const [name, v] of Object.entries(params) as [string, number][]) {
    if (!Number.isFinite(v)) throw new Error(`${name} ${v} out of range 0..=1`);
  }
}

function stepOf(startBeats: number, stepsPerBeat: number): number {
  const gridPos = Math.round(startBeats * stepsPerBeat);
  return ((gridPos % stepsPerBeat) + stepsPerBeat) % stepsPerBeat;
}

/**
 * Learn the feel of `clip`: bucket each unmuted note to its nearest grid
 * step and average timing offsets and velocity ratios per step. Empty steps
 * stay neutral (offset 0, scale 1); learned offsets clamp to the ±0.25 lane.
 */
export function extractGroove(clip: MidiClip, stepsPerBeat: number): GrooveTemplate {
  MidiClipSchema.parse(clip);
  validateSteps(stepsPerBeat);
  const n = stepsPerBeat;
  const offsets = new Array<number>(n).fill(0);
  const velScale = new Array<number>(n).fill(0);
  const counts = new Array<number>(n).fill(0);
  for (const note of clip.notes) {
    if (note.muted) continue;
    const step = stepOf(note.start_beats, n);
    const grid = Math.round(note.start_beats * n) / n;
    offsets[step] += note.start_beats - grid + note.timing_offset_beats;
    velScale[step] += note.velocity / 100;
    counts[step] += 1;
  }
  for (let i = 0; i < n; i++) {
    if (counts[i] > 0) {
      offsets[i] /= counts[i];
      velScale[i] /= counts[i];
    } else {
      offsets[i] = 0;
      velScale[i] = 1;
    }
    offsets[i] = Math.min(0.25, Math.max(-0.25, offsets[i]));
  }
  return validateTemplateReturn({ steps_per_beat: n, offsets, vel_scale: velScale });
}

function validateTemplateReturn(t: GrooveTemplate): GrooveTemplate {
  validateTemplate(t);
  return t;
}

/**
 * Blend `template` into `clip` with separate depths. Order per unmuted
 * note: (1) `quantize` pulls the onset toward the template grid inside the
 * ±0.25 feel lane — grid positions never move, only feel; (2) `timing` adds
 * the template offset; (3) `velocity` blends toward `velocity * vel_scale`.
 * Muted notes are untouched. Returns a new clip (immutable, like the
 * piano-roll store ops).
 */
export function applyGroove(
  clip: MidiClip,
  template: GrooveTemplate,
  params: ApplyParams,
): MidiClip {
  MidiClipSchema.parse(clip);
  validateTemplate(template);
  validateParams(params);
  const spb = template.steps_per_beat;
  const notes = clip.notes.map((note) => {
    if (note.muted) return note;
    const step = stepOf(note.start_beats, spb);
    let offset = note.timing_offset_beats;
    if (params.quantize > 0) {
      const onset = note.start_beats + offset;
      const snapped = Math.round(onset * spb) / spb;
      offset = Math.min(0.25, Math.max(-0.25, offset + (snapped - onset) * params.quantize));
    }
    offset = Math.min(0.25, Math.max(-0.25, offset + template.offsets[step] * params.timing));
    const target = note.velocity * template.vel_scale[step];
    const blended = note.velocity + (target - note.velocity) * params.velocity;
    return {
      ...note,
      timing_offset_beats: offset,
      velocity: Math.min(127, Math.max(1, Math.round(blended))),
    };
  });
  const out: MidiClip = { ...clip, notes: sortNotes(notes) };
  MidiClipSchema.parse(out);
  return out;
}

/** Single-knob blend: no quantize, timing and velocity move together. */
export function applyGrooveAmount(
  clip: MidiClip,
  template: GrooveTemplate,
  amount: number,
): MidiClip {
  return applyGroove(clip, template, { quantize: 0, timing: amount, velocity: amount });
}

/** Per-note deltas between a clip and its grooved preview (for readouts). */
export interface GroovePreview {
  clip: MidiClip;
  moved: number;
  meanAbsTimingDelta: number;
  meanAbsVelocityDelta: number;
}

/** Apply without committing: the panel shows the deltas, the clip stays. */
export function previewGroove(
  clip: MidiClip,
  template: GrooveTemplate,
  params: ApplyParams,
): GroovePreview {
  const out = applyGroove(clip, template, params);
  let moved = 0;
  let timingSum = 0;
  let velSum = 0;
  const before = new Map(clip.notes.map((n) => [n.note_id, n]));
  for (const n of out.notes) {
    const prev = before.get(n.note_id);
    if (!prev || prev.muted) continue;
    const dt = Math.abs(n.timing_offset_beats - prev.timing_offset_beats);
    const dv = Math.abs(n.velocity - prev.velocity);
    if (dt > 1e-12 || dv > 0) moved += 1;
    timingSum += dt;
    velSum += dv;
  }
  const count = Math.max(1, out.notes.filter((n) => !n.muted).length);
  return {
    clip: out,
    moved,
    meanAbsTimingDelta: timingSum / count,
    meanAbsVelocityDelta: velSum / count,
  };
}

/**
 * Named pool of groove templates: the user-facing groove library.
 * Insert replaces under the same (trimmed) name; JSON round-trips so a
 * host can persist it beside the project without touching the schema.
 */
export class GroovePool {
  private entries = new Map<string, GrooveTemplate>();

  /** File `template` under `name` (replaces any entry already there). */
  set(name: string, template: GrooveTemplate): void {
    const clean = name.trim();
    if (!clean) throw new Error("groove name must be non-empty");
    validateTemplate(template);
    this.entries.set(clean, structuredClone(template));
  }

  get(name: string): GrooveTemplate | undefined {
    const found = this.entries.get(name.trim());
    return found ? structuredClone(found) : undefined;
  }

  /** Remove by name. Returns `true` when something was removed. */
  delete(name: string): boolean {
    return this.entries.delete(name.trim());
  }

  /** Entry names in sort order. */
  names(): string[] {
    return [...this.entries.keys()].sort();
  }

  get size(): number {
    return this.entries.size;
  }

  toJSON(): string {
    return JSON.stringify(Object.fromEntries(this.entries));
  }

  static fromJSON(json: string): GroovePool {
    let raw: unknown;
    try {
      raw = JSON.parse(json);
    } catch {
      throw new Error("groove pool decode: invalid JSON");
    }
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
      throw new Error("groove pool decode: expected an object");
    }
    const pool = new GroovePool();
    for (const [name, template] of Object.entries(raw as Record<string, unknown>)) {
      pool.set(name, GrooveTemplateSchema.parse(template));
    }
    return pool;
  }
}

/** Keep op/commit ids filesystem/log safe (mirrors script-name rules). */
export function sanitizeGrooveName(name: string): string {
  const clean = name.trim().replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64);
  return clean || "groove";
}

/**
 * Opaque asset source for a grooved clip: `groove:<name>+<source>`, in the
 * same family as `comp:<takes>` and `take:<id>`. The encoded grooved
 * `MidiClip` bytes travel with the panel's `onEdit`; the op stays ordinary.
 */
export function groovedSource(targetSource: string, grooveName: string): string {
  return `groove:${sanitizeGrooveName(grooveName)}+${targetSource}`;
}

/**
 * Commit a grooved clip as one frozen `ClipAdded` op: `seq: 0` is a
 * placeholder the engine replaces; `target` names the new clip and
 * `value_json` carries the full clip. Later nudges are frozen `ClipMoved`
 * ops and the commit itself undoes/redoes like any op — no new IPC.
 */
export function grooveCommitOp(actor: string, target: Clip, grooveName: string): Op {
  if (!target || typeof target.track_id !== "string" || !target.track_id) {
    throw new Error("clip.track_id must be a non-empty string");
  }
  const clean = sanitizeGrooveName(grooveName);
  const clip: Clip = {
    ...target,
    id: `${target.id}~groove-${clean}`,
    name: `${target.name} (groove ${grooveName.trim() || clean})`,
    source: groovedSource(target.source, grooveName),
  };
  return OpSchema.parse({
    seq: 0,
    actor,
    kind: "ClipAdded" as OpKind,
    target: clip.id,
    value_json: JSON.stringify(clip),
  });
}
