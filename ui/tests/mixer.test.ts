import { describe, expect, test } from "bun:test";
import { sampleProject } from "../src/project/sample";
import {
  captureSnapshot,
  dbToGain,
  effectiveTrackGain,
  gainToDb,
  isReferenceTrack,
  loudnessDb,
  makeGroup,
  matchGainFor,
  recallSnapshot,
  referenceTracks,
  rms,
  snapshotDiff,
  strips,
  vcaTrim,
} from "../src/mixer/model";

function desk() {
  const p = sampleProject();
  p.routing = [
    { id: "e1", from_node: "trk_click", from_port: "out", to_node: "bus_rhythm", to_port: "in", kind: "Audio" },
    { id: "e2", from_node: "trk_music", from_port: "out", to_node: "bus_rhythm", to_port: "in", kind: "Audio" },
    { id: "e3", from_node: "bus_rhythm", from_port: "out", to_node: "master", to_port: "in", kind: "Audio" },
  ];
  p.devices = [
    { id: "bus_rhythm", kind: "Bus", name: "Rhythm", params: [
      { id: "gain", label: "Gain", value: 0.5, min: 0, max: 4, default: 1, unit: "x" },
    ] },
  ];
  const groups = [makeGroup("g", "All", ["trk_click", "trk_music"], -6)];
  return { p, groups };
}

describe("mixer model", () => {
  test("strips are a view over Audio edges", () => {
    const { p } = desk();
    const map = new Map(strips(p).map((s) => [s.id, s.out]));
    expect(map.get("trk_click")).toBe("bus_rhythm");
  });

  test("snapshot null-test: capture→recall is identity", () => {
    const { p, groups } = desk();
    const before = JSON.stringify({ p, groups });
    const snap = captureSnapshot(p, groups, "A");
    recallSnapshot(p, groups, snap);
    expect(JSON.stringify({ p, groups })).toBe(before);
    expect(snapshotDiff(snap, captureSnapshot(p, groups, "A"))).toEqual([]);
  });

  test("recall restores edited faders; diff names the strip", () => {
    const { p, groups } = desk();
    const snap = captureSnapshot(p, groups, "A");
    p.tracks[0].volume = 0.1;
    recallSnapshot(p, groups, snap);
    expect(p.tracks[0].volume).toBeCloseTo(0.8, 9);
    const b = captureSnapshot(p, groups, "B");
    p.tracks[1].pan = 0.5;
    expect(snapshotDiff(b, captureSnapshot(p, groups, "C"))).toEqual(["trk_music"]);
  });

  test("VCA trim and nested bus gain multiply", () => {
    const { p, groups } = desk();
    expect(vcaTrim(groups, "trk_click")).toBeCloseTo(dbToGain(-6), 9);
    expect(effectiveTrackGain(p, groups, "trk_click")).toBeCloseTo(0.8 * dbToGain(-6) * 0.5, 9);
  });

  test("loudness match equalizes A/B; silence is the safe no-op", () => {
    const a = new Array(64).fill(0.5);
    const b = new Array(64).fill(0.25);
    expect(loudnessDb(a) - loudnessDb(b)).toBeCloseTo(6.0206, 2);
    expect(matchGainFor(a, b)).toBeCloseTo(2, 6);
    expect(matchGainFor([], b)).toBe(1);
    expect(rms([])).toBe(0);
    expect(gainToDb(0)).toBe(-120);
  });

  test("reference convention: never in the mix list", () => {
    const { p } = desk();
    expect(isReferenceTrack("ref_mix", "anything")).toBe(true);
    expect(isReferenceTrack("x", "[REF] Commercial")).toBe(true);
    expect(referenceTracks(p)).toEqual([]);
    p.tracks.push({
      id: "ref_commercial", name: "[REF] Commercial", volume: 0.9, pan: 0,
      muted: false, solo: false, clip_ids: [], device_ids: [],
    });
    expect(referenceTracks(p)).toEqual(["ref_commercial"]);
  });
});
