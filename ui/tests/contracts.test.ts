import { describe, expect, test } from "bun:test";
import { ACTIONS } from "../src/actions/registry";
import { COMMAND_NAMES, EVENTS } from "../src/generated/ipc";
import { ProjectSchema } from "../src/generated/project";
import { parseSample } from "../src/project/sample";

describe("v0 contracts", () => {
  test("sample project validates against the generated Zod schema", () => {
    const p = parseSample();
    expect(p.tracks).toHaveLength(2);
    expect(p.clips).toHaveLength(2);
    expect(p.schema_version).toBe(0);
  });

  test("generated schema rejects a bad project", () => {
    const bad = { ...parseSample(), tempo: "fast" };
    expect(() => ProjectSchema.parse(bad)).toThrow();
  });

  test("every IPC registry action points at a real IPC command", () => {
    const valid = new Set<string>(COMMAND_NAMES as unknown as string[]);
    for (const a of ACTIONS) {
      if (a.kind === "local") {
        // UI-only actions (palette.open, vim.*) are frozen as `— (local)`.
        expect(a.ipc).toBe("local");
      } else {
        expect(valid.has(a.ipc)).toBe(true);
      }
    }
  });

  test("every frozen action id has a registry entry (Track K owns the full registry)", () => {
    const ids = new Set(ACTIONS.map((a) => a.id));
    for (const id of [
      "transport.play",
      "transport.stop",
      "project.new",
      "project.get",
      "project.open",
      "project.save",
      "project.undo",
      "project.redo",
      "op.apply",
      "track.add",
      "track.list",
      "clip.add",
      "param.set",
      "engine.set_tempo",
      "palette.open",
      "vim.mode.normal",
      "vim.mode.insert",
      "vim.mode.visual",
    ]) {
      expect(ids.has(id)).toBe(true);
    }
  });

  test("palette-critical commands are wired to actions", () => {
    const wired = new Set(ACTIONS.map((a) => a.ipc));
    for (const name of [
      "engine_play",
      "engine_stop",
      "project_new",
      "project_save",
      "op_undo",
      "op_redo",
    ]) {
      expect(wired.has(name)).toBe(true);
    }
  });

  test("event table is frozen at four v0 events", () => {
    expect(Object.keys(EVENTS)).toEqual([
      "project_changed",
      "engine_state_changed",
      "param_changed",
      "op_applied",
    ]);
  });
});
