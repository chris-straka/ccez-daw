import { describe, expect, test } from "bun:test";
import { sampleProject } from "../src/project/sample";
import type { Project } from "../src/generated/project";
import {
  arrangementLayout,
  clipsSortedOnTrack,
  findOverlaps,
  launcherSlots,
  moveClipOp,
  moveSectionOps,
  propsFor,
  sampleTimelineDoc,
  sectionClips,
  validateClipProps,
} from "../src/timeline/model";

/** Timeline demo project: mirrors `sample_timeline_project` in Rust. */
function timelineProject(): Project {
  const base = sampleProject();
  return {
    ...base,
    id: "proj_timeline",
    name: "Timeline Demo",
    tracks: [
      {
        id: "trk_drums",
        name: "Drums",
        volume: 0.8,
        pan: 0,
        muted: false,
        solo: false,
        clip_ids: ["clip_verse_1", "clip_chorus_1"],
        device_ids: [],
      },
      {
        id: "trk_bass",
        name: "Bass",
        volume: 0.8,
        pan: 0,
        muted: false,
        solo: false,
        clip_ids: ["clip_verse_2", "clip_chorus_2"],
        device_ids: [],
      },
    ],
    clips: [
      {
        id: "clip_verse_1",
        track_id: "trk_drums",
        name: "Verse-1",
        start_beats: 0,
        length_beats: 8,
        kind: "Audio",
        source: "take:clip_verse_1",
      },
      {
        id: "clip_verse_2",
        track_id: "trk_bass",
        name: "Verse-2",
        start_beats: 8,
        length_beats: 8,
        kind: "Midi",
        source: "take:clip_verse_2",
      },
      {
        id: "clip_chorus_1",
        track_id: "trk_drums",
        name: "Chorus-1",
        start_beats: 16,
        length_beats: 8,
        kind: "Midi",
        source: "take:clip_chorus_1",
      },
      {
        id: "clip_chorus_2",
        track_id: "trk_bass",
        name: "Chorus-2",
        start_beats: 24,
        length_beats: 8,
        kind: "Midi",
        source: "take:clip_chorus_2",
      },
    ],
  };
}

describe("Track E timeline + clips", () => {
  test("move-Chorus-2 emits one frozen ClipMoved op", () => {
    // Mirrors the Rust `move_chorus_2_via_frozen_clip_moved_op` test: the
    // drag of clip Chorus-2 is a frozen op-log entry, not a new IPC shape.
    const op = moveClipOp("ui", "clip_chorus_2", 32);
    expect(op.kind).toBe("ClipMoved");
    expect(op.target).toBe("clip_chorus_2");
    expect(op.seq).toBe(0); // placeholder: the engine assigns seq
    expect(JSON.parse(op.value_json)).toEqual({ startBeats: 32 });
  });

  test("moving a section fans out to member clips only", () => {
    const project = timelineProject();
    const chorus = sampleTimelineDoc().sections.find((s) => s.id === "sec_chorus")!;
    const ops = moveSectionOps("ui", project, chorus, 8);
    expect(ops.map((o) => o.target).sort()).toEqual(["clip_chorus_1", "clip_chorus_2"]);
    for (const op of ops) expect(op.kind).toBe("ClipMoved");
  });

  test("launcher and linear are views over one model", () => {
    const project = timelineProject();
    const doc = sampleTimelineDoc();
    const drums = clipsSortedOnTrack(project, "trk_drums");
    expect(drums.map((c) => c.id)).toEqual(["clip_verse_1", "clip_chorus_1"]);
    const slots = launcherSlots(project, doc);
    expect(slots).toHaveLength(4); // 2 sections x 2 tracks
    const chorusDrums = slots.find(
      (s) => s.section_id === "sec_chorus" && s.track_id === "trk_drums",
    )!;
    expect(chorusDrums.clip_ids).toEqual(["clip_chorus_1"]);
    const chorus = doc.sections.find((s) => s.id === "sec_chorus")!;
    expect(sectionClips(project, chorus).map((c) => c.id)).toEqual([
      "clip_chorus_1",
      "clip_chorus_2",
    ]);
  });

  test("multiple arrangements share section objects", () => {
    const doc = sampleTimelineDoc();
    const linear = arrangementLayout(doc, "arr_linear");
    expect(linear.map((s) => s.section_id)).toEqual(["sec_verse", "sec_chorus"]);
    expect(linear[1].offset_beats).toBe(16);
    const radio = arrangementLayout(doc, "arr_radio");
    expect(radio.map((s) => s.section_id)).toEqual([
      "sec_chorus",
      "sec_verse",
      "sec_chorus",
    ]);
    const last = radio[radio.length - 1];
    expect(last.offset_beats + last.length_beats).toBe(48);
  });

  test("object-level clip props validate with flat defaults", () => {
    const doc = sampleTimelineDoc();
    const props = propsFor(doc, "clip_chorus_2");
    expect(() => validateClipProps(props)).not.toThrow();
    expect(props.fx).toEqual(["dev_chorus_dub"]);
    expect(props.out).toBe("bus_dub");
    const flat = propsFor(doc, "clip_verse_1");
    expect(flat.gain_db).toBe(0);
    expect(flat.time_ratio).toBe(1);
    expect(() =>
      validateClipProps({ ...flat, pitch_semitones: 48 }),
    ).toThrow();
  });

  test("overlaps are detected per track", () => {
    const project = timelineProject();
    expect(findOverlaps(project, "trk_bass")).toEqual([]);
    const overlapping: Project = {
      ...project,
      clips: [
        ...project.clips,
        {
          id: "clip_overlap",
          track_id: "trk_bass",
          name: "Overlap",
          start_beats: 26,
          length_beats: 4,
          kind: "Audio",
          source: "take:overlap",
        },
      ],
    };
    const pairs = findOverlaps(overlapping, "trk_bass");
    expect(pairs).toHaveLength(1);
    expect(pairs[0].sort()).toEqual(["clip_chorus_2", "clip_overlap"]);
  });
});
