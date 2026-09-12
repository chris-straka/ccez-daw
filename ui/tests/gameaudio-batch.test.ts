import { describe, expect, test } from "bun:test";
import {
  audibleLayerIds,
  BATCH_STATES,
  checkBatchCompleteness,
  expectedVariants,
  isBatchComplete,
  variantPath,
  type BatchCueView,
  type BatchVariant,
} from "../src/gameaudio/batch";

function cue(): BatchCueView {
  return {
    id: "cue_overworld",
    layers: [
      { id: "bed", states: [] },
      { id: "drums", states: ["combat", "boss"] },
      { id: "pad", states: ["night"] },
    ],
  };
}

function variants(): BatchVariant[] {
  const c = cue();
  return [...BATCH_STATES].flatMap((state) => {
    const layer_ids = audibleLayerIds(c, state);
    return [{ cue_id: c.id, state, path: variantPath(c.id, state), layer_ids, loop_end_beats: 8 }];
  });
}

describe("batch variants", () => {
  test("canonical path shape", () => {
    expect(variantPath("cue_overworld", "night")).toBe("variants/cue_overworld_night.wav");
  });

  test("expected set is cues × TP states", () => {
    expect(expectedVariants(["a", "b"], ["field", "combat"])).toEqual([
      { cueId: "a", state: "field" },
      { cueId: "a", state: "combat" },
      { cueId: "b", state: "field" },
      { cueId: "b", state: "combat" },
    ]);
  });

  test("audible layers follow state gates in cue order", () => {
    const c = cue();
    expect(audibleLayerIds(c, "field")).toEqual(["bed"]);
    expect(audibleLayerIds(c, "combat")).toEqual(["bed", "drums"]);
    expect(audibleLayerIds(c, "night")).toEqual(["bed", "pad"]);
  });

  test("clean batch is complete", () => {
    const report = checkBatchCompleteness(variants(), [cue()], [cue().id], [...BATCH_STATES]);
    expect(isBatchComplete(report)).toBe(true);
  });

  test("missing variant, wrong layers, bad path, bad loop all reported", () => {
    const vs = variants().filter((v) => v.state !== "dungeon");
    vs.push({ cue_id: "cue_overworld", state: "stealth", path: "variants/cue_overworld_stealth.wav", layer_ids: ["bed"], loop_end_beats: 8 });
    const field = vs.find((v) => v.state === "field")!;
    field.layer_ids = ["bed", "drums"];
    const night = vs.find((v) => v.state === "night")!;
    night.path = "stems/wrong.wav";
    const boss = vs.find((v) => v.state === "boss")!;
    boss.loop_end_beats = 0;
    const report = checkBatchCompleteness(vs, [cue()], [cue().id], [...BATCH_STATES]);
    expect(report.missing).toEqual(["cue_overworld:dungeon"]);
    expect(report.extra).toEqual(["cue_overworld:stealth"]);
    expect(report.layerMismatches.map((m) => m.state)).toEqual(["field"]);
    expect(report.pathMismatches.map((m) => m.state)).toEqual(["night"]);
    expect(report.badLoops.map((m) => m.state)).toEqual(["boss"]);
    expect(isBatchComplete(report)).toBe(false);
  });
});
