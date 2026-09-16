import { describe, expect, test } from "bun:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { InMemoryBackend } from "../src/backend";
import { createServer } from "../src/index";
import { COVERAGE_TOOL_NAMES, TOOLS, createToolHandlers } from "../src/tools";

function textOf(result: unknown): any {
  const content = (result as any).content;
  expect(Array.isArray(content)).toBe(true);
  expect(content[0].type).toBe("text");
  return JSON.parse(content[0].text);
}

async function linkedClient(backend: InMemoryBackend): Promise<Client> {
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const server = createServer(backend);
  await server.connect(serverT);
  const client = new Client({ name: "coverage-test", version: "0.0.0" });
  await client.connect(clientT);
  return client;
}

async function call(client: Client, name: string, args: Record<string, unknown>): Promise<any> {
  return textOf(await client.callTool({ name, arguments: args }));
}

function handlers(backend = new InMemoryBackend()) {
  return { backend, h: createToolHandlers(backend) };
}

describe("post-v0 coverage tool rows (additive)", () => {
  test("eight coverage tools appended after the GA-5 rows", () => {
    expect(TOOLS.map((t) => t.name).slice(10)).toEqual([...COVERAGE_TOOL_NAMES]);
    for (const t of TOOLS.slice(10)) {
      expect(t.action).toContain(".");
    }
  });

  test("every coverage tool mirrors its registry action id", () => {
    const byName = new Map(TOOLS.map((t) => [t.name, t.action]));
    expect(byName.get("automation_set_point")).toBe("automation.point_set");
    expect(byName.get("session_launch")).toBe("session.launch");
    expect(byName.get("session_jam_record")).toBe("session.jam_record");
    expect(byName.get("comp_commit")).toBe("comp.commit");
    expect(byName.get("groove_apply")).toBe("groove.apply");
    expect(byName.get("branch_merge")).toBe("branch.merge");
    expect(byName.get("record_punch")).toBe("record.punch");
    expect(byName.get("link_join")).toBe("link.join");
  });
});

describe("coverage handlers: mutations land as mcp ops", () => {
  test("automation_set_point creates a lane and logs AutomationPointSet", async () => {
    const { backend, h } = handlers();
    const out = (await h.automation_set_point({
      lane: "trk_1:volume",
      beat: 4,
      value: 0.5,
      node: "trk_1",
      param: "volume",
    })) as { seq: number };
    expect(out.seq).toBe(1);
    expect(backend.getOpLog()[0]).toMatchObject({
      actor: "mcp",
      kind: "AutomationPointSet",
      target: "trk_1:volume",
    });
    expect(() =>
      backend.setAutomationPoint("nope:pan", 0, 0.5),
    ).toThrow("unknown lane");
    expect(() => backend.setAutomationPoint("trk_1:volume", -1, 0.5)).toThrow();
  });

  test("session_launch plans without writing; session_jam_record writes ClipAdded", async () => {
    const { backend, h } = handlers();
    const before = backend.getOpLog().length;
    const plan = (await h.session_launch({ pressBeat: 5, gridBeats: 4 })) as {
      startBeat: number;
    };
    expect(plan).toEqual({ startBeat: 8 });
    expect(backend.getOpLog()).toHaveLength(before);
    expect(() => backend.planLaunch(1, 0)).toThrow("gridBeats");
    const jam = (await h.session_jam_record({
      clips: [{ trackId: "trk_1", name: "Jam", startBeats: 8, lengthBeats: 4, kind: "Midi" }],
    })) as { clipIds: string[]; seqs: number[] };
    expect(jam.clipIds).toHaveLength(1);
    expect(jam.seqs).toHaveLength(1);
    expect(backend.getOpLog().at(-1)?.kind).toBe("ClipAdded");
  });

  test("comp_commit stamps comp:<takes> source; record_punch stamps take:<id>", async () => {
    const { backend, h } = handlers();
    const comp = (await h.comp_commit({
      trackId: "trk_1",
      name: "Keeper",
      startBeats: 0,
      lengthBeats: 8,
      kind: "Audio",
      takes: ["take_a", "take_b"],
    })) as { clipId: string };
    const compClip = backend.getProject().clips.find((c) => c.id === comp.clipId);
    expect(compClip?.source).toBe("comp:take_a+take_b");
    expect(() => backend.commitComp({ trackId: "trk_1", name: "X", startBeats: 0, lengthBeats: 1, kind: "Audio", takes: [] })).toThrow(
      "takes",
    );
    const punch = (await h.record_punch({
      trackId: "trk_1",
      name: "Punch",
      kind: "Audio",
      startBeats: 4,
      endBeats: 8,
    })) as { clipId: string };
    const take = backend.getProject().clips.find((c) => c.id === punch.clipId);
    expect(take?.source).toBe(`take:${punch.clipId}`);
    expect(take?.start_beats).toBe(4);
    expect(take?.length_beats).toBe(4);
    expect(() => backend.commitPunch({ trackId: "trk_1", name: "X", kind: "Audio", startBeats: 8, endBeats: 8 })).toThrow(
      "endBeats",
    );
  });

  test("groove_apply stamps a grooved copy; amount validated", async () => {
    const { backend, h } = handlers();
    const added = backend.addClip({ trackId: "trk_1", name: "Beat", startBeats: 0, lengthBeats: 4, kind: "Midi" });
    const out = (await h.groove_apply({ clipId: added.clipId, groove: "Swing 16", amount: 0.5 })) as {
      clipId: string;
    };
    expect(out.clipId).toBe(`${added.clipId}~groove-swing-16`);
    const grooved = backend.getProject().clips.find((c) => c.id === out.clipId);
    expect(grooved?.source).toContain("groove:swing-16+");
    expect(() => backend.applyGroove(added.clipId, "x", 2)).toThrow("amount");
    expect(() => backend.applyGroove("missing", "x", 0.5)).toThrow("unknown clip");
  });

  test("branch_merge applies clean ops, surfaces same-target conflicts", async () => {
    const { backend, h } = handlers();
    backend.setParam("trk_1", "volume", 0.8);
    const conflict = (await h.branch_merge({
      source: "feature",
      target: "main",
      sourceOps: [{ kind: "ParamSet", target: "trk_1:volume", value_json: "0.2" }],
    })) as { merged: number[]; conflicts: string[] };
    expect(conflict.merged).toEqual([]);
    expect(conflict.conflicts).toEqual(["trk_1:volume"]);
    // Identical effect is a no-op, not a conflict.
    const same = (await h.branch_merge({
      source: "feature",
      target: "main",
      sourceOps: [{ kind: "ParamSet", target: "trk_1:volume", value_json: "0.8" }],
    })) as { merged: number[]; conflicts: string[] };
    expect(same).toEqual({ merged: [], conflicts: [] });
    const clean = (await h.branch_merge({
      source: "feature",
      target: "main",
      sourceOps: [{ kind: "ParamSet", target: "trk_1:pan", value_json: "0.5" }],
    })) as { merged: number[]; conflicts: string[] };
    expect(clean.merged).toHaveLength(1);
    expect(clean.conflicts).toEqual([]);
    expect(backend.getOpLog().at(-1)).toMatchObject({ actor: "mcp", kind: "ParamSet" });
  });

  test("link_join tracks peers/tempo and writes no ops", async () => {
    const { backend, h } = handlers();
    const before = backend.getOpLog().length;
    const one = (await h.link_join({ session: "jam", peer: "laptop", tempo: 128 })) as {
      session: string;
      tempo: number;
      peers: string[];
    };
    expect(one).toEqual({ session: "jam", tempo: 128, peers: ["laptop"] });
    const two = (await h.link_join({ session: "jam", peer: "phone" })) as {
      session: string;
      tempo: number;
      peers: string[];
    };
    expect(two).toEqual({ session: "jam", tempo: 128, peers: ["laptop", "phone"] });
    expect(backend.getOpLog()).toHaveLength(before);
    expect(() => backend.joinLink("jam", "x", 0)).toThrow("tempo");
  });
});

describe("coverage tools over the MCP wire", () => {
  test("client drives the new tools end to end", async () => {
    const backend = new InMemoryBackend();
    const client = await linkedClient(backend);
    const { tools } = await client.listTools();
    expect(tools.map((t) => t.name).slice(10)).toEqual([...COVERAGE_TOOL_NAMES]);

    const point = await call(client, "automation_set_point", {
      lane: "trk_1:volume",
      beat: 2,
      value: 0.7,
      node: "trk_1",
      param: "volume",
    });
    expect(point.seq).toBe(1);
    const plan = await call(client, "session_launch", { pressBeat: 5, gridBeats: 4 });
    expect(plan).toEqual({ startBeat: 8 });
    const link = await call(client, "link_join", { session: "jam", peer: "laptop" });
    expect(link.tempo).toBe(120);
    // Undo reaches the automation op: MCP output stays undoable by design.
    expect(backend.undo()).toEqual({ undoneSeq: 1 });
  });
});
