import { describe, expect, test } from "bun:test";
import { TOOLS } from "../src/tools";

describe("v0 MCP tool list", () => {
  test("six frozen tools, each mirroring one registry action", () => {
    expect(TOOLS.map((t) => t.name)).toEqual([
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
