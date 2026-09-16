import { describe, expect, test } from "bun:test";
import type { Project } from "../src/generated/project";
import { sampleProject } from "../src/project/sample";
import { sampleTimelineDoc } from "../src/timeline/model";
import {
  jamRecordOps,
  launchBeatFor,
  planSceneLaunch,
  planSlotLaunch,
  quantizeBeat,
  quantizeLaunchForScene,
} from "../src/timeline/launch";

function timelineProject(): Project {
  const base = sampleProject();
  return {
    ...base,
    id: "proj_timeline",
    name: "Timeline Demo",
    tracks: [
      { id: "trk_drums", name: "Drums", volume: 0.8, pan: 0, muted: false, solo: false, clip_ids: ["clip_verse_1", "clip_chorus_1"], device_ids: [] },
      { id: "trk_bass", name: "Bass", volume: 0.8, pan: 0, muted: false, solo: false, clip_ids: ["clip_verse_2", "clip_chorus_2"], device_ids: [] },
    ],
    clips: [
      { id: "clip_verse_1", track_id: "trk_drums", name: "Verse-1", start_beats: 0, length_beats: 8, kind: "Audio", source: "take:clip_verse_1" },
      { id: "clip_verse_2", track_id: "trk_bass", name: "Verse-2", start_beats: 8, length_beats: 8, kind: "Midi", source: "take:clip_verse_2" },
      { id: "clip_chorus_1", track_id: "trk_drums", name: "Chorus-1", start_beats: 16, length_beats: 8, kind: "Midi", source: "take:clip_chorus_1" },
      { id: "clip_chorus_2", track_id: "trk_bass", name: "Chorus-2", start_beats: 24, length_beats: 8, kind: "Midi", source: "take:clip_chorus_2" },
    ],
  };
}

describe("clip-slot/scene launch", () => {
  test("quant grid snaps up to the next boundary", () => {
    expect(quantizeBeat(0, 1)).toBe(0);
    expect(quantizeBeat(3, 4)).toBe(4);
    expect(quantizeBeat(4, 4)).toBe(4);
    expect(quantizeBeat(17.2, 1)).toBe(18);
    expect(() => quantizeBeat(1, 0)).toThrow();
    expect(() => quantizeBeat(-1, 1)).toThrow();
  });

  test("scene quant holds section phrases", () => {
    expect(quantizeLaunchForScene(0, 16, 16)).toBe(16);
    expect(quantizeLaunchForScene(17, 16, 16)).toBe(32);
    expect(quantizeLaunchForScene(16, 16, 16)).toBe(16);
  });

  test("slot and scene plans carry quantized beats", () => {
    const project = timelineProject();
    const doc = sampleTimelineDoc();
    const plan = planSlotLaunch(
      project, doc,
      { section_id: "sec_chorus", track_id: "trk_drums" }, 17.2, { kind: "beat" },
    );
    expect(plan.launch_beat).toBe(18);
    expect(plan.clip_ids).toEqual(["clip_chorus_1"]);
    const scene = planSceneLaunch(project, doc, "sec_chorus", 3, { kind: "bar" });
    expect(scene.launch_beat).toBe(4);
    expect(scene.track_id).toBeNull();
    expect(scene.clip_ids).toEqual(["clip_chorus_1", "clip_chorus_2"]);
    expect(launchBeatFor(2, { kind: "scene" }, doc, "sec_chorus")).toBe(16);
    expect(() => planSceneLaunch(project, doc, "sec_nope", 0, { kind: "beat" })).toThrow();
  });

  test("jam records scenes as frozen ClipAdded ops", () => {
    const project = timelineProject();
    const doc = sampleTimelineDoc();
    const ops = jamRecordOps("ui", project, doc, [
      { section_id: "sec_verse", launch_beat: 0 },
      { section_id: "sec_chorus", launch_beat: 16 },
    ]);
    expect(ops).toHaveLength(4);
    for (const op of ops) {
      expect(op.kind).toBe("ClipAdded");
      expect(op.seq).toBe(0);
    }
    const starts = ops
      .filter((o) => JSON.parse(o.value_json).id.endsWith("__jam1"))
      .map((o) => JSON.parse(o.value_json).start_beats);
    expect(starts).toEqual([16, 24]);
  });

  test("jam rejects empty, unknown, and time-travelling events", () => {
    const project = timelineProject();
    const doc = sampleTimelineDoc();
    expect(() => jamRecordOps("ui", project, doc, [])).toThrow();
    expect(() =>
      jamRecordOps("ui", project, doc, [{ section_id: "sec_nope", launch_beat: 0 }]),
    ).toThrow();
    expect(() =>
      jamRecordOps("ui", project, doc, [
        { section_id: "sec_chorus", launch_beat: 16 },
        { section_id: "sec_verse", launch_beat: 4 },
      ]),
    ).toThrow();
  });
});
