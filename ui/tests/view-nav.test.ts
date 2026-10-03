import { describe, expect, test } from "bun:test";
import { ACTIONS } from "../src/actions/registry";
import {
  DEFAULT_BINDINGS,
  createKeymap,
  findConflicts,
  resolveAction,
} from "../src/input/keybindings";
import { createVimStore, replayKeys, stubEffects } from "../src/input/vim";
import {
  contextForTab,
  elementIdForFocusAction,
  findSectionByName,
  sectionNameForGotoAction,
  tabForFocusAction,
} from "../src/input/view-nav";
import { sampleTimelineDoc } from "../src/timeline/model";

describe("view-nav mappings (dead-shortcut wiring)", () => {
  test("focus actions map to workspace tabs (mixer is the side column, no tab)", () => {
    expect(tabForFocusAction("view.focusArrangement")).toBe("Timeline");
    expect(tabForFocusAction("view.focusPianoRoll")).toBe("Piano roll");
    expect(tabForFocusAction("view.focusSession")).toBe("Session");
    expect(tabForFocusAction("view.focusMixer")).toBeNull();
    expect(tabForFocusAction("nope.missing")).toBeUndefined();
  });

  test("focus actions map to focusable element ids", () => {
    expect(elementIdForFocusAction("view.focusArrangement")).toBe("timeline-view");
    expect(elementIdForFocusAction("view.focusPianoRoll")).toBe("piano-view");
    expect(elementIdForFocusAction("view.focusMixer")).toBe("mixer-view");
    expect(elementIdForFocusAction("view.focusSession")).toBe("session-view");
    expect(elementIdForFocusAction("nope.missing")).toBeUndefined();
  });

  test("workspace tabs map to vim key contexts", () => {
    expect(contextForTab("Timeline")).toBe("arrangement");
    expect(contextForTab("Piano roll")).toBe("piano-roll");
    expect(contextForTab("Session")).toBe("arrangement");
    expect(contextForTab("Anything else")).toBe("arrangement");
  });

  test("goto actions map to section names; lookup is case-insensitive", () => {
    expect(sectionNameForGotoAction("section.goto.chorus")).toBe("Chorus");
    expect(sectionNameForGotoAction("section.goto.verse")).toBe("Verse");
    expect(sectionNameForGotoAction("view.focusMixer")).toBeUndefined();
    const sections = sampleTimelineDoc().sections;
    expect(findSectionByName(sections, "Chorus")?.start_beats).toBe(16);
    expect(findSectionByName(sections, "verse")?.start_beats).toBe(0);
    expect(findSectionByName(sections, "sec_chorus")?.start_beats).toBe(16);
    expect(findSectionByName(sections, "Bridge")).toBeUndefined();
  });
});

describe("no dead bindings", () => {
  test("every default binding action is a registered action", () => {
    const ids = new Set(ACTIONS.map((a) => a.id));
    for (const b of DEFAULT_BINDINGS) {
      expect(ids.has(b.action)).toBe(true);
    }
  });

  test("? opens the shortcuts editor and the table stays conflict-free", () => {
    const bindings = createKeymap();
    expect(resolveAction(bindings, "normal", "arrangement", "?", {})).toBe(
      "help.show_shortcuts",
    );
    expect(findConflicts(bindings)).toEqual([]);
  });

  test("g-chords reach the shell as dispatched actions", () => {
    const bindings = createKeymap();
    const store = createVimStore({ tracks: 4, steps: 64 });
    const effects = stubEffects();
    replayKeys(store, ["g", "m"], bindings, effects);
    replayKeys(store, ["g", "c"], bindings, effects);
    expect(effects.log).toEqual(["view.focusMixer", "section.goto.chorus"]);
  });
});
