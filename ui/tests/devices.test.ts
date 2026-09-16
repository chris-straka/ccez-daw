import { describe, expect, test } from "bun:test";
import type { Node } from "../src/generated/project";
import {
  ARP_MODE_OPTIONS,
  CHORD_TYPE_OPTIONS,
  CHORD_VOICING_OPTIONS,
  DEFAULT_PAD_NOTES,
  DRUM_PAD_COUNT,
  bindPadDevice,
  clampToNode,
  defaultDrumRack,
  deviceClassOf,
  deviceParamOps,
  padTrimOps,
  paramValue,
  setPadNote,
  setPadTrack,
} from "../src/devices/model";

/** Stub a frozen device node the way Rust `instantiate` does. */
function stubNode(id: string, classCode: number | null, params: Array<[string, number, number, number]>): Node {
  return {
    id,
    kind: "Device",
    name: id,
    params: [
      ...(classCode === null
        ? []
        : [{ id: "device_class", label: "Device class", value: classCode, min: 0, max: 255, default: classCode, unit: "" }]),
      ...params.map(([pid, value, min, max]) => ({
        id: pid,
        label: pid,
        value,
        min,
        max,
        default: min,
        unit: "",
      })),
    ],
  };
}

const sampler = () =>
  stubNode("smp1", 7, [
    ["transpose", 0, -48, 48],
    ["gain", 1, 0, 4],
    ["attack", 0.005, 0, 10],
    ["release", 0.05, 0, 10],
    ["cutoff", 20000, 20, 20000],
  ]);

describe("device class + param helpers", () => {
  test("deviceClassOf reads the frozen class tag", () => {
    expect(deviceClassOf(sampler())).toBe("sampler");
    expect(deviceClassOf(stubNode("a", 8, []))).toBe("arpeggiator");
    expect(deviceClassOf(stubNode("c", 9, []))).toBe("chord");
    expect(deviceClassOf(stubNode("h", 10, []))).toBe("humanize");
    expect(deviceClassOf(stubNode("old", null, []))).toBe("foreign");
    expect(deviceClassOf(stubNode("weird", 99, []))).toBe("foreign");
  });

  test("paramValue falls back when missing", () => {
    expect(paramValue(sampler(), "gain", 0)).toBe(1);
    expect(paramValue(sampler(), "nope", 7)).toBe(7);
  });

  test("deviceParamOps emits one clamped ParamSet op per param", () => {
    const ops = deviceParamOps("ui", sampler(), { transpose: 12, gain: 99 });
    expect(ops).toHaveLength(2);
    expect(ops[0].kind).toBe("ParamSet");
    expect(ops[0].target).toBe("smp1:transpose");
    expect(ops[0].value_json).toBe("12");
    // Clamped to the node's own max (gain 4), not passed through.
    expect(ops[1].target).toBe("smp1:gain");
    expect(ops[1].value_json).toBe("4");
  });

  test("deviceParamOps rejects unknown param ids", () => {
    expect(() => deviceParamOps("ui", sampler(), { nope: 1 })).toThrow();
  });

  test("clampToNode respects node ranges", () => {
    const n = sampler();
    expect(clampToNode(n, "transpose", 100)).toBe(48);
    expect(clampToNode(n, "attack", -1)).toBe(0);
    expect(clampToNode(n, "missing", 5)).toBe(5);
  });
});

describe("MIDI FX option tables", () => {
  test("arp modes cover codes 0..3", () => {
    expect(ARP_MODE_OPTIONS.map((o) => o.code)).toEqual([0, 1, 2, 3]);
    expect(ARP_MODE_OPTIONS.map((o) => o.label)).toEqual(["up", "down", "up-down", "random"]);
  });

  test("chord types and voicings cover their codes", () => {
    expect(CHORD_TYPE_OPTIONS.map((o) => o.code)).toEqual([0, 1, 2, 3]);
    expect(CHORD_VOICING_OPTIONS.map((o) => o.code)).toEqual([0, 1, 2]);
  });
});

describe("drum rack sidecar", () => {
  test("default rack holds 16 pads on the GM note map", () => {
    const rack = defaultDrumRack("trk_drums");
    expect(rack).toHaveLength(DRUM_PAD_COUNT);
    expect(rack.map((p) => p.note)).toEqual([...DEFAULT_PAD_NOTES]);
    expect(rack[0]).toMatchObject({ pad: 0, trackId: "trk_drums", gain: 1, transpose: 0, deviceId: "" });
  });

  test("setPadNote retargets and rejects out-of-range notes", () => {
    const rack = defaultDrumRack("t");
    const next = setPadNote(rack, 0, 60);
    expect(next[0].note).toBe(60);
    expect(rack[0].note).toBe(36); // immutable update
    expect(() => setPadNote(rack, 0, 128)).toThrow();
    expect(() => setPadNote(rack, 0, -1)).toThrow();
    expect(() => setPadNote(rack, 16, 60)).toThrow();
  });

  test("setPadTrack needs a non-empty track", () => {
    const rack = defaultDrumRack("t");
    expect(setPadTrack(rack, 1, "bass")[1].trackId).toBe("bass");
    expect(() => setPadTrack(rack, 1, "")).toThrow();
  });

  test("padTrimOps is empty until bound, then gain+transpose ParamSets", () => {
    const rack = defaultDrumRack("t");
    expect(padTrimOps("ui", rack[0])).toEqual([]);
    const bound = bindPadDevice(rack, 0, "smp1")[0];
    const ops = padTrimOps("ui", { ...bound, gain: 99, transpose: -99 });
    expect(ops.map((o) => o.target)).toEqual(["smp1:gain", "smp1:transpose"]);
    expect(ops.map((o) => o.value_json)).toEqual(["4", "-48"]);
    for (const op of ops) expect(op.kind).toBe("ParamSet");
  });
});
