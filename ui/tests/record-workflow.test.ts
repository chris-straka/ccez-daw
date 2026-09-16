import { describe, expect, test } from "bun:test";
import { ClipSchema, OpSchema } from "../src/generated/project";
import { punchTakeClip, validatePunch } from "../src/record/record";
import {
  RecordError,
  armTrack,
  disarmTrack,
  isArmed,
  punchRangeForSection,
  takeCommitOp,
  toggleArm,
} from "../src/record/record";

describe("record workflow: arm / section punch / take commit", () => {
  test("arm helpers track switches in sorted order", () => {
    let armed: string[] = [];
    expect(isArmed(armed, "trk_vox")).toBe(false);
    armed = armTrack(armed, "trk_vox");
    armed = armTrack(armed, "trk_gtr");
    expect(armed).toEqual(["trk_gtr", "trk_vox"]);
    // Re-arming is idempotent.
    expect(armTrack(armed, "trk_vox")).toEqual(armed);
    armed = toggleArm(armed, "trk_vox");
    expect(isArmed(armed, "trk_vox")).toBe(false);
    armed = toggleArm(armed, "trk_vox");
    expect(isArmed(armed, "trk_vox")).toBe(true);
    armed = disarmTrack(disarmTrack(armed, "trk_gtr"), "trk_vox");
    expect(armed).toEqual([]);
  });

  test("section punch snaps to launcher section boundaries", () => {
    expect(punchRangeForSection(16, 16)).toEqual({ start_beats: 16, end_beats: 32 });
    expect(() => punchRangeForSection(0, 0)).toThrow(RecordError);
    expect(() => punchRangeForSection(0, -4)).toThrow(RecordError);
    expect(() => punchRangeForSection(-16, 16)).toThrow(RecordError);
    expect(() => punchRangeForSection(0, Number.POSITIVE_INFINITY)).toThrow(RecordError);
  });

  test("take commit is an ordinary ClipAdded op carrying the take clip", () => {
    const punch = validatePunch(4, 8);
    const take = punchTakeClip("take_music_p1", "trk_music", "Take 1", "Audio", punch);
    const op = takeCommitOp("ui", take);
    expect(() => OpSchema.parse(op)).not.toThrow();
    expect(op.kind).toBe("ClipAdded");
    expect(op.seq).toBe(0);
    expect(op.target).toBe("take_music_p1");
    const carried = ClipSchema.parse(JSON.parse(op.value_json as string));
    expect(carried.id).toBe("take_music_p1");
    expect(carried.source).toBe("take:take_music_p1");
    expect(carried.start_beats).toBe(4);
    expect(carried.length_beats).toBe(4);
  });
});
