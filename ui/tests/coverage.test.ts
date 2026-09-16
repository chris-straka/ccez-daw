import { describe, expect, test } from "bun:test";
import { ACTIONS, findAction } from "../src/actions/registry";
import {
  createKeymap,
  findConflicts,
  resolveAction,
} from "../src/input/keybindings";
import { createVimStore, replayKeys, stubEffects } from "../src/input/vim";
import { filterActions, runPaletteAction } from "../src/palette/model";
import { compileScript, describeScriptApi, runScript } from "../src/scripting/api";
import { automationPointSetOp } from "../src/scripting/ops";

const COVERAGE_IDS = [
  "automation.point_set",
  "session.launch",
  "session.jam_record",
  "comp.commit",
  "groove.apply",
  "branch.merge",
  "record.punch",
  "link.join",
] as const;

describe("post-v0 action coverage: one action runs everywhere", () => {
  test("all eight coverage ids are registered", () => {
    const ids = new Set(ACTIONS.map((a) => a.id));
    for (const id of COVERAGE_IDS) expect(ids.has(id)).toBe(true);
  });

  test("automation.point_set is IPC-backed via op_apply; the rest are local", () => {
    const auto = findAction("automation.point_set");
    expect(auto?.kind).toBe("ipc");
    expect(auto?.ipc).toBe("op_apply");
    for (const id of COVERAGE_IDS.slice(1)) {
      const a = findAction(id);
      expect(a?.kind).toBe("local");
      expect(a?.ipc).toBe("local");
    }
  });

  test("local coverage actions resolve to descriptors until wired", async () => {
    for (const id of COVERAGE_IDS.slice(1)) {
      const a = findAction(id);
      await expect(a!.run({})).resolves.toEqual({ local: id, args: {} });
    }
  });

  test("palette finds and runs every coverage action", async () => {
    for (const id of COVERAGE_IDS) {
      const hits = filterActions(id.split(".")[0], ACTIONS);
      expect(hits.map((h) => h.id)).toContain(id);
      // Palette runs through the registry: ipc-backed would invoke Tauri,
      // so run the locals through the default runner here.
      if (id !== "automation.point_set") {
        await expect(runPaletteAction(id, { probe: true })).resolves.toEqual({
          local: id,
          args: { probe: true },
        });
      }
    }
    await expect(runPaletteAction("nope.missing")).rejects.toThrow("unknown action");
  });
});

describe("post-v0 vim coverage", () => {
  test("S/J/C/r/L dispatch the feature actions; table stays conflict-free", () => {
    const bindings = createKeymap();
    expect(resolveAction(bindings, "normal", "arrangement", "S", {})).toBe("session.launch");
    expect(resolveAction(bindings, "normal", "arrangement", "J", {})).toBe("session.jam_record");
    expect(resolveAction(bindings, "normal", "arrangement", "C", {})).toBe("comp.commit");
    expect(resolveAction(bindings, "normal", "arrangement", "r", {})).toBe("record.punch");
    expect(resolveAction(bindings, "normal", "arrangement", "L", {})).toBe("link.join");
    expect(findConflicts(bindings)).toEqual([]);
  });

  test("programmatic actions stay palette/MCP-only (no key claims them)", () => {
    const bindings = createKeymap();
    const keyed = new Set(bindings.map((b) => b.action));
    for (const id of ["automation.point_set", "groove.apply", "branch.merge"]) {
      expect(keyed.has(id)).toBe(false);
    }
  });

  test("vim replay dispatches record punch alongside transport keys", () => {
    const bindings = createKeymap();
    const store = createVimStore({ tracks: 4, steps: 16 });
    const effects = stubEffects();
    replayKeys(store, [" ", "r"], bindings, effects);
    expect(store.dispatched).toEqual(["transport.play", "record.punch"]);
  });
});

describe("post-v0 scripting coverage", () => {
  test("setAutomationPoint emits an AutomationPointSet op", async () => {
    expect(describeScriptApi()).toContain("setAutomationPoint");
    const ops = await compileScript(
      "auto",
      `setAutomationPoint("trk_1:volume", 4, 0.5, "trk_1", "volume");`,
    );
    expect(ops).toHaveLength(1);
    expect(ops[0].kind).toBe("AutomationPointSet");
    expect(ops[0].target).toBe("trk_1:volume");
    expect(ops[0].actor).toBe("script:auto");
    expect(JSON.parse(ops[0].value_json)).toEqual({
      beat: 4,
      value: 0.5,
      node: "trk_1",
      param: "volume",
    });
  });

  test("scripts run end to end through op_apply and stay undo-shaped", async () => {
    let next = 1;
    const result = await runScript(
      "auto",
      `setAutomationPoint("trk_1:volume", 4, 0.5, "trk_1", "volume");`,
      async () => next++,
    );
    expect(result.seqs).toEqual([1]);
    expect(result.ops[0].seq).toBe(0);
  });

  test("automationPointSetOp validates beats and lane creation", () => {
    expect(() => automationPointSetOp("ui", "lane", -1, 0.5, "n", "p")).toThrow();
    expect(() => automationPointSetOp("ui", "lane", 0, 0.5)).toThrow("node:param");
    const update = automationPointSetOp("ui", "lane", 0, 0.5, undefined, undefined, true);
    expect(JSON.parse(update.value_json)).toEqual({ beat: 0, value: 0.5 });
  });
});
