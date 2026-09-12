import { describe, expect, test } from "bun:test";
import { ACTIONS } from "../src/actions/registry";
import { createKeymap, matchKeys, resolveAction } from "../src/input/keybindings";
import { createVimStore, handleKey, replayKeys, stubEffects } from "../src/input/vim";
import { filterActions, paletteActionIds, runPaletteAction } from "../src/palette/model";

const bindings = createKeymap();

function rig() {
  const store = createVimStore({ tracks: 4, steps: 16 });
  store.clipStarts = [4, 8, 12];
  const effects = stubEffects();
  return { store, effects };
}

describe("vim key-sequence replay (headless)", () => {
  test("hjkl + arrows move the arrangement cursor and clamp at edges", () => {
    const { store, effects } = rig();
    const outcomes = replayKeys(store, ["l", "l", "j", "k", "h"], bindings, effects);
    expect(outcomes.every((o) => o.kind === "moved")).toBe(true);
    expect(store.cursor).toEqual({ track: 0, step: 1 });
    expect(store.dispatched).toEqual([]);

    replayKeys(store, ["h", "h", "k", "k"], bindings, effects);
    expect(store.cursor).toEqual({ track: 0, step: 0 });
  });

  test("gg / G / 0 / $ jump to first, last, line start, line end", () => {
    const { store, effects } = rig();
    // "g" alone is a strict prefix of "gg": waits for more keys.
    expect(handleKey(store, "g", bindings, effects)).toEqual({ kind: "pending" });
    expect(handleKey(store, "g", bindings, effects).kind).toBe("moved");
    expect(store.cursor).toEqual({ track: 0, step: 0 });

    replayKeys(store, ["l", "l", "j"], bindings, effects);
    replayKeys(store, ["$"], bindings, effects);
    expect(store.cursor).toEqual({ track: 1, step: 15 });
    replayKeys(store, ["0"], bindings, effects);
    expect(store.cursor).toEqual({ track: 1, step: 0 });
    replayKeys(store, ["G"], bindings, effects);
    expect(store.cursor).toEqual({ track: 3, step: 15 });
  });

  test("w / b jump between clip starts; unknown keys are ignored", () => {
    const { store, effects } = rig();
    replayKeys(store, ["w"], bindings, effects);
    expect(store.cursor.step).toBe(4);
    replayKeys(store, ["w"], bindings, effects);
    expect(store.cursor.step).toBe(8);
    replayKeys(store, ["b"], bindings, effects);
    expect(store.cursor.step).toBe(4);
    expect(handleKey(store, "z", bindings, effects)).toEqual({ kind: "ignored" });
  });

  test("i / v / escape switch modes; motions follow the mode table", () => {
    const { store, effects } = rig();
    expect(handleKey(store, "i", bindings, effects)).toEqual({
      kind: "mode-changed",
      mode: "insert",
    });
    // hjkl are inert in insert mode (no binding applies).
    expect(handleKey(store, "j", bindings, effects)).toEqual({ kind: "ignored" });
    expect(handleKey(store, "escape", bindings, effects)).toEqual({
      kind: "mode-changed",
      mode: "normal",
    });
    expect(handleKey(store, "v", bindings, effects)).toEqual({
      kind: "mode-changed",
      mode: "visual",
    });
    replayKeys(store, ["l"], bindings, effects);
    expect(store.cursor.step).toBe(1);
  });

  test("transport + palette keys dispatch registry actions, not motions", () => {
    const { store, effects } = rig();
    replayKeys(store, [" ", "u", "ctrl+k"], bindings, effects);
    expect(effects.log).toEqual(["transport.play", "project.undo", "palette.open"]);
    expect(store.dispatched).toEqual(["transport.play", "project.undo", "palette.open"]);
  });

  test("piano-roll context shares the motion table", () => {
    const { store, effects } = rig();
    store.context = "piano-roll";
    replayKeys(store, ["j", "l", "w"], bindings, effects);
    expect(store.cursor).toEqual({ track: 1, step: 4 });
  });
});

describe("remappable keybindings", () => {
  test("every binding is overridable by action id", () => {
    const remapped = createKeymap({ "transport.play": "X", "vim.motion.down": "J" });
    expect(resolveAction(remapped, "normal", "arrangement", "X")).toBe("transport.play");
    expect(resolveAction(remapped, "normal", "arrangement", " ")).toBeUndefined();
    expect(resolveAction(remapped, "normal", "arrangement", "J")).toBe("vim.motion.down");
    expect(resolveAction(remapped, "normal", "arrangement", "j")).toBeUndefined();
  });

  test("multi-key remaps participate in prefix matching", () => {
    const remapped = createKeymap({ "vim.motion.first": "tt" });
    expect(matchKeys(remapped, "normal", "arrangement", "t")).toEqual({ kind: "pending" });
    expect(resolveAction(remapped, "normal", "arrangement", "tt")).toBe("vim.motion.first");
    expect(resolveAction(remapped, "normal", "arrangement", "gg")).toBeUndefined();
  });
});

describe("command palette", () => {
  test("empty query lists every registry action", () => {
    expect(filterActions("")).toHaveLength(ACTIONS.length);
  });

  test("fuzzy query finds actions by title or id", () => {
    const ids = filterActions("pla").map((a) => a.id);
    expect(ids).toContain("transport.play");
    expect(filterActions("vimmode").map((a) => a.id)).toContain("vim.mode.normal");
    expect(filterActions("zzz-no-such-action")).toHaveLength(0);
  });

  test("palette can run every registry action (stubbed runner)", () => {
    const ran: string[] = [];
    return (async () => {
      for (const id of paletteActionIds()) {
        await runPaletteAction(id, {}, (rid, args) => {
          ran.push(rid);
          expect(args).toEqual({});
          return Promise.resolve({ stubbed: rid });
        });
      }
      expect(ran.sort()).toEqual(paletteActionIds().sort());
    })();
  });

  test("unknown action ids reject", async () => {
    await expect(runPaletteAction("nope.missing")).rejects.toThrow("unknown action");
  });
});
