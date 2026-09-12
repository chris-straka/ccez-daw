import { describe, expect, test } from "bun:test";
import {
  DEFAULT_CLICK_THRESHOLD,
  analyzeSeam,
  applyEdgeFade,
  clickErrorText,
  detectLoopClick,
  injectClick,
  isLoopWindowValid,
  parseWaiverFile,
  seamStep,
  synthesizeLoopTone,
  waiverFileJson,
  wrapBeatIntoLoop,
} from "../src/gameaudio/loop";

describe("loop-seam audition (S-2)", () => {
  test("clean loop passes, seeded click is detected", () => {
    // Loop-clean baseline: preview tone plus the export-style edge fade.
    const clean = applyEdgeFade(synthesizeLoopTone(8000, 220, 0.5), 240);
    expect(seamStep(clean)).toBeLessThan(1e-3);
    expect(detectLoopClick(clean)).toBe(false);
    expect(detectLoopClick(clean, DEFAULT_CLICK_THRESHOLD)).toBe(false);

    const clicked = injectClick(clean, 2026, 32, 0.3);
    const report = analyzeSeam(clicked);
    expect(report.step).toBeGreaterThan(DEFAULT_CLICK_THRESHOLD);
    expect(report.click).toBe(true);
    expect(detectLoopClick(clicked)).toBe(true);
    // Same seed, same click: deterministic.
    expect(seamStep(injectClick(clean, 2026, 32, 0.3))).toBeCloseTo(report.step, 9);
    // Different seed still clicks (any jump over threshold counts).
    expect(detectLoopClick(injectClick(clean, 7, 32, 0.3))).toBe(true);
  });

  test("raw whole-cycle tone still clicks without the fade", () => {
    expect(detectLoopClick(synthesizeLoopTone(8000, 220, 0.5))).toBe(true);
  });

  test("fix (edge fade) clears the click the injection made", () => {
    const raw = synthesizeLoopTone(8000, 220, 0.5);
    const clicked = injectClick(applyEdgeFade(raw, 240), 2026, 32, 0.3);
    expect(detectLoopClick(clicked)).toBe(true);
    const fixed = applyEdgeFade(clicked, 240);
    expect(detectLoopClick(fixed)).toBe(false);
  });

  test("click error text carries fix-or-waive guidance", () => {
    const report = analyzeSeam(injectClick(synthesizeLoopTone(8000, 220), 2026, 32, 0.3));
    const text = clickErrorText("stems/cue_bed.wav", report);
    expect(text).toContain("stems/cue_bed.wav");
    expect(text).toContain("loop seam");
    expect(text).toContain("Fix:");
    expect(text).toContain("loop-waivers.json");
  });

  test("audition clock wraps gaplessly into the loop window", () => {
    expect(wrapBeatIntoLoop(9.5, 0, 8)).toBeCloseTo(1.5, 9);
    expect(wrapBeatIntoLoop(8, 0, 8)).toBeCloseTo(0, 9);
    expect(wrapBeatIntoLoop(-1, 0, 8)).toBeCloseTo(7, 9);
    expect(wrapBeatIntoLoop(3, 0, 8)).toBeCloseTo(3, 9);
    // Degenerate windows hold at start instead of producing NaN.
    expect(wrapBeatIntoLoop(5, 0, 0)).toBe(0);
    expect(wrapBeatIntoLoop(5, 4, 2)).toBe(4);
  });

  test("loop windows validate like the ship-gate", () => {
    expect(isLoopWindowValid(0, 8)).toBe(true);
    expect(isLoopWindowValid(0, 0)).toBe(false); // one-shot, not a loop
    expect(isLoopWindowValid(8, 4)).toBe(false);
    expect(isLoopWindowValid(-1, 4)).toBe(false);
  });

  test("waiver file round-trips and rejects empty reasons", () => {
    const json = waiverFileJson([
      { path: "stems/cue_bed.wav", reason: "field recording with a real tape splice" },
    ]);
    expect(parseWaiverFile(json)).toEqual([
      { path: "stems/cue_bed.wav", reason: "field recording with a real tape splice" },
    ]);
    expect(parseWaiverFile('{ "waivers": [] }')).toEqual([]);
    expect(() => parseWaiverFile('{ "nope": [] }')).toThrow();
    expect(() => parseWaiverFile('{ "waivers": [{ "path": "s.wav", "reason": "  " }] }')).toThrow();
    expect(() =>
      parseWaiverFile('{ "waivers": [{ "path": "s.wav", "reason": "a" }, { "path": "s.wav", "reason": "b" }] }'),
    ).toThrow();
  });

  test("empty buffers stay silent", () => {
    expect(seamStep(new Float32Array(0))).toBe(0);
    expect(detectLoopClick(new Float32Array(0))).toBe(false);
  });
});
