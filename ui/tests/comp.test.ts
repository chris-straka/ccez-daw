import { describe, expect, test } from "bun:test";
import { ClipSchema } from "../src/generated/project";
import type { Clip } from "../src/generated/project";
import {
  CompError,
  RetroBuffer,
  auditionTake,
  buildComp,
  compCommitOp,
  takesForRegion,
} from "../src/comp/comp";

function take(id: string, start: number, len: number): Clip {
  return {
    id,
    track_id: "trk_vox",
    name: id,
    start_beats: start,
    length_beats: len,
    kind: "Audio",
    source: `take:${id}`,
  };
}

describe("Track E comping + retrospective capture", () => {
  test("comp-take builds a composite spanning sections from two takes", () => {
    const takes = [take("take_1", 0, 4), take("take_2", 0, 4)];
    const comp = buildComp(
      "clip_comp",
      "trk_vox",
      "Vox comp",
      [
        { take_id: "take_2", start_beats: 0, end_beats: 2 },
        { take_id: "take_1", start_beats: 2, end_beats: 4 },
      ],
      takes,
    );
    expect(comp.start_beats).toBe(0);
    expect(comp.length_beats).toBe(4);
    expect(comp.kind).toBe("Audio");
    expect(comp.source).toBe("comp:take_2+take_1");
    expect(() => ClipSchema.parse(comp)).not.toThrow();
  });

  test("comp-take rejects gaps, overlaps, and unknown takes", () => {
    const takes = [take("take_1", 0, 4), take("take_2", 0, 4)];
    expect(() =>
      buildComp(
        "c",
        "trk_vox",
        "x",
        [
          { take_id: "take_1", start_beats: 0, end_beats: 1 },
          { take_id: "take_2", start_beats: 2, end_beats: 4 },
        ],
        takes,
      ),
    ).toThrow(CompError);
    expect(() =>
      buildComp(
        "c",
        "trk_vox",
        "x",
        [{ take_id: "take_9", start_beats: 0, end_beats: 4 }],
        takes,
      ),
    ).toThrow(CompError);
    expect(() => buildComp("c", "trk_vox", "x", [], takes)).toThrow(CompError);
    try {
      buildComp(
        "c",
        "trk_vox",
        "x",
        [
          { take_id: "take_1", start_beats: 0, end_beats: 1 },
          { take_id: "take_2", start_beats: 2, end_beats: 4 },
        ],
        takes,
      );
      throw new Error("should have thrown");
    } catch (e) {
      expect((e as CompError).failure).toBe("gap-or-overlap");
    }
  });

  test("takes-for-region lists overlapping takes in order", () => {
    const takes = [take("take_2", 4, 4), take("take_1", 0, 4)];
    const found = takesForRegion(takes, "trk_vox", 0, 8);
    expect(found.map((c) => c.id)).toEqual(["take_1", "take_2"]);
    expect(takesForRegion(takes, "trk_vox", 0, 4)).toHaveLength(1);
    expect(takesForRegion(takes, "trk_other", 0, 8)).toHaveLength(0);
  });

  test("audition selects a take and rejects strangers", () => {
    const takes = [take("take_1", 0, 4), take("take_2", 0, 4)];
    expect(auditionTake(takes, "trk_vox", "take_2").id).toBe("take_2");
    expect(() => auditionTake(takes, "trk_vox", "take_9")).toThrow(CompError);
    try {
      auditionTake(takes, "trk_vox", "take_9");
      throw new Error("should have thrown");
    } catch (e) {
      expect((e as CompError).failure).toBe("unknown-take");
    }
    const other: Clip = { ...take("take_x", 0, 4), track_id: "trk_other" };
    try {
      auditionTake([...takes, other], "trk_vox", "take_x");
      throw new Error("should have thrown");
    } catch (e) {
      expect((e as CompError).failure).toBe("wrong-track");
    }
  });

  test("comp commit is a frozen ClipAdded op carrying the composite", () => {
    const takes = [take("take_1", 0, 4), take("take_2", 0, 4)];
    const comp = buildComp(
      "clip_comp",
      "trk_vox",
      "Vox comp",
      [
        { take_id: "take_2", start_beats: 0, end_beats: 2 },
        { take_id: "take_1", start_beats: 2, end_beats: 4 },
      ],
      takes,
    );
    const op = compCommitOp("ui", comp);
    expect(op.kind).toBe("ClipAdded");
    expect(op.seq).toBe(0);
    expect(op.target).toBe("clip_comp");
    const payload = JSON.parse(op.value_json) as Clip;
    expect(payload).toEqual(comp);
    expect(() => ClipSchema.parse(payload)).not.toThrow();
  });

  test("retrospective capture materializes the played window as a clip", () => {
    const buf = new RetroBuffer(128);
    buf.push(4.0, '{"note":60}');
    buf.push(4.5, '{"note":64}');
    buf.push(5.0, '{"note":67}');
    expect(buf.retrieve(4.0)).toHaveLength(3);
    expect(buf.retrieve(6.0)).toHaveLength(0);

    const clip = buf.toClip("clip_retro", "trk_vox", "Rescued idea", "Midi", 4.0);
    expect(clip).not.toBeNull();
    expect(clip!.start_beats).toBe(4.0);
    expect(clip!.length_beats).toBe(1.0);
    expect(clip!.source).toBe("retro:3-events");
    expect(() => ClipSchema.parse(clip!)).not.toThrow();

    expect(buf.toClip("clip_none", "trk_vox", "Nothing", "Midi", 6.0)).toBeNull();

    const small = new RetroBuffer(2);
    small.push(0, "a");
    small.push(1, "b");
    small.push(2, "c");
    expect(small.length).toBe(2);
    expect(small.retrieve(0)[0].data).toBe("b");
  });
});
