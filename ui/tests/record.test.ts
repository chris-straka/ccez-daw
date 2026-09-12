import { describe, expect, test } from "bun:test";
import { ClipSchema } from "../src/generated/project";
import type { Clip } from "../src/generated/project";
import { buildComp, takesForRegion } from "../src/comp/comp";
import {
  RecordError,
  assignTakeLanes,
  countInClicks,
  countInSeconds,
  isPunching,
  latencyBeats,
  compensateCapture,
  punchOverlap,
  punchTakeClip,
  shouldMonitor,
  validatePunch,
} from "../src/record/record";

function punchClips(): Clip[] {
  const punch = validatePunch(0, 4);
  return [
    punchTakeClip("take_vox_p1", "trk_vox", "Vox p1", "Audio", punch),
    punchTakeClip("take_vox_p2", "trk_vox", "Vox p2", "Audio", punch),
  ];
}

describe("Agent 7 record workflow", () => {
  test("punch-take: two punched passes lane up and comp into a keeper", () => {
    const clips = punchClips();
    expect(clips[0].source).toBe("take:take_vox_p1");
    expect(clips[0].length_beats).toBe(4);
    for (const c of clips) expect(() => ClipSchema.parse(c)).not.toThrow();

    const takes = takesForRegion(clips, "trk_vox", 0, 4);
    expect(takes.map((t) => t.id)).toEqual(["take_vox_p1", "take_vox_p2"]);

    expect(assignTakeLanes(takes)).toEqual([
      { take_id: "take_vox_p1", lane: 0 },
      { take_id: "take_vox_p2", lane: 1 },
    ]);

    const keeper = buildComp("clip_vox_keeper", "trk_vox", "Vox keeper", [
      { take_id: "take_vox_p2", start_beats: 0, end_beats: 2 },
      { take_id: "take_vox_p1", start_beats: 2, end_beats: 4 },
    ], takes);
    expect(keeper.length_beats).toBe(4);
    expect(keeper.source).toBe("comp:take_vox_p2+take_vox_p1");
    expect(() => ClipSchema.parse(keeper)).not.toThrow();
  });

  test("auto-punch gates on range edges; manual follows the transport", () => {
    const auto = { kind: "auto", range: validatePunch(8, 12) } as const;
    expect(isPunching(7.5, auto, true)).toBe(false);
    expect(isPunching(8, auto, true)).toBe(true);
    expect(isPunching(11.9, auto, true)).toBe(true);
    expect(isPunching(12, auto, true)).toBe(false);
    expect(isPunching(10, auto, false)).toBe(false);
    expect(isPunching(99, { kind: "manual" }, true)).toBe(true);
    expect(isPunching(99, { kind: "manual" }, false)).toBe(false);
    expect(() => validatePunch(4, 4)).toThrow(RecordError);
    expect(() => validatePunch(-1, 4)).toThrow(RecordError);
  });

  test("count-in clicks accent bar starts; monitor auto mutes during playback", () => {
    const clicks = countInClicks({ bars: 1, beats_per_bar: 4 });
    expect(clicks).toEqual([
      { offset_beats: -4, accent: true },
      { offset_beats: -3, accent: false },
      { offset_beats: -2, accent: false },
      { offset_beats: -1, accent: false },
    ]);
    expect(countInSeconds({ bars: 1, beats_per_bar: 4 }, 120)).toBe(2);
    expect(shouldMonitor("auto", true, "Recording")).toBe(true);
    expect(shouldMonitor("auto", true, "Playing")).toBe(false);
    expect(shouldMonitor("on", true, "Playing")).toBe(true);
    expect(shouldMonitor("off", true, "Recording")).toBe(false);
  });

  test("latency compensation round-trips; overlap trims manual passes", () => {
    const beats = latencyBeats(256, 256, 48_000, 120);
    expect(beats).toBeCloseTo((512 / 48_000) * 2, 12);
    expect(compensateCapture(10 + beats, beats)).toBeCloseTo(10, 12);
    const window = validatePunch(8, 12);
    expect(punchOverlap(6, 14, window)).toEqual([8, 12]);
    expect(punchOverlap(0, 8, window)).toBeNull();
    expect(() => assignTakeLanes([])).toThrow(RecordError);
  });
});
