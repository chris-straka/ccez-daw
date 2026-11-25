import { describe, expect, test } from "bun:test";
import { AdaptiveCueSchema, SfxBankSchema } from "../src/generated/project";
import { audibleLayers } from "../src/gameaudio/cues";
import {
  cueBoss,
  cueField,
  cueStingerBoss,
  describeFixture,
  fixtureBank,
  fixtureClipIds,
  FIXTURE_CUE_IDS,
  fixtureCues,
  fixtureParams,
  validateFixture,
} from "../src/gameaudio/fixture";

describe("fixture builders", () => {
  test("cues and bank validate against the frozen v1 schemas", () => {
    for (const cue of fixtureCues()) {
      expect(AdaptiveCueSchema.safeParse(cue).success).toBe(true);
    }
    expect(SfxBankSchema.safeParse(fixtureBank()).success).toBe(true);
  });

  test("cue ids match the shipped manifest order", () => {
    expect(fixtureCues().map((c) => c.id)).toEqual([...FIXTURE_CUE_IDS]);
  });

  test("field cue covers field/village/combat/night layers", () => {
    const field = cueField();
    expect(audibleLayers(field, "field").map((l) => l.id).sort()).toEqual(["bed", "strings"]);
    expect(audibleLayers(field, "village").map((l) => l.id).sort()).toEqual(["bed", "strings"]);
    expect(audibleLayers(field, "combat").map((l) => l.id).sort()).toEqual(["bed", "drums"]);
    expect(audibleLayers(field, "night").map((l) => l.id).sort()).toEqual(["bed", "nightpad"]);
  });

  test("boss cue stacks brass + choir on the bed; stinger rule names a real cue", () => {
    const boss = cueBoss();
    expect(audibleLayers(boss, "boss").map((l) => l.id).sort()).toEqual(["bed", "brass", "choir"]);
    const stinger = cueField().transitions.find((t) => t.kind === "Stinger");
    expect(stinger?.stinger_cue_id).toBe("cue_stinger_boss");
    expect(cueStingerBoss().id).toBe("cue_stinger_boss");
  });

  test("params declare the four template sliders", () => {
    expect(fixtureParams().map((p) => p.id)).toEqual(["threat", "time_of_day", "health_low", "mounted"]);
  });
});

describe("validateFixture (authoring-side validator mirror)", () => {
  test("clean fixture reports no problems", () => {
    expect(validateFixture()).toEqual([]);
  });

  test("dangling clip in bank is an error", () => {
    const bank = fixtureBank();
    bank.events[0]!.clip_ids.push("clip_missing");
    const problems = validateFixture(fixtureCues(), bank);
    expect(problems.some((p) => p.includes("clip_missing"))).toBe(true);
  });

  test("unknown game param and unknown mix target are errors", () => {
    const bank = fixtureBank();
    bank.events[0]!.rtpc.push({ param: "wind", target_node: "bus_typo", target_param: "volume", min: 0, max: 1 });
    const problems = validateFixture(fixtureCues(), bank, fixtureParams(), {
      mixTargets: ["bus_sfx:volume", "bus_music:volume", "trk_sfx:volume", "trk_sfx:pan"],
    });
    expect(problems.some((p) => p.includes("`wind`"))).toBe(true);
    expect(problems.some((p) => p.includes("bus_typo:volume"))).toBe(true);
  });

  test("empty pool and duplicate events are errors", () => {
    const bank = fixtureBank();
    bank.events.push({ ...bank.events[0]!, clip_ids: [] });
    const problems = validateFixture(fixtureCues(), bank);
    expect(problems.some((p) => p.includes("empty clip pool"))).toBe(true);
    expect(problems.some((p) => p.includes("duplicate event id"))).toBe(true);
  });

  test("clip inventory covers every referenced id", () => {
    const ids = new Set(fixtureClipIds());
    for (const cue of fixtureCues()) {
      for (const l of cue.layers) for (const c of l.clip_ids) expect(ids.has(c)).toBe(true);
    }
    expect(validateFixture(fixtureCues(), fixtureBank(), fixtureParams(), { clipIds: [...ids] })).toEqual([]);
  });
});

describe("describeFixture", () => {
  test("summarizes cues and RTPC bindings", () => {
    const lines = describeFixture();
    expect(lines[0]).toContain("demo-tp-v1: 3 cues, 4 events, 4 params");
    expect(lines.some((l) => l.includes("threat->bus_sfx:volume"))).toBe(true);
    expect(lines.some((l) => l.includes("mounted->trk_sfx:volume"))).toBe(true);
  });
});
