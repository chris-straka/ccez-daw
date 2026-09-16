import { describe, expect, test } from "bun:test";
import { ClipSchema, OpSchema } from "../src/generated/project";
import {
  TakeBuffer,
  availableInputDevices,
  formatLevels,
  monitorLevels,
  punchTakeClip,
  selectInputDevice,
  takeCommitOp,
  validatePunch,
} from "../src/record/record";

const DEVICES = [
  { id: "mic", name: "Microphone" },
  { id: "di", name: "DI Box" },
];

describe("record input half: device select, monitoring levels, take buffer", () => {
  test("no devices listed headless: null input fallback, never an error", () => {
    expect(availableInputDevices(undefined)).toEqual([]);
    expect(availableInputDevices(null)).toEqual([]);
    expect(availableInputDevices([])).toEqual([]);
    expect(availableInputDevices(DEVICES)).toEqual(DEVICES);
  });

  test("device select keeps the current device on unknown ids", () => {
    expect(selectInputDevice(null, DEVICES, null)).toBeNull();
    expect(selectInputDevice(null, DEVICES, "mic")).toBe("mic");
    // Unknown hardware: keep capturing from the current source.
    expect(selectInputDevice("mic", DEVICES, "ghost")).toBe("mic");
    expect(selectInputDevice(null, DEVICES, "ghost")).toBeNull();
    // Reselecting null returns to the headless-safe source.
    expect(selectInputDevice("mic", DEVICES, null)).toBeNull();
  });

  test("monitoring levels mirror the Rust monitor_levels math", () => {
    expect(monitorLevels([])).toEqual({ peak: 0, rms: 0 });
    const flat = monitorLevels([0.5, 0.5, 0.5, 0.5]);
    expect(flat.peak).toBeCloseTo(0.5, 12);
    expect(flat.rms).toBeCloseTo(0.5, 12);
    const mixed = monitorLevels([-1, 0.5, -0.5, 0]);
    expect(mixed.peak).toBe(1);
    expect(mixed.rms).toBeCloseTo(Math.sqrt((1 + 0.25 + 0.25) / 4), 12);
    expect(formatLevels({ peak: 0.5, rms: 0.25 })).toBe("peak 0.50 rms 0.25");
  });

  test("take buffer accumulates drained blocks and reports live levels", () => {
    const buf = new TakeBuffer();
    expect(buf.frames()).toBe(0);
    expect(buf.levels()).toEqual({ peak: 0, rms: 0 });
    buf.push([0.25, -0.5]);
    buf.push([0, 0.5]);
    expect(buf.frames()).toBe(4);
    const levels = buf.levels();
    expect(levels.peak).toBe(0.5);
    expect(levels.rms).toBeCloseTo(Math.sqrt((0.0625 + 0.25 + 0 + 0.25) / 4), 12);
    buf.clear();
    expect(buf.frames()).toBe(0);
  });

  test("captured take lands as an ordinary ClipAdded audio clip", () => {
    // Null-input roundtrip on the UI side: blocks accumulate, then the
    // take commits through the frozen ClipAdded op like any other op.
    const buf = new TakeBuffer();
    for (let i = 0; i < 4; i++) buf.push([0.1, -0.1, 0.2, -0.2]);
    expect(buf.frames()).toBe(16);
    expect(buf.levels().peak).toBeCloseTo(0.2, 12);

    const punch = validatePunch(4, 8);
    const take = punchTakeClip("take_vox_p1", "trk_vox", "Vox p1", "Audio", punch);
    expect(take.kind).toBe("Audio");
    expect(take.source).toBe("take:take_vox_p1");
    const op = takeCommitOp("ui", take);
    expect(() => OpSchema.parse(op)).not.toThrow();
    expect(op.kind).toBe("ClipAdded");
    const carried = ClipSchema.parse(JSON.parse(op.value_json as string));
    expect(carried.source).toBe("take:take_vox_p1");
  });
});
