import { describe, expect, test } from "bun:test";
import { LibraryItemSchema, type Node } from "../src/generated/project";
import { searchLibrary } from "../src/browser/library";
import { seedLibrary } from "../src/browser/seed";
import {
  DEVICE_PARAM_RANGES,
  NATIVE_PRESETS,
  applyPresetToNode,
  findNativePreset,
  presetParamOps,
  seedNativePresetLibrary,
  type NativePreset,
} from "../src/browser/presets";

/** Build a stub device node for a class, the way Rust `instantiate` does. */
function stubNode(preset: NativePreset): Node {
  const code = {
    gain: 1,
    lowpass: 2,
    highpass: 3,
    delay: 4,
    distortion: 5,
    sampler: 7,
    arpeggiator: 8,
    chord: 9,
    humanize: 10,
  }[preset.deviceClass];
  return {
    id: "probe",
    kind: "Device",
    name: preset.name,
    params: [
      { id: "device_class", label: "Device class", value: code, min: 0, max: 255, default: code, unit: "" },
      ...DEVICE_PARAM_RANGES[preset.deviceClass].map((r) => ({
        id: r.id,
        label: r.id,
        value: r.min,
        min: r.min,
        max: r.max,
        default: r.min,
        unit: "",
      })),
    ],
  };
}

describe("native preset library", () => {
  test("every preset applies cleanly to its device class", () => {
    expect(NATIVE_PRESETS.length).toBeGreaterThan(0);
    for (const preset of NATIVE_PRESETS) {
      const node = stubNode(preset);
      expect(() => applyPresetToNode(node, preset)).not.toThrow();
      // Exact values, not clamped ones: proves every id exists and every
      // value sits inside its [min, max].
      for (const [param, value] of Object.entries(preset.params)) {
        const p = node.params.find((q) => q.id === param);
        expect(p).toBeDefined();
        expect(p!.value).toBe(value);
      }
    }
  });

  test("every preset param is a known id inside its range", () => {
    for (const preset of NATIVE_PRESETS) {
      const ranges = DEVICE_PARAM_RANGES[preset.deviceClass];
      for (const [param, value] of Object.entries(preset.params)) {
        const r = ranges.find((q) => q.id === param);
        expect(r).toBeDefined();
        expect(value).toBeGreaterThanOrEqual(r!.min);
        expect(value).toBeLessThanOrEqual(r!.max);
      }
    }
  });

  test("wrong class and unknown ids are loud", () => {
    const lp = findNativePreset("lp_warm_pad")!;
    const gainNode = stubNode(findNativePreset("gain_unity")!);
    expect(() => applyPresetToNode(gainNode, lp)).toThrow();
    expect(findNativePreset("no_such_preset")).toBeUndefined();
    expect(() => presetParamOps("ui", "d1", "no_such_preset")).toThrow();
    const bad: NativePreset = { ...lp, params: { nope: 1 } };
    expect(() => applyPresetToNode(stubNode(lp), bad)).toThrow();
  });

  test("preset stamping expands to undoable ParamSet ops", () => {
    const ops = presetParamOps("ui", "dly1", "dly_dotted_groove");
    const preset = findNativePreset("dly_dotted_groove")!;
    expect(ops).toHaveLength(Object.keys(preset.params).length);
    for (const op of ops) {
      // The existing undoable path: ParamSet on a node:param address.
      expect(op.kind).toBe("ParamSet");
      expect(op.target.startsWith("dly1:")).toBe(true);
      const param = op.target.slice("dly1:".length);
      expect(preset.params[param]).toBeDefined();
      expect(JSON.parse(op.value_json)).toBe(preset.params[param]);
    }
  });

  test("every preset seeds a valid, unique browser item", () => {
    const items = seedNativePresetLibrary();
    expect(items).toHaveLength(NATIVE_PRESETS.length);
    const ids = new Set(items.map((i) => i.id));
    expect(ids.size).toBe(items.length);
    for (const item of items) {
      expect(() => LibraryItemSchema.parse(item)).not.toThrow();
      expect(item.kind).toBe("Preset");
    }
    // No id collisions with the existing demo seeds.
    const seedIds = new Set(seedLibrary().map((i) => i.id));
    for (const id of ids) expect(seedIds.has(id)).toBe(false);
  });

  test("every preset is discoverable through browser search", () => {
    const lib = [...seedLibrary(), ...seedNativePresetLibrary()];
    for (const preset of NATIVE_PRESETS) {
      // Its own name surfaces it in the palette window (topK 8, the
      // palette default): name + tags + description all index it.
      const hits = searchLibrary(preset.name, lib, { topK: 8 });
      expect(hits.map((h) => h.item.id)).toContain(preset.id);
    }
    // A groove concept query surfaces the drunk-drums starting point.
    const groove = searchLibrary("drunk swung drum groove with slouch", lib, { topK: 3 });
    expect(groove.map((h) => h.item.id)).toContain("hum_drunk_drums");
  });
});
