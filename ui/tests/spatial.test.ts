import { describe, expect, test } from "bun:test";
import type { Node, Project } from "../src/generated/project";
import { deviceClassOf, deviceParamOps } from "../src/devices/model";
import { sampleProject } from "../src/project/sample";
import {
  decodeStereo,
  encodeMono,
  encodeStereo,
  insertGain,
  monitorStereo,
  spatialDeviceForTrack,
  spatialParamsOf,
  trackMonitorStereo,
  trackSpatialParams,
} from "../src/mixer/spatial";

/** Stub a frozen Spatial device node the way Rust `instantiate` does. */
function spatialNode(id: string, azimuth: number, elevation: number, gain: number): Node {
  const param = (pid: string, value: number, min: number, max: number) => ({
    id: pid,
    label: pid,
    value,
    min,
    max,
    default: min,
    unit: "",
  });
  return {
    id,
    kind: "Device",
    name: id,
    params: [
      { id: "device_class", label: "Device class", value: 11, min: 0, max: 255, default: 11, unit: "" },
      param("azimuth", azimuth, -180, 180),
      param("elevation", elevation, -90, 90),
      param("gain", gain, 0, 4),
    ],
  };
}

function desk(): Project {
  const p = sampleProject() as Project;
  p.devices = [spatialNode("sp_click", 90, 0, 1)];
  const track = p.tracks.find((t) => t.id === "trk_click");
  if (!track) throw new Error("sample project needs trk_click");
  track.device_ids = ["sp_click"];
  return p;
}

describe("ambisonic encode (exactness)", () => {
  test("encodeMono pins the cardinal points", () => {
    expect(encodeMono(1, 0, 0, 1)).toEqual([1, 1, 0, 0]);
    const [w, x, y, z] = encodeMono(1, 90, 0, 1);
    expect(w).toBeCloseTo(1, 9);
    expect(x).toBeCloseTo(0, 9);
    expect(y).toBeCloseTo(1, 9);
    expect(z).toBeCloseTo(0, 9);
    const rear = encodeMono(1, 180, 0, 1);
    expect(rear[1]).toBeCloseTo(-1, 9);
    expect(encodeMono(1, 0, 90, 1)[3]).toBeCloseTo(1, 9);
    expect(encodeMono(0.5, 0, 0, 2)).toEqual([1, 1, 0, 0]);
    expect(encodeMono(0, 33, 12, 4)).toEqual([0, 0, 0, 0]);
  });

  test("encodeStereo collapses to the mono mid at zero spread", () => {
    for (const [l, r] of [[1, 0], [0.5, -0.25], [-1, -1]] as const) {
      const got = encodeStereo(l, r, 30, 10, 2, 0);
      const want = encodeMono((l + r) / 2, 30, 10, 2);
      for (let i = 0; i < 4; i++) expect(got[i]).toBeCloseTo(want[i], 9);
    }
    expect(encodeStereo(0.8, 0.8, 0, 0, 1, 0)).toEqual(encodeMono(0.8, 0, 0, 1));
  });
});

describe("ambisonic decode (exactness + roundtrip)", () => {
  test("decodeStereo places sources exactly", () => {
    expect(decodeStereo([1, 1, 0, 0])).toEqual([0.75, 0.75]);
    expect(decodeStereo([1, 0, 1, 0])).toEqual([1, 0]);
    expect(decodeStereo([1, 0, -1, 0])).toEqual([0, 1]);
    expect(decodeStereo([1, -1, 0, 0])).toEqual([0.25, 0.25]);
    expect(decodeStereo([0, 0, 0, 0])).toEqual([0, 0]);
  });

  test("roundtrip holds sum and difference identities", () => {
    for (const [az, el] of [[0, 0], [90, 0], [-45, 20], [180, -30], [30, 80]] as const) {
      const wxyz = encodeMono(0.7, az, el, 1.5);
      const [l, r] = decodeStereo(wxyz);
      expect(l + r).toBeCloseTo(wxyz[0] + wxyz[1] / 2, 9);
      expect(l - r).toBeCloseTo(wxyz[2], 9);
    }
    const front = decodeStereo(encodeMono(1, 0, 0, 1));
    const rear = decodeStereo(encodeMono(1, 180, 0, 1));
    expect(front[0] + front[1]).toBeGreaterThan(rear[0] + rear[1]);
    const top = decodeStereo(encodeMono(1, 0, 90, 1));
    expect(top[0]).toBe(top[1]);
  });

  test("insertGain pins front/rear and clamps degrees", () => {
    expect(insertGain(0, 0, 1)).toBeCloseTo(1.5, 9);
    expect(insertGain(180, 0, 1)).toBeCloseTo(0.5, 9);
    expect(insertGain(90, 0, 1)).toBeCloseTo(1, 9);
    expect(insertGain(0, 0, 2)).toBeCloseTo(3, 9);
    expect(insertGain(999, 0, 1)).toBeCloseTo(0.5, 9);
  });
});

describe("per-track spatial params (frozen Node + ParamSet, no schema)", () => {
  test("deviceClassOf reads the spatial tag", () => {
    expect(deviceClassOf(spatialNode("sp", 0, 0, 1))).toBe("spatial");
  });

  test("params read with defaults; edits become clamped ParamSet ops", () => {
    expect(spatialParamsOf(null)).toEqual({ azimuth: 0, elevation: 0, gain: 1 });
    expect(spatialParamsOf({ params: [] })).toEqual({ azimuth: 0, elevation: 0, gain: 1 });
    const node = spatialNode("sp", 0, 0, 1);
    expect(spatialParamsOf(node)).toEqual({ azimuth: 0, elevation: 0, gain: 1 });
    const [op] = deviceParamOps("ui", node, { azimuth: -30 });
    expect(op.kind).toBe("ParamSet");
    expect(op.target).toBe("sp:azimuth");
    expect(JSON.parse(op.value_json)).toBe(-30);
    // Clamped to the node's own range; unknown ids throw.
    const [clamped] = deviceParamOps("ui", node, { gain: 99 });
    expect(JSON.parse(clamped.value_json)).toBe(4);
    expect(() => deviceParamOps("ui", node, { azimth: 1 })).toThrow();
  });

  test("track lookup follows device_ids; unknown tracks throw", () => {
    const p = desk();
    expect(spatialDeviceForTrack(p, "trk_click")?.id).toBe("sp_click");
    expect(spatialDeviceForTrack(p, "trk_music")).toBeUndefined();
    expect(trackSpatialParams(p, "trk_click")).toEqual({ azimuth: 90, elevation: 0, gain: 1 });
    expect(trackSpatialParams(p, "trk_music")).toEqual({ azimuth: 0, elevation: 0, gain: 1 });
    expect(() => spatialDeviceForTrack(p, "nope")).toThrow();
  });

  test("track monitor places hard-left and centers by default", () => {
    const p = desk();
    const { left, right } = trackMonitorStereo(p, "trk_click", [1, 1, 1, 1]);
    expect(left).toEqual([1, 1, 1, 1]);
    expect(right).toEqual([0, 0, 0, 0]);
    const center = trackMonitorStereo(p, "trk_music", [1, 1]);
    expect(center.left).toEqual([0.75, 0.75]);
    expect(center.right).toEqual([0.75, 0.75]);
    const silent = monitorStereo([], { azimuth: 90, elevation: 0, gain: 1 });
    expect(silent.left).toEqual([]);
    expect(silent.right).toEqual([]);
  });
});
