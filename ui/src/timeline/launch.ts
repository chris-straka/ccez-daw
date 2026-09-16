import type { Clip, Op, OpKind, Project } from "../generated/project";
import { OpSchema } from "../generated/project";
import {
  clipEnd,
  launcherSlots,
  sectionEnd,
  type TimelineDoc,
} from "./model";

/**
 * Clip-slot / scene launch model (TS mirror of `core/src/launcher.rs`).
 *
 * The slot grid (`launcherSlots`) is a view; this module adds the
 * performance side: presses quantize to the next grid boundary, and a jam
 * (a sequence of scene launches) records back to the arrangement as frozen
 * `ClipAdded` ops with `seq: 0` placeholders the engine replaces.
 */

export type LaunchQuant =
  | { kind: "none" }
  | { kind: "beat" }
  | { kind: "bar" }
  | { kind: "scene" }
  | { kind: "beats"; grid: number };

export interface SlotRef {
  section_id: string;
  track_id: string;
}

export interface LaunchPlan {
  section_id: string;
  track_id: string | null;
  requested_beat: number;
  launch_beat: number;
  clip_ids: string[];
}

export interface JamEvent {
  section_id: string;
  launch_beat: number;
}

function checkBeat(beat: number, what: string): void {
  if (!Number.isFinite(beat) || beat < 0) {
    throw new Error(`${what} beat ${beat} must be finite and >= 0`);
  }
}

/** Next grid boundary at or after `requested` (already-on-grid presses hold). */
export function quantizeBeat(requested: number, grid: number): number {
  checkBeat(requested, "requested");
  if (!Number.isFinite(grid) || grid <= 0) {
    throw new Error(`quant grid ${grid} must be finite and > 0`);
  }
  const ratio = requested / grid;
  if (Math.ceil(ratio) - ratio < 1e-9) return requested;
  const snapped = Math.ceil(ratio) * grid;
  const rounded = Math.round(snapped / grid) * grid;
  return Math.abs(rounded - snapped) < 1e-9 ? rounded : snapped;
}

/** Scene quantization: the section start when pressed early, else the next phrase. */
export function quantizeLaunchForScene(
  requested: number,
  sectionStart: number,
  sectionLen: number,
): number {
  checkBeat(requested, "requested");
  if (!Number.isFinite(sectionStart) || sectionStart < 0) {
    throw new Error(`section start ${sectionStart} must be finite and >= 0`);
  }
  if (!Number.isFinite(sectionLen) || sectionLen <= 0) {
    throw new Error(`section length ${sectionLen} must be finite and > 0`);
  }
  if (requested <= sectionStart) return sectionStart;
  return sectionStart + Math.ceil((requested - sectionStart) / sectionLen) * sectionLen;
}

function quantGrid(quant: LaunchQuant): number | null {
  switch (quant.kind) {
    case "none":
      return 0;
    case "beat":
      return 1;
    case "bar":
      return 4;
    case "scene":
      return null;
    case "beats":
      return quant.grid;
  }
}

/** Resolve the quantized launch beat for a press. */
export function launchBeatFor(
  requested: number,
  quant: LaunchQuant,
  doc: TimelineDoc,
  sectionId: string,
): number {
  checkBeat(requested, "requested");
  if (quant.kind === "scene") {
    const section = doc.sections.find((s) => s.id === sectionId);
    if (!section) throw new Error(`unknown section \`${sectionId}\``);
    return quantizeLaunchForScene(requested, section.start_beats, section.length_beats);
  }
  if (quant.kind === "none") return requested;
  const grid = quantGrid(quant)!;
  return quantizeBeat(requested, grid);
}

/** Plan launching one slot cell. Throws on unknown section/slot. */
export function planSlotLaunch(
  project: Project,
  doc: TimelineDoc,
  slot: SlotRef,
  requestedBeat: number,
  quant: LaunchQuant,
): LaunchPlan {
  const section = doc.sections.find((s) => s.id === slot.section_id);
  if (!section) throw new Error(`unknown section \`${slot.section_id}\``);
  if (!project.tracks.some((t) => t.id === slot.track_id)) {
    throw new Error(`unknown slot \`${slot.section_id}:${slot.track_id}\``);
  }
  const end = sectionEnd(section);
  const clip_ids = project.clips
    .filter(
      (c) => c.track_id === slot.track_id && c.start_beats < end && clipEnd(c) > section.start_beats,
    )
    .slice()
    .sort((a, b) => a.start_beats - b.start_beats || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
    .map((c) => c.id);
  return {
    section_id: slot.section_id,
    track_id: slot.track_id,
    requested_beat: requestedBeat,
    launch_beat: launchBeatFor(requestedBeat, quant, doc, slot.section_id),
    clip_ids,
  };
}

/** Plan launching a whole scene (section row across all tracks). */
export function planSceneLaunch(
  project: Project,
  doc: TimelineDoc,
  sectionId: string,
  requestedBeat: number,
  quant: LaunchQuant,
): LaunchPlan {
  if (!doc.sections.some((s) => s.id === sectionId)) {
    throw new Error(`unknown section \`${sectionId}\``);
  }
  const clip_ids = [
    ...new Set(
      launcherSlots(project, doc)
        .filter((s) => s.section_id === sectionId)
        .flatMap((s) => s.clip_ids),
    ),
  ].sort();
  return {
    section_id: sectionId,
    track_id: null,
    requested_beat: requestedBeat,
    launch_beat: launchBeatFor(requestedBeat, quant, doc, sectionId),
    clip_ids,
  };
}

/** Validate a jam: non-empty, known sections, non-decreasing launch beats. */
export function validateJam(doc: TimelineDoc, events: JamEvent[]): void {
  if (events.length === 0) throw new Error("jam has no events");
  let prev = -Infinity;
  events.forEach((ev, i) => {
    if (!doc.sections.some((s) => s.id === ev.section_id)) {
      throw new Error(`unknown section \`${ev.section_id}\``);
    }
    if (!Number.isFinite(ev.launch_beat) || ev.launch_beat < 0) {
      throw new Error(`event ${i} launch beat ${ev.launch_beat} must be finite and >= 0`);
    }
    if (ev.launch_beat < prev) {
      throw new Error(`event ${i} launch beat ${ev.launch_beat} is before event ${i - 1} (${prev})`);
    }
    prev = ev.launch_beat;
  });
}

/**
 * Record a jam into the arrangement: each event copies the launched
 * section's clips to `launch_beat + (clip.start - section.start)` as frozen
 * `ClipAdded` ops. New ids are `{clip_id}__jam{i}`; collisions throw.
 */
export function jamRecordOps(
  actor: string,
  project: Project,
  doc: TimelineDoc,
  events: JamEvent[],
): Op[] {
  validateJam(doc, events);
  const ops: Op[] = [];
  const taken = new Set(project.clips.map((c) => c.id));
  events.forEach((ev, i) => {
    const section = doc.sections.find((s) => s.id === ev.section_id)!;
    const end = sectionEnd(section);
    const member: Clip[] = project.clips
      .filter((c) => c.start_beats < end && clipEnd(c) > section.start_beats)
      .slice()
      .sort((a, b) => a.start_beats - b.start_beats || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
    for (const clip of member) {
      const newId = `${clip.id}__jam${i}`;
      if (taken.has(newId)) throw new Error(`clip id \`${newId}\` already exists`);
      taken.add(newId);
      ops.push(
        OpSchema.parse({
          seq: 0,
          actor,
          kind: "ClipAdded" as OpKind,
          target: clip.track_id,
          value_json: JSON.stringify({
            ...clip,
            id: newId,
            name: `${clip.name} (jam ${i})`,
            start_beats: ev.launch_beat + (clip.start_beats - section.start_beats),
          }),
        }),
      );
    }
  });
  return ops;
}
