import { describe, expect, test } from "bun:test";
import { sampleProject } from "../src/project/sample";
import type { Edge, Project } from "../src/generated/project";
import {
  addEdge,
  cablesFrom,
  cablesTo,
  collectNodeIds,
  edgesOfKind,
  patchFromJson,
  patchToJson,
  reconnectEdge,
  removeEdge,
} from "../src/patch/model";
import { layoutPatch, signalTopoOrder } from "../src/patch/layout";

function edge(id: string, from: string, to: string, kind: Edge["kind"]): Edge {
  return { id, from_node: from, from_port: "out", to_node: to, to_port: "in", kind };
}

/** Patch demo: one cable of each kind over tracks + an implicit mix bus. */
function patchProject(): Project {
  return {
    ...sampleProject(),
    id: "proj_patch",
    name: "Patch Demo",
    devices: [
      { id: "dev_dly", kind: "Device", name: "Delay", params: [] },
      { id: "lfo_1", kind: "Modulator", name: "LFO", params: [] },
    ],
    routing: [
      edge("e1", "trk_click", "dev_dly", "Audio"),
      edge("e2", "dev_dly", "mix", "Audio"),
      edge("e3", "trk_music", "mix", "Audio"),
      edge("e4", "clip_b", "trk_music", "Midi"),
      edge("e5", "lfo_1", "dev_dly", "Modulation"),
      edge("e6", "trk_click", "dev_dly", "Sidechain"),
    ],
  };
}

describe("Track G patch graph over the one routing model", () => {
  test("graph-edit round trip: project -> json -> project preserves routing", () => {
    const project = patchProject();
    // Edit: add a cable, re-patch one end, delete another.
    const added = addEdge(project, edge("e7", "trk_music", "dev_dly", "Audio"));
    expect(added.routing).toHaveLength(7);
    expect(project.routing).toHaveLength(6); // input untouched (immutable)
    const moved = reconnectEdge(added, "e7", { to_port: "in2" });
    expect(moved.routing.find((e) => e.id === "e7")?.to_port).toBe("in2");
    // Re-patching onto an occupied jack is rejected, not silently merged.
    expect(() => reconnectEdge(added, "e7", { to_node: "mix", to_port: "in" })).toThrow(
      "duplicate",
    );
    const { project: cut, removed } = removeEdge(moved, "e6");
    expect(removed).toBe(true);
    expect(cut.routing).toHaveLength(6);
    // Round trip through JSON: identical routing on the other side.
    const back = patchFromJson(cut, patchToJson(cut));
    expect(back.routing).toEqual(cut.routing);
    // And the JSON itself validates row-by-row against the frozen schema.
    expect(JSON.parse(patchToJson(cut))).toHaveLength(6);
  });

  test("remove of an unknown edge is a no-op; reconnect of one throws", () => {
    const project = patchProject();
    const { removed, project: same } = removeEdge(project, "nope");
    expect(removed).toBe(false);
    expect(same.routing).toEqual(project.routing);
    expect(() => reconnectEdge(project, "nope", { to_node: "mix" })).toThrow("unknown edge");
  });

  test("one graph carries all four kinds; mixer views read signal only", () => {
    const project = patchProject();
    expect(edgesOfKind(project, "Audio")).toHaveLength(3);
    expect(edgesOfKind(project, "Midi")).toHaveLength(1);
    expect(edgesOfKind(project, "Modulation")).toHaveLength(1);
    expect(edgesOfKind(project, "Sidechain")).toHaveLength(1);
    // Mix-point inputs and downstream views ignore control-rate cables.
    expect(cablesTo(project, "dev_dly").map((e) => e.id)).toEqual(["e1"]);
    expect(cablesFrom(project, "dev_dly").map((e) => e.id)).toEqual(["e2"]);
    // Implicit endpoints (mix bus, clips) are addressable layout nodes.
    expect(collectNodeIds(project)).toContain("mix");
  });

  test("signal order flows producers-first; modulation loops are legal", () => {
    const project = patchProject();
    const topo = signalTopoOrder(project);
    expect(topo.ok).toBe(true);
    if (topo.ok) {
      const pos = (id: string) => topo.order.indexOf(id);
      expect(pos("trk_click")).toBeLessThan(pos("dev_dly"));
      expect(pos("dev_dly")).toBeLessThan(pos("mix"));
    }
    // Modulation-only feedback lays out fine (control-rate, like core).
    const looped = addEdge(project, edge("m2", "dev_dly", "lfo_1", "Modulation"));
    expect(signalTopoOrder(looped).ok).toBe(true);
    // Audio feedback is a cycle naming its members.
    const cyclic = addEdge(project, edge("loop", "mix", "trk_click", "Audio"));
    const bad = signalTopoOrder(cyclic);
    expect(bad.ok).toBe(false);
    if (!bad.ok) expect(bad.cycle).toContain("mix");
    // The layout still returns (cycle nodes share layer 0, flagged).
    const layout = layoutPatch(cyclic);
    expect(layout.cycle).toContain("mix");
    const flagged = layout.nodes.find((n) => n.id === "mix");
    expect(flagged?.inCycle).toBe(true);
    // Acyclic layout flows left-to-right through the delay.
    const good = layoutPatch(project);
    expect(good.cycle).toEqual([]);
    const x = (id: string) => good.nodes.find((n) => n.id === id)!.x;
    expect(x("trk_click")).toBeLessThan(x("dev_dly"));
    expect(x("dev_dly")).toBeLessThan(x("mix"));
  });

  test("validation rejects drift at the edit boundary", () => {
    const project = patchProject();
    expect(() => addEdge(project, edge("e1", "a", "b", "Audio"))).toThrow("duplicate edge id");
    expect(() =>
      addEdge(project, edge("e9", "trk_click", "dev_dly", "Audio")),
    ).toThrow("duplicate cable");
    expect(() => addEdge(project, edge("e9", "mix", "mix", "Audio"))).toThrow("self-loop");
    expect(() =>
      addEdge(project, { ...edge("e9", "a", "b", "Audio"), kind: "CV" as never }),
    ).toThrow();
    expect(() => patchFromJson(project, "not json")).toThrow("invalid JSON");
    expect(() => patchFromJson(project, '{"id":"e1"}')).toThrow("expected an Edge array");
  });
});
