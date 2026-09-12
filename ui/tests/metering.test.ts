import { describe, expect, test } from "bun:test";
import {
  checkTarget,
  formatLufs,
  gainForTarget,
  gainToDb,
  meterBlock,
} from "../src/metering/model";

describe("metering model", () => {
  test("block reduces to peak/RMS with a clip lamp", () => {
    const r = meterBlock([0.5, -0.25, 0, 0.25]);
    expect(r.peak).toBeCloseTo(0.5, 6);
    expect(r.peakDb).toBeCloseTo(gainToDb(0.5), 9);
    expect(r.rms).toBeCloseTo(0.3061862, 5);
    expect(r.clipped).toBe(false);
    expect(meterBlock([0.2, 1]).clipped).toBe(true);
  });

  test("empty block is silence, not NaN", () => {
    const r = meterBlock([]);
    expect(r.peak).toBe(0);
    expect(r.peakDb).toBe(-120);
    expect(r.rmsDb).toBe(-120);
    expect(r.clipped).toBe(false);
  });

  test("target validation mirrors the Rust range checks", () => {
    expect(checkTarget({ targetLufs: -16, truePeakCeilingDbfs: -1 })).toBeNull();
    expect(checkTarget({ targetLufs: -80, truePeakCeilingDbfs: -1 })).not.toBeNull();
    expect(checkTarget({ targetLufs: -16, truePeakCeilingDbfs: 3 })).not.toBeNull();
    expect(checkTarget({ targetLufs: NaN, truePeakCeilingDbfs: -1 })).not.toBeNull();
  });

  test("preview gain hits the target; the ceiling wins when hot", () => {
    const q = gainForTarget(-22, 0.25, { targetLufs: -16, truePeakCeilingDbfs: -1 });
    expect(q.gainDb).toBeCloseTo(6.0, 6);
    expect(q.limitedByCeiling).toBe(false);
    const hot = gainForTarget(-8, 0.9, { targetLufs: -8, truePeakCeilingDbfs: -9 });
    expect(hot.limitedByCeiling).toBe(true);
    expect(gainToDb(0.9 * hot.gain)).toBeCloseTo(-9, 6);
    expect(gainForTarget(-70, 0, { targetLufs: -16, truePeakCeilingDbfs: -1 }).gain).toBe(1);
  });

  test("LUFS formatting floors at silence", () => {
    expect(formatLufs(-16.04)).toBe("-16.0 LUFS");
    expect(formatLufs(-70)).toBe("-70.0 LUFS (silence)");
    expect(formatLufs(Number.NEGATIVE_INFINITY)).toBe("-70.0 LUFS (silence)");
  });
});
