import { describe, expect, test } from "bun:test";
import { TOOLS } from "../src/tools";

describe("v0 MCP tool list", () => {
  test("six frozen tools, each mirroring one registry action", () => {
    // Frozen six stay first and untouched; GA-5 appends its rows after.
    expect(TOOLS.map((t) => t.name).slice(0, 6)).toEqual([
      "project_get",
      "project_list_tracks",
      "project_add_clip",
      "param_set",
      "transport_play",
      "transport_stop",
    ]);
    for (const t of TOOLS) {
      expect(t.action.length).toBeGreaterThan(0);
    }
  });
});

describe("GA-5 game-audio tool rows (additive)", () => {
  test("four game-audio tools appended after the frozen six", () => {
    expect(TOOLS.map((t) => t.name).slice(6)).toEqual([
      "gameaudio_list_cues",
      "gameaudio_audition",
      "gameaudio_trigger_sfx",
      "gameaudio_export",
    ]);
    for (const t of TOOLS.slice(6)) {
      expect(t.action.startsWith("gameaudio.")).toBe(true);
    }
  });
});
