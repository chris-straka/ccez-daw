import { describe, expect, test } from "bun:test";
import { ACTIONS } from "../src/actions/registry";
import {
  type Binding,
  createKeymap,
  clearPersistedBindings,
  evaluateWhenClause,
  findConflicts,
  loadCustomKeymap,
  loadPersistedBindings,
  matchKeys,
  memoryBindingStore,
  previewRemapConflicts,
  resolveAction,
  savePersistedBindings,
  validateBinding,
} from "../src/input/keybindings";
import {
  diffBindingsAgainstDefaults,
  loadEditorTable,
  persistEditorTable,
  remapAction,
  resetBindings,
  resetEditor,
  searchBindings,
} from "../src/input/editor-model";
import { createVimStore, handleKey, replayKeys, setVimContext, stubEffects } from "../src/input/vim";

const bindings = createKeymap();

function rig() {
  const store = createVimStore({ tracks: 4, steps: 16 });
  store.clipStarts = [4, 8, 12];
  const effects = stubEffects();
  return { store, effects };
}

describe("multi-key chords", () => {
  test("g waits, then gc / gv / ga / gp / gm dispatch", () => {
    const { store, effects } = rig();
    expect(handleKey(store, "g", bindings, effects)).toEqual({ kind: "pending" });
    const out = handleKey(store, "c", bindings, effects);
    expect(out).toEqual({ kind: "dispatched", action: "section.goto.chorus" });
    expect(store.dispatched).toEqual(["section.goto.chorus"]);

    for (const [token, action] of [
      ["v", "section.goto.verse"],
      ["a", "view.focusArrangement"],
      ["p", "view.focusPianoRoll"],
      ["m", "view.focusMixer"],
    ] as const) {
      expect(handleKey(store, "g", bindings, effects)).toEqual({ kind: "pending" });
      expect(handleKey(store, token, bindings, effects)).toEqual({
        kind: "dispatched",
        action,
      });
    }
  });

  test("gg still resolves alongside the new g-chords", () => {
    const { store, effects } = rig();
    store.cursor = { track: 2, step: 5 };
    expect(handleKey(store, "g", bindings, effects)).toEqual({ kind: "pending" });
    expect(handleKey(store, "g", bindings, effects).kind).toBe("moved");
    expect(store.cursor).toEqual({ track: 0, step: 0 });
  });

  test("abandoned chord prefix resets on a non-matching key", () => {
    const { store, effects } = rig();
    expect(handleKey(store, "g", bindings, effects)).toEqual({ kind: "pending" });
    expect(handleKey(store, "z", bindings, effects)).toEqual({ kind: "ignored" });
    // Buffer was reset: a lone motion works again.
    expect(handleKey(store, "j", bindings, effects).kind).toBe("moved");
  });

  test("every chord target is a registered local action", () => {
    const ids = new Set(ACTIONS.map((a) => a.id));
    for (const action of [
      "section.goto.chorus",
      "section.goto.verse",
      "view.focusArrangement",
      "view.focusPianoRoll",
      "view.focusMixer",
      "mixer.muteSelected",
      "mixer.soloSelected",
    ]) {
      expect(ids.has(action)).toBe(true);
    }
  });
});

describe("session view focus", () => {
  test("gs dispatches view.focusSession and the action is registered", () => {
    const { store, effects } = rig();
    expect(handleKey(store, "g", bindings, effects)).toEqual({ kind: "pending" });
    expect(handleKey(store, "s", bindings, effects)).toEqual({
      kind: "dispatched",
      action: "view.focusSession",
    });
    expect(new Set(ACTIONS.map((a) => a.id)).has("view.focusSession")).toBe(true);
  });

  test("the default table stays conflict-free with the session chord", () => {
    expect(findConflicts(createKeymap())).toEqual([]);
  });
});

describe("per-view contexts (arrangement vs piano-roll vs mixer)", () => {
  test("mixer m / M dispatch only in the mixer view", () => {
    const { store, effects } = rig();
    setVimContext(store, "mixer");
    replayKeys(store, ["m", "M"], bindings, effects);
    expect(store.dispatched).toEqual(["mixer.muteSelected", "mixer.soloSelected"]);

    setVimContext(store, "arrangement");
    expect(handleKey(store, "m", bindings, effects)).toEqual({ kind: "ignored" });
    expect(handleKey(store, "M", bindings, effects)).toEqual({ kind: "ignored" });

    setVimContext(store, "piano-roll");
    expect(handleKey(store, "m", bindings, effects)).toEqual({ kind: "ignored" });
  });

  test("setVimContext switches view and clears pending chord state", () => {
    const { store, effects } = rig();
    expect(handleKey(store, "g", bindings, effects)).toEqual({ kind: "pending" });
    setVimContext(store, "mixer");
    expect(store.context).toBe("mixer");
    expect(store.pending).toBe("");
  });

  test("global motions keep working in the mixer view", () => {
    const { store, effects } = rig();
    setVimContext(store, "mixer");
    replayKeys(store, ["l", "j"], bindings, effects);
    expect(store.cursor).toEqual({ track: 1, step: 1 });
  });
});

describe("when-clauses", () => {
  const gated: Binding[] = [
    ...createKeymap(),
    { keys: "x", mode: "normal", context: "all", action: "clip.add", when: "hasSelection" },
    { keys: "X", mode: "normal", context: "all", action: "track.add", when: "!playing" },
    {
      keys: "ctrl+x",
      mode: "normal",
      context: "all",
      action: "project.undo",
      when: "hasSelection && !playing",
    },
  ];

  test("evaluator handles literals, negation, &&, ||, parens", () => {
    expect(evaluateWhenClause(undefined, {})).toBe(true);
    expect(evaluateWhenClause("", {})).toBe(true);
    expect(evaluateWhenClause("hasSelection", { hasSelection: true })).toBe(true);
    expect(evaluateWhenClause("hasSelection", {})).toBe(false);
    expect(evaluateWhenClause("!playing", {})).toBe(true);
    expect(evaluateWhenClause("!playing", { playing: true })).toBe(false);
    expect(evaluateWhenClause("a && b", { a: true, b: true })).toBe(true);
    expect(evaluateWhenClause("a && b", { a: true })).toBe(false);
    expect(evaluateWhenClause("a || b", { b: true })).toBe(true);
    expect(evaluateWhenClause("(a || b) && !c", { a: true, c: true })).toBe(false);
    expect(evaluateWhenClause("(a || b) && !c", { b: true })).toBe(true);
    // Fail closed on garbage.
    expect(evaluateWhenClause("a &&", { a: true })).toBe(false);
    expect(evaluateWhenClause("((a)", { a: true })).toBe(false);
  });

  test("gated bindings resolve only when their clause holds", () => {
    expect(resolveAction(gated, "normal", "arrangement", "x", {})).toBeUndefined();
    expect(resolveAction(gated, "normal", "arrangement", "x", { hasSelection: true })).toBe(
      "clip.add",
    );
    expect(resolveAction(gated, "normal", "arrangement", "X", { playing: true })).toBeUndefined();
    expect(resolveAction(gated, "normal", "arrangement", "X", {})).toBe("track.add");
    expect(
      resolveAction(gated, "normal", "arrangement", "ctrl+x", { hasSelection: true }),
    ).toBe("project.undo");
  });

  test("vim store flags gate replay end to end", () => {
    const { store, effects } = rig();
    store.flags = {};
    expect(handleKey(store, "x", gated, effects)).toEqual({ kind: "ignored" });
    store.flags = { hasSelection: true };
    expect(handleKey(store, "x", gated, effects)).toEqual({
      kind: "dispatched",
      action: "clip.add",
    });
  });

  test("matchKeys prefix logic ignores gated-out bindings", () => {
    const table: Binding[] = [
      { keys: "qx", mode: "normal", context: "all", action: "clip.add", when: "hasSelection" },
    ];
    expect(matchKeys(table, "normal", "arrangement", "q", {})).toEqual({ kind: "none" });
    expect(matchKeys(table, "normal", "arrangement", "q", { hasSelection: true })).toEqual({
      kind: "pending",
    });
  });
});

describe("conflict detection", () => {
  test("the default table is conflict-free", () => {
    expect(findConflicts(createKeymap())).toEqual([]);
  });

  test("same keys in non-overlapping mode/context are not conflicts", () => {
    const table: Binding[] = [
      { keys: "m", mode: "normal", context: "mixer", action: "mixer.muteSelected" },
      { keys: "m", mode: "normal", context: "arrangement", action: "clip.add" },
      { keys: "h", mode: "normal", context: "all", action: "vim.motion.left" },
      { keys: "h", mode: "visual", context: "all", action: "vim.motion.left" },
    ];
    expect(findConflicts(table)).toEqual([]);
  });

  test("same keys + overlapping scope + different actions conflict", () => {
    const table: Binding[] = [
      ...createKeymap(),
      { keys: "u", mode: "normal", context: "arrangement", action: "clip.add" },
    ];
    const conflicts = findConflicts(table);
    expect(conflicts).toHaveLength(1);
    expect(conflicts[0].keys).toBe("u");
    expect(new Set(conflicts[0].actions)).toEqual(new Set(["project.undo", "clip.add"]));
  });

  test("previewRemapConflicts reports without mutating", () => {
    const before = JSON.stringify(bindings);
    const preview = previewRemapConflicts(bindings, "transport.play", "u");
    expect(preview.length).toBeGreaterThan(0);
    expect(preview.some((c) => c.actions.includes("project.undo"))).toBe(true);
    expect(JSON.stringify(bindings)).toBe(before);
    expect(previewRemapConflicts(bindings, "transport.play", "F9")).toEqual([]);
  });
});

describe("shortcuts editor model", () => {
  test("search finds by keys or action id", () => {
    expect(searchBindings(bindings, "gc").map((b) => b.action)).toContain("section.goto.chorus");
    expect(searchBindings(bindings, "mixer").length).toBeGreaterThan(0);
    expect(searchBindings(bindings, "zzz-nope")).toEqual([]);
    expect(searchBindings(bindings, "")).toHaveLength(bindings.length);
  });

  test("remapAction re-keys uniformly and reports conflicts", () => {
    const clean = remapAction(bindings, "transport.play", "F9");
    expect(clean.error).toBeNull();
    expect(clean.conflicts).toEqual([]);
    expect(resolveAction(clean.bindings, "normal", "arrangement", "F9")).toBe("transport.play");

    const clash = remapAction(bindings, "transport.play", "u");
    expect(clash.error).toBeNull();
    expect(clash.conflicts.length).toBeGreaterThan(0);

    const unknown = remapAction(bindings, "nope.missing", "q");
    expect(unknown.error).toContain("unknown action");
    const empty = remapAction(bindings, "transport.play", "");
    expect(empty.error).toContain("non-empty");
  });

  test("resetBindings returns a fresh default table", () => {
    const fresh = resetBindings();
    expect(fresh).toEqual([...createKeymap()]);
    expect(fresh).not.toBe(bindings);
  });

  test("validateBinding accepts Space and rejects bad rows", () => {
    expect(
      validateBinding({ keys: " ", mode: "normal", context: "all", action: "transport.play" }),
    ).toBeNull();
    expect(validateBinding({ keys: "", mode: "normal", context: "all", action: "x" })).not.toBeNull();
    expect(
      validateBinding({ keys: "q", mode: "sideways" as never, context: "all", action: "x" }),
    ).not.toBeNull();
    expect(
      validateBinding({ keys: "q", mode: "normal", context: "all", action: "x", when: "a &&" }),
    ).not.toBeNull();
  });
});

describe("persisted custom bindings", () => {
  test("save / load round-trips overrides + extra", () => {
    const store = memoryBindingStore();
    savePersistedBindings(
      {
        overrides: { "transport.play": "F9" },
        extra: [{ keys: "F8", mode: "normal", context: "mixer", action: "clip.add" }],
      },
      store,
    );
    const loaded = loadPersistedBindings(store);
    expect(loaded.overrides["transport.play"]).toBe("F9");
    expect(loaded.extra).toHaveLength(1);
    const table = loadCustomKeymap(store);
    expect(resolveAction(table, "normal", "arrangement", "F9")).toBe("transport.play");
    expect(resolveAction(table, "normal", "mixer", "F8")).toBe("clip.add");
  });

  test("missing or corrupt storage yields defaults", () => {
    const store = memoryBindingStore();
    expect(loadPersistedBindings(store)).toEqual({ overrides: {}, extra: [] });
    const bad = memoryBindingStore({
      "ccez-daw.keybindings.v1": "{not json",
    });
    expect(loadPersistedBindings(bad)).toEqual({ overrides: {}, extra: [] });
    const junk = memoryBindingStore({
      "ccez-daw.keybindings.v1": JSON.stringify({
        overrides: { "transport.play": "", ok: 42 },
        extra: [{ keys: "", mode: "nope", context: "all", action: "" }, null],
      }),
    });
    expect(loadPersistedBindings(junk)).toEqual({ overrides: {}, extra: [] });
  });

  test("editor remap persists through diff + reload", () => {
    const store = memoryBindingStore();
    const start = loadEditorTable(store);
    const remapped = remapAction(start, "section.goto.chorus", "gC");
    expect(remapped.error).toBeNull();
    persistEditorTable(remapped.bindings, store);
    const reloaded = loadEditorTable(store);
    expect(resolveAction(reloaded, "normal", "arrangement", "gC")).toBe("section.goto.chorus");
    expect(resolveAction(reloaded, "normal", "arrangement", "gc")).toBeUndefined();
    // Reset clears customs.
    const fresh = resetEditor(store);
    expect(resolveAction(fresh, "normal", "arrangement", "gc")).toBe("section.goto.chorus");
    expect(loadPersistedBindings(store)).toEqual({ overrides: {}, extra: [] });
  });

  test("diff of an untouched table persists nothing", () => {
    expect(diffBindingsAgainstDefaults(createKeymap())).toEqual({ overrides: {}, extra: [] });
    clearPersistedBindings(memoryBindingStore());
  });
});
