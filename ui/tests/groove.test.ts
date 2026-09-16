import { describe, expect, test } from "bun:test";
import type { Clip } from "../src/generated/project";
import type { MidiClip } from "../src/pianoroll/model";
import { newNote } from "../src/pianoroll/model";
import {
  GroovePool,
  applyGroove,
  applyGrooveAmount,
  extractGroove,
  flatGroove,
  grooveCommitOp,
  groovedSource,
  previewGroove,
  sanitizeGrooveName,
  validateTemplate,
} from "../src/groove/model";

/** Step 1 (of 4) played late (+0.06) and loud (120); rest on-grid at 100. */
function swungClip(): MidiClip {
  const starts = [0, 0.25, 0.5, 0.75];
  return {
    length_beats: 4,
    notes: starts.map((start_beats, i) => ({
      ...newNote(i + 1, 60, i === 1 ? 120 : 100, start_beats, 0.2),
      timing_offset_beats: i === 1 ? 0.06 : 0,
    })),
  };
}

function gridClip(): MidiClip {
  const starts = [0, 0.25, 0.5, 0.75];
  return {
    length_beats: 4,
    notes: starts.map((start_beats, i) => newNote(i + 1, 60, 100, start_beats, 0.2)),
  };
}

function timelineClip(): Clip {
  return {
    id: "clip_b",
    track_id: "trk_music",
    name: "Sketch",
    start_beats: 4,
    length_beats: 8,
    kind: "Midi",
    source: "take:1",
  };
}

describe("groove extract (mirrors Rust groove tests)", () => {
  test("learns per-step timing and velocity; empty steps stay neutral", () => {
    const t = extractGroove(swungClip(), 4);
    expect(t.steps_per_beat).toBe(4);
    expect(t.offsets[1]).toBeCloseTo(0.06, 12);
    expect(t.vel_scale[1]).toBeCloseTo(1.2, 12);
    for (const i of [0, 2, 3]) {
      expect(t.offsets[i]).toBe(0);
      expect(t.vel_scale[i]).toBe(1);
    }
  });

  test("ignores muted notes; rejects bad resolution", () => {
    const clip = swungClip();
    clip.notes.push({ ...newNote(9, 72, 127, 1.5, 0.2), muted: true, timing_offset_beats: 0.25 });
    const t = extractGroove(clip, 4);
    expect(t.offsets[2]).toBe(0);
    expect(t.vel_scale[2]).toBe(1);
    expect(() => extractGroove(gridClip(), 0)).toThrow();
  });

  test("flat template is the identity transfer", () => {
    const flat = flatGroove(4);
    expect(applyGroove(gridClip(), flat, { quantize: 1, timing: 1, velocity: 1 })).toEqual(
      gridClip(),
    );
  });
});

describe("groove apply depths", () => {
  test("amount 0 is identity; amount 1 transfers feel", () => {
    const template = extractGroove(swungClip(), 4);
    expect(applyGrooveAmount(gridClip(), template, 0)).toEqual(gridClip());
    const out = applyGrooveAmount(gridClip(), template, 1);
    expect(out.notes[1].timing_offset_beats).toBeCloseTo(0.06, 12);
    expect(out.notes[1].velocity).toBe(120);
    expect(out.notes[0].timing_offset_beats).toBe(0);
    expect(out.notes[0].velocity).toBe(100);
  });

  test("timing and velocity depths move independently", () => {
    const template = extractGroove(swungClip(), 4);
    const timingOnly = applyGroove(gridClip(), template, { quantize: 0, timing: 1, velocity: 0 });
    expect(timingOnly.notes[1].timing_offset_beats).toBeCloseTo(0.06, 12);
    expect(timingOnly.notes[1].velocity).toBe(100);
    const velHalf = applyGroove(gridClip(), template, { quantize: 0, timing: 0, velocity: 0.5 });
    expect(velHalf.notes[1].timing_offset_beats).toBe(0);
    expect(velHalf.notes[1].velocity).toBe(110);
  });

  test("quantize pulls onsets to grid inside the lane, never moves starts", () => {
    const early: MidiClip = {
      length_beats: 4,
      notes: [{ ...newNote(1, 60, 100, 1, 0.2), timing_offset_beats: -0.08 }],
    };
    const out = applyGroove(early, flatGroove(4), { quantize: 1, timing: 0, velocity: 0 });
    expect(out.notes[0].timing_offset_beats).toBeCloseTo(0, 12);
    expect(out.notes[0].start_beats).toBe(1);
  });

  test("clamps to the microtiming lane, skips muted, refuses bad depths", () => {
    const template = { steps_per_beat: 2, offsets: [0.25, -0.25], vel_scale: [1, 1] };
    const clip: MidiClip = {
      length_beats: 4,
      notes: [
        { ...newNote(1, 60, 100, 0, 0.2), timing_offset_beats: 0.2 },
        { ...newNote(2, 64, 40, 0.5, 0.2), muted: true },
      ],
    };
    const out = applyGrooveAmount(clip, template, 1);
    expect(out.notes[0].timing_offset_beats).toBe(0.25);
    expect(out.notes[1].timing_offset_beats).toBe(0);
    expect(out.notes[1].velocity).toBe(40);
    expect(() =>
      applyGroove(gridClip(), flatGroove(4), { quantize: 0, timing: 0, velocity: 2 }),
    ).toThrow();
    expect(() => validateTemplate({ steps_per_beat: 2, offsets: [0], vel_scale: [1, 1] })).toThrow();
  });

  test("preview reports deltas without committing", () => {
    const template = extractGroove(swungClip(), 4);
    const before = gridClip();
    const p = previewGroove(before, template, { quantize: 0, timing: 1, velocity: 1 });
    expect(p.moved).toBe(1);
    expect(p.meanAbsTimingDelta).toBeCloseTo(0.06 / 4, 12);
    expect(p.meanAbsVelocityDelta).toBeCloseTo(20 / 4, 12);
    expect(before.notes[1].velocity).toBe(100);
  });
});

describe("groove pool + op commit", () => {
  test("pool files, replaces, trims, forgets, and JSON round-trips", () => {
    const pool = new GroovePool();
    expect(pool.size).toBe(0);
    expect(() => pool.set("  ", flatGroove(4))).toThrow();
    pool.set("swing", extractGroove(swungClip(), 4));
    pool.set("flat", flatGroove(4));
    expect(pool.names()).toEqual(["flat", "swing"]);
    pool.set(" swing ", flatGroove(4));
    expect(pool.size).toBe(2);
    expect(pool.get("swing")).toEqual(flatGroove(4));
    const back = GroovePool.fromJSON(pool.toJSON());
    expect(back.names()).toEqual(["flat", "swing"]);
    expect(back.get("flat")).toEqual(flatGroove(4));
    expect(() => GroovePool.fromJSON("not json")).toThrow();
    expect(pool.delete("swing")).toBe(true);
    expect(pool.delete("swing")).toBe(false);
    expect(pool.get("swing")).toBeUndefined();
  });

  test("commit is an ordinary ClipAdded op with the groove source convention", () => {
    expect(sanitizeGrooveName("late hats!")).toBe("late_hats_");
    expect(groovedSource("take:1", "late hats!")).toBe("groove:late_hats_+take:1");
    const op = grooveCommitOp("ui", timelineClip(), "late hats!");
    expect(op.kind).toBe("ClipAdded");
    expect(op.seq).toBe(0);
    expect(op.target).toBe("clip_b~groove-late_hats_");
    const payload = JSON.parse(op.value_json) as Clip;
    expect(payload.track_id).toBe("trk_music");
    expect(payload.kind).toBe("Midi");
    expect(payload.start_beats).toBe(4);
    expect(payload.source).toBe("groove:late_hats_+take:1");
  });
});
