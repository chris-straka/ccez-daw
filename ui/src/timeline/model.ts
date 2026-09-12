import type { Clip, Op, OpKind, Project } from "../generated/project";
import { OpSchema } from "../generated/project";

/**
 * Track E: timeline + clips model (TypeScript mirror of `core/src/timeline.rs`).
 *
 * One model, two views: the **linear** view lays clips on a beat ruler and
 * the **launcher** view groups the same clips into `(section, track)` slots.
 * Sections are objects (named beat ranges), arrangements are orders over
 * shared sections, and per-clip sound lives in a `ClipProps` sidecar keyed
 * by clip id (gain/pitch/time/fades/FX chain/routing target). Nothing here
 * changes the frozen v0 schema — all clip motion is emitted as frozen
 * `ClipMoved` ops (`{"startBeats": n}`) with `seq: 0` placeholders the
 * engine replaces.
 */

export interface Section {
  id: string;
  name: string;
  start_beats: number;
  length_beats: number;
}

export interface Arrangement {
  id: string;
  name: string;
  section_ids: string[];
}

export interface ClipProps {
  clip_id: string;
  gain_db: number;
  pitch_semitones: number;
  time_ratio: number;
  fade_in_beats: number;
  fade_out_beats: number;
  fx: string[];
  out: string | null;
}

export interface TimelineDoc {
  sections: Section[];
  arrangements: Arrangement[];
  clip_props: ClipProps[];
}

export interface LauncherSlot {
  section_id: string;
  track_id: string;
  clip_ids: string[];
}

export interface ArrangedSection {
  section_id: string;
  name: string;
  offset_beats: number;
  length_beats: number;
}

/** Flat (unity) props: audible no-op, routing untouched. */
export function flatClipProps(clipId: string): ClipProps {
  return {
    clip_id: clipId,
    gain_db: 0,
    pitch_semitones: 0,
    time_ratio: 1,
    fade_in_beats: 0,
    fade_out_beats: 0,
    fx: [],
    out: null,
  };
}

/** Range-check one prop set. Throws; the renderer never reinterprets. */
export function validateClipProps(props: ClipProps): void {
  if (!props.clip_id) throw new Error("clip_id must be non-empty");
  if (!(props.gain_db >= -60 && props.gain_db <= 12)) {
    throw new Error(`gain_db ${props.gain_db} out of [-60, +12]`);
  }
  if (!(props.pitch_semitones >= -24 && props.pitch_semitones <= 24)) {
    throw new Error(`pitch_semitones ${props.pitch_semitones} out of [-24, +24]`);
  }
  if (!(props.time_ratio > 0 && Number.isFinite(props.time_ratio))) {
    throw new Error(`time_ratio ${props.time_ratio} must be finite and > 0`);
  }
  if (
    !(props.fade_in_beats >= 0 && Number.isFinite(props.fade_in_beats)) ||
    !(props.fade_out_beats >= 0 && Number.isFinite(props.fade_out_beats))
  ) {
    throw new Error("fades must be finite and >= 0");
  }
}

export function propsFor(doc: TimelineDoc, clipId: string): ClipProps {
  return doc.clip_props.find((p) => p.clip_id === clipId) ?? flatClipProps(clipId);
}

export function clipEnd(clip: Pick<Clip, "start_beats" | "length_beats">): number {
  return clip.start_beats + clip.length_beats;
}

export function sectionEnd(section: Pick<Section, "start_beats" | "length_beats">): number {
  return section.start_beats + section.length_beats;
}

export function beatsToSeconds(beats: number, tempo: number): number {
  return (beats * 60) / tempo;
}

export function secondsToBeats(seconds: number, tempo: number): number {
  return (seconds * tempo) / 60;
}

function byStartThenId(
  a: Pick<Clip, "id" | "start_beats">,
  b: Pick<Clip, "id" | "start_beats">,
): number {
  return a.start_beats - b.start_beats || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}

/** Linear view: clips on one track, sorted by start (then id). */
export function clipsSortedOnTrack(project: Project, trackId: string): Clip[] {
  return project.clips
    .filter((c) => c.track_id === trackId)
    .slice()
    .sort(byStartThenId);
}

/** Overlapping clip id-pairs on one track (half-open; touching edges ok). */
export function findOverlaps(project: Project, trackId: string): Array<[string, string]> {
  const clips = clipsSortedOnTrack(project, trackId);
  const out: Array<[string, string]> = [];
  for (let i = 0; i < clips.length; i++) {
    for (let j = i + 1; j < clips.length; j++) {
      if (clips[j].start_beats >= clipEnd(clips[i])) break;
      out.push([clips[i].id, clips[j].id]);
    }
  }
  return out;
}

/** Clips overlapping a section's beat range, sorted by start. */
export function sectionClips(project: Project, section: Section): Clip[] {
  const end = sectionEnd(section);
  return project.clips
    .filter((c) => c.start_beats < end && clipEnd(c) > section.start_beats)
    .slice()
    .sort(byStartThenId);
}

/** Launcher view: one slot per `(section, track)` with overlapping clip ids. */
export function launcherSlots(project: Project, doc: TimelineDoc): LauncherSlot[] {
  const slots: LauncherSlot[] = [];
  for (const section of doc.sections) {
    const end = sectionEnd(section);
    for (const track of project.tracks) {
      const ids = project.clips
        .filter(
          (c) =>
            c.track_id === track.id &&
            c.start_beats < end &&
            clipEnd(c) > section.start_beats,
        )
        .slice()
        .sort(byStartThenId)
        .map((c) => c.id);
      slots.push({ section_id: section.id, track_id: track.id, clip_ids: ids });
    }
  }
  return slots;
}

/** Lay an arrangement out: sections concatenate in listed order. */
export function arrangementLayout(doc: TimelineDoc, arrangementId: string): ArrangedSection[] {
  const arrangement = doc.arrangements.find((a) => a.id === arrangementId);
  if (!arrangement) throw new Error(`unknown arrangement \`${arrangementId}\``);
  const out: ArrangedSection[] = [];
  let offset = 0;
  for (const id of arrangement.section_ids) {
    const section = doc.sections.find((s) => s.id === id);
    if (!section) throw new Error(`unknown section \`${id}\``);
    out.push({
      section_id: section.id,
      name: section.name,
      offset_beats: offset,
      length_beats: section.length_beats,
    });
    offset += section.length_beats;
  }
  return out;
}

/**
 * Build the frozen `ClipMoved` op moving one clip. `seq: 0` is a placeholder
 * the engine replaces; the caller supplies the actor (`ui`, `mcp`, ...).
 */
export function moveClipOp(actor: string, clipId: string, newStartBeats: number): Op {
  return OpSchema.parse({
    seq: 0,
    actor,
    kind: "ClipMoved" as OpKind,
    target: clipId,
    value_json: JSON.stringify({ startBeats: newStartBeats }),
  });
}

/**
 * Move a whole section by `deltaBeats`: one frozen `ClipMoved` op per member
 * clip (section motion is a fan-out, not a new op kind).
 */
export function moveSectionOps(
  actor: string,
  project: Project,
  section: Section,
  deltaBeats: number,
): Op[] {
  return sectionClips(project, section).map((c) =>
    moveClipOp(actor, c.id, c.start_beats + deltaBeats),
  );
}

/** Demo sidecar doc: Verse `0..16`, Chorus `16..32`, two arrangements. */
export function sampleTimelineDoc(): TimelineDoc {
  return {
    sections: [
      { id: "sec_verse", name: "Verse", start_beats: 0, length_beats: 16 },
      { id: "sec_chorus", name: "Chorus", start_beats: 16, length_beats: 16 },
    ],
    arrangements: [
      { id: "arr_linear", name: "Linear", section_ids: ["sec_verse", "sec_chorus"] },
      {
        id: "arr_radio",
        name: "Radio",
        section_ids: ["sec_chorus", "sec_verse", "sec_chorus"],
      },
    ],
    clip_props: [
      {
        clip_id: "clip_chorus_2",
        gain_db: 2,
        pitch_semitones: -2,
        time_ratio: 1,
        fade_in_beats: 0.5,
        fade_out_beats: 1,
        fx: ["dev_chorus_dub"],
        out: "bus_dub",
      },
    ],
  };
}
