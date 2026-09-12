import { describe, expect, test } from "bun:test";
import { sampleProject } from "../src/project/sample";
import {
  audibleLayers,
  cloneCue,
  collectStates,
  describeTransition,
  layerMatrix,
  makeCue,
  makeLayer,
  normalizeRule,
  parseCue,
  removeTransition,
  serializeCue,
  setLayerClips,
  setLayerVolume,
  stingerSlots,
  toggleLayerState,
  transitionFor,
  upsertTransition,
  validateCue,
} from "../src/gameaudio/cues";

function desk() {
  const p = sampleProject();
  const clipIds = p.clips.map((c) => c.id);
  const cue = makeCue("cue_main", "Main Theme");
  const bed = { ...makeLayer("layer_bed", "Bed"), clip_ids: [clipIds[0] as string], volume: 0.8 };
  const drums = {
    ...makeLayer("layer_drums", "Drums"),
    clip_ids: [clipIds[1] as string],
    states: ["combat"],
    volume: 1,
  };
  cue.layers.push(bed, drums);
  return { cue, clipIds };
}

describe("cue/transition editor", () => {
  test("layer matrix: bed is always on, drums only in combat", () => {
    const { cue } = desk();
    expect(audibleLayers(cue, "explore").map((l) => l.id)).toEqual(["layer_bed"]);
    expect(audibleLayers(cue, "combat").map((l) => l.id).sort()).toEqual(["layer_bed", "layer_drums"]);
    const m = layerMatrix(cue, ["explore", "combat"]);
    expect(m["layer_bed"]).toEqual({ explore: true, combat: true });
    expect(m["layer_drums"]).toEqual({ explore: false, combat: true });
  });

  test("matrix toggle flips one cell", () => {
    const { cue } = desk();
    toggleLayerState(cue, "layer_drums", "explore");
    expect(audibleLayers(cue, "explore").map((l) => l.id).sort()).toEqual(["layer_bed", "layer_drums"]);
    toggleLayerState(cue, "layer_drums", "explore");
    expect(audibleLayers(cue, "explore").map((l) => l.id)).toEqual(["layer_bed"]);
  });

  test("no rule = Cut fallback; upsert replaces the exact pair", () => {
    const { cue } = desk();
    expect(transitionFor(cue, "explore", "combat")).toBeNull();
    expect(describeTransition(transitionFor(cue, "explore", "combat"))).toMatch(/Cut/);
    upsertTransition(cue, {
      id: "trx_a",
      from_state: "explore",
      to_state: "combat",
      kind: "Fade",
      fade_beats: 4,
      stinger_cue_id: "",
    });
    expect(transitionFor(cue, "explore", "combat")?.kind).toBe("Fade");
    upsertTransition(cue, {
      id: "trx_b",
      from_state: "explore",
      to_state: "combat",
      kind: "BarWait",
      fade_beats: 99,
      stinger_cue_id: "junk",
    });
    const rule = transitionFor(cue, "explore", "combat");
    expect(rule?.kind).toBe("BarWait");
    expect(rule?.fade_beats).toBe(0);
    expect(rule?.stinger_cue_id).toBe("");
    expect(cue.transitions).toHaveLength(1);
    removeTransition(cue, "explore", "combat");
    expect(transitionFor(cue, "explore", "combat")).toBeNull();
  });

  test("stinger slots list Stinger rules; validator enforces kind invariants", () => {
    const { cue } = desk();
    // normalizeRule fills a missing stinger id with a placeholder.
    const filled = normalizeRule({
      id: "t",
      from_state: "explore",
      to_state: "combat",
      kind: "Stinger",
      fade_beats: 0,
      stinger_cue_id: "",
    });
    expect(filled.stinger_cue_id).not.toBe("");
    // But a raw Stinger with an empty slot is invalid until armed.
    expect(
      validateCue(
        { ...cue, transitions: [{ id: "t", from_state: "explore", to_state: "combat", kind: "Stinger", fade_beats: 0, stinger_cue_id: "" }] },
        { cueIds: ["sting_brass"] },
      ).join("\n"),
    ).toMatch(/stinger/);
    upsertTransition(cue, {
      id: "trx_s",
      from_state: "explore",
      to_state: "combat",
      kind: "Stinger",
      fade_beats: 0,
      stinger_cue_id: "sting_brass",
    });
    expect(stingerSlots(cue)).toEqual([
      { ruleId: "trx_s", from: "explore", to: "combat", stingerId: "sting_brass" },
    ]);
    expect(validateCue(cue, { clipIds: desk().clipIds, cueIds: ["sting_brass"] })).toEqual([]);
    expect(
      validateCue(cue, { cueIds: ["other"] }).join("\n"),
    ).toMatch(/unknown stinger/);
  });

  test("validator catches dangling clips and bad fades", () => {
    const { cue, clipIds } = desk();
    setLayerClips(cue, "layer_bed", ["nope"]);
    setLayerVolume(cue, "layer_bed", 1);
    expect(validateCue(cue, { clipIds }).join("\n")).toMatch(/unknown clip/);
    setLayerClips(cue, "layer_bed", [clipIds[0] as string]);
    upsertTransition(cue, {
      id: "trx_f",
      from_state: "combat",
      to_state: "explore",
      kind: "Fade",
      fade_beats: 0,
      stinger_cue_id: "",
    });
    expect(validateCue(cue, { clipIds }).join("\n")).toMatch(/fade_beats/);
  });

  test("editor round-trip: serialize → parse is identity and validates", () => {
    const { cue, clipIds } = desk();
    upsertTransition(cue, {
      id: "trx_f",
      from_state: "explore",
      to_state: "combat",
      kind: "Fade",
      fade_beats: 4,
      stinger_cue_id: "",
    });
    upsertTransition(cue, {
      id: "trx_s",
      from_state: "combat",
      to_state: "explore",
      kind: "Stinger",
      fade_beats: 0,
      stinger_cue_id: "sting_brass",
    });
    const back = parseCue(serializeCue(cloneCue(cue)));
    expect(back).toEqual(cue);
    expect(validateCue(back, { clipIds, cueIds: ["sting_brass"] })).toEqual([]);
    expect(collectStates(back).sort()).toEqual(["combat", "explore"]);
  });
});
