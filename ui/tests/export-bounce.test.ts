import { describe, expect, test } from "bun:test";
import {
  bounceSampleCount,
  describeBounceConfig,
  enqueueBatchItems,
  enqueueBounceItem,
  enqueuePackageItem,
  parseIdList,
  queueSummary,
  resetQueueIds,
  setItemStatus,
  validateBounceConfig,
} from "../src/export/bounceQueue";

describe("bounce config", () => {
  test("clean config validates (mirrors BounceConfig::new)", () => {
    expect(validateBounceConfig({ sampleRate: 44100, startBeat: 0, lengthBeats: 4 })).toBeNull();
  });

  test("bad rate, start, and length each reported", () => {
    expect(validateBounceConfig({ sampleRate: 0, startBeat: 0, lengthBeats: 4 })).not.toBeNull();
    expect(validateBounceConfig({ sampleRate: -1, startBeat: 0, lengthBeats: 4 })).not.toBeNull();
    expect(validateBounceConfig({ sampleRate: 44100, startBeat: -1, lengthBeats: 4 })).not.toBeNull();
    expect(validateBounceConfig({ sampleRate: 44100, startBeat: 0, lengthBeats: 0 })).not.toBeNull();
    expect(validateBounceConfig({ sampleRate: 44100, startBeat: NaN, lengthBeats: 4 })).not.toBeNull();
    expect(validateBounceConfig({ sampleRate: 44100, startBeat: 0, lengthBeats: Infinity })).not.toBeNull();
  });

  test("sample count is length * 60 / tempo * rate", () => {
    // 4 beats @ 120 BPM at 8 kHz = 2 s = 16000 samples.
    expect(bounceSampleCount({ sampleRate: 8000, startBeat: 0, lengthBeats: 4 }, 120)).toBe(16000);
    expect(bounceSampleCount({ sampleRate: 44100, startBeat: 0, lengthBeats: 4 }, 0)).toBe(0);
  });

  test("summary names the window, rate, and count", () => {
    const s = describeBounceConfig({ sampleRate: 8000, startBeat: 0, lengthBeats: 4 }, 120);
    expect(s).toContain("8000 Hz");
    expect(s).toContain("16000 samples");
  });
});

describe("export queue", () => {
  test("batch enqueue fans out cue × state as queued items", () => {
    resetQueueIds();
    const items = enqueueBatchItems(["a", "b"], ["field", "combat"]);
    expect(items).toHaveLength(4);
    expect(items.map((i) => i.label)).toEqual(["a:field", "a:combat", "b:field", "b:combat"]);
    expect(items.every((i) => i.status === "queued")).toBe(true);
    expect(new Set(items.map((i) => i.id)).size).toBe(4);
  });

  test("bounce and package items enqueue queued", () => {
    resetQueueIds();
    expect(enqueueBounceItem("0–4 beats").kind).toBe("bounce");
    expect(enqueuePackageItem("demo").kind).toBe("package");
    expect(enqueuePackageItem("demo").status).toBe("queued");
  });

  test("status flips and summary counts per-item state", () => {
    resetQueueIds();
    let items = enqueueBatchItems(["a"], ["field", "combat"]);
    items = setItemStatus(items, items[0].id, "ready", "variants/a_field.wav");
    items = setItemStatus(items, items[1].id, "error", "unknown cue `a`");
    expect(queueSummary(items)).toEqual({ queued: 0, ready: 1, errors: 1 });
    // Unknown ids are untouched.
    expect(setItemStatus(items, "nope", "ready", "x")).toEqual(items);
  });

  test("id lists split on commas and whitespace", () => {
    expect(parseIdList("a, b  c,,")).toEqual(["a", "b", "c"]);
    expect(parseIdList("")).toEqual([]);
  });
});
