import { describe, expect, test } from "bun:test";
import { sampleProject } from "../src/project/sample";
import {
  evalLane,
  instantiateClip,
  mergePoints,
  modSumAt,
  paramBaseAndRange,
  renderLaneSamples,
  resolveParamSamples,
  sampleAutomationDoc,
  validateClip,
  validateLane,
} from "../src/automation/model";

const lane = () => ({
  id: "lane_vol",
  target: { node: "trk_music", param: "volume" },
  points: [
    { beat: 0, value: 0 },
    { beat: 4, value: 1 },
  ],
});

describe("automation lanes", () => {
  test("lane render is sample-accurate (tempo 60, 4 Hz => value = i/16)", () => {
    const out = renderLaneSamples(lane(), 0, 60, 4, 17);
    expect(out).toHaveLength(17);
    for (let i = 0; i < out.length; i++) {
      expect(out[i]).toBeCloseTo(Math.min(i / 16, 1), 12);
    }
    expect(out[0]).toBe(0);
    expect(out[4]).toBe(0.25);
    expect(out[8]).toBe(0.5);
    expect(out[16]).toBe(1);
  });

  test("mid-block offset agrees with point evaluation", () => {
    expect(renderLaneSamples(lane(), 1, 60, 4, 4)).toEqual([0.25, 0.3125, 0.375, 0.4375]);
  });

  test("holds outside the point range", () => {
    expect(evalLane(lane(), -100)).toBe(0);
    expect(evalLane(lane(), 100)).toBe(1);
  });

  test("non-ascending beats rejected", () => {
    const bad = lane();
    bad.points.push({ beat: 4, value: 0 });
    expect(() => validateLane(bad)).toThrow();
  });
});

describe("reusable clips", () => {
  test("instantiate stamps relative beats to absolute", () => {
    const [clip] = sampleAutomationDoc().clips;
    expect(instantiateClip(clip, 8)).toEqual([
      { beat: 8, value: 0 },
      { beat: 12, value: 1 },
    ]);
  });

  test("merge replaces same-beat points and restores order", () => {
    const target = lane();
    target.points.push({ beat: 8, value: 0 });
    const merged = mergePoints(target, [
      { beat: 8, value: 0.5 },
      { beat: 10, value: 0.9 },
    ]);
    expect(merged.map((p) => p.beat)).toEqual([0, 4, 8, 10]);
    expect(merged.find((p) => p.beat === 8)!.value).toBe(0.5);
  });

  test("out-of-range clip points rejected", () => {
    expect(() =>
      validateClip({ id: "c", name: "C", length_beats: 2, points: [{ beat: 5, value: 0 }] }),
    ).toThrow();
  });
});

describe("modulation", () => {
  test("constant mod clamps to the volume ceiling", () => {
    const project = sampleProject();
    const target = { node: "trk_music", param: "volume" };
    const l = { id: "l", target, points: [{ beat: 0, value: 0.8 }] };
    const matrix = {
      routes: [{ id: "r", source: { kind: "Constant", value: 10 } as const, target, depth: 1 }],
    };
    const out = resolveParamSamples(project, target, l, matrix, 0, 120, 44100, 8);
    expect(out.every((v) => v === 1.5)).toBe(true);
  });

  test("any-to-any: device param addressed identically", () => {
    const project = {
      ...sampleProject(),
      devices: [
        {
          id: "dev_filter",
          kind: "Device" as const,
          name: "Filter",
          params: [
            { id: "cutoff", label: "Cutoff", value: 1000, min: 20, max: 20000, default: 1000, unit: "Hz" },
          ],
        },
      ],
    };
    const target = { node: "dev_filter", param: "cutoff" };
    const matrix = {
      routes: [{ id: "r", source: { kind: "Constant", value: 1 } as const, target, depth: 100 }],
    };
    expect(resolveParamSamples(project, target, null, matrix, 0, 120, 44100, 4)).toEqual([
      1100, 1100, 1100, 1100,
    ]);
  });

  test("sine route contributes depth * sin at exact times", () => {
    const doc = sampleAutomationDoc();
    const matrix = { routes: doc.routes };
    // 5 Hz sine at t=0.05 s (quarter period) peaks at +1 => depth 0.1.
    expect(modSumAt(matrix, { node: "trk_music", param: "volume" }, 0.05)).toBeCloseTo(0.1, 12);
    expect(modSumAt(matrix, { node: "trk_music", param: "volume" }, 0)).toBeCloseTo(0, 12);
  });

  test("unknown param throws instead of rendering silence", () => {
    expect(() =>
      paramBaseAndRange(sampleProject(), { node: "trk_music", param: "reverb_size" }),
    ).toThrow();
  });
});
