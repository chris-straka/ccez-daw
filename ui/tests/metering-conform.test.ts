import { describe, expect, test } from "bun:test";
import {
  CONFORM_PRESETS,
  conformReportFilename,
  conformVerdict,
  presetById,
  previewConformGain,
} from "../src/metering/conform";
import { checkTarget } from "../src/metering/model";

describe("conformance presets", () => {
  test("table covers all five targets with in-range numbers", () => {
    expect(CONFORM_PRESETS.map((p) => p.id)).toEqual([
      "pc",
      "switch",
      "playstation",
      "xbox",
      "mobile",
    ]);
    for (const p of CONFORM_PRESETS) {
      expect(checkTarget(p)).toBeNull();
    }
  });

  test("only PlayStation is spec-backed; the rest say approximate", () => {
    expect(presetById("playstation")?.approximate).toBe(false);
    for (const id of ["pc", "switch", "xbox", "mobile"]) {
      expect(presetById(id)?.approximate).toBe(true);
    }
    expect(presetById("gameboy")).toBeUndefined();
  });

  test("preview gain hits the target; the ceiling wins when hot", () => {
    const ps = presetById("playstation")!;
    const q = previewConformGain(-22, 0.25, ps);
    expect(q.gainDb).toBeCloseTo(-2.0, 6);
    expect(q.limitedByCeiling).toBe(false);
    const hot = previewConformGain(-8, 0.9, {
      ...ps,
      targetLufs: -8,
      truePeakCeilingDbfs: -9,
    });
    expect(hot.limitedByCeiling).toBe(true);
    expect(previewConformGain(-70, 0, ps).gain).toBe(1);
  });

  test("verdict passes on target, fails ceiling-limited or hot output", () => {
    const ps = presetById("playstation")!;
    expect(
      conformVerdict(ps, {
        outputLufs: -23.6,
        outputTruePeakDbfs: -2.05,
        limitedByCeiling: false,
      }).passed,
    ).toBe(true);
    const limited = conformVerdict(ps, {
      outputLufs: -26,
      outputTruePeakDbfs: -2.0,
      limitedByCeiling: true,
    });
    expect(limited.passed).toBe(false);
    expect(limited.ceilingMet).toBe(true);
    expect(
      conformVerdict(ps, {
        outputLufs: -20,
        outputTruePeakDbfs: -2.0,
        limitedByCeiling: false,
      }).targetMet,
    ).toBe(false);
  });

  test("report artifact names itself after the stem", () => {
    expect(conformReportFilename("boss_layer")).toBe("boss_layer.conform.json");
  });
});
