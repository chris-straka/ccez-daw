/**
 * View-navigation mapping behind the `g`-chord focus actions and the shell's
 * vim-context sync (dead-shortcuts cluster).
 *
 * Pure and DOM-free: the App shell applies these (switch tab, focus element,
 * move the vim cursor), headless tests drive the same mapping. The mixer is
 * an always-visible side column, not a workspace tab, so
 * `view.focusMixer` maps to a tab of `null` plus a focusable element id.
 */
import type { KeyContext } from "./keybindings";
import type { Section } from "../timeline/model";

/** Workspace tab a focus action selects; `null` = no tab (mixer column). */
export function tabForFocusAction(id: string): string | null | undefined {
  switch (id) {
    case "view.focusArrangement":
      return "Timeline";
    case "view.focusPianoRoll":
      return "Piano roll";
    case "view.focusSession":
      return "Session";
    case "view.focusMixer":
      return null;
    default:
      return undefined;
  }
}

/** Focusable element id a focus action moves DOM focus to. */
export function elementIdForFocusAction(id: string): string | undefined {
  switch (id) {
    case "view.focusArrangement":
      return "timeline-view";
    case "view.focusPianoRoll":
      return "piano-view";
    case "view.focusMixer":
      return "mixer-view";
    case "view.focusSession":
      return "session-view";
    default:
      return undefined;
  }
}

/** Vim key context that follows a workspace tab (mixer focus sets `mixer`). */
export function contextForTab(tab: string): KeyContext {
  return tab === "Piano roll" ? "piano-roll" : "arrangement";
}

/** Section name a `section.goto.*` action seeks; unknown ids stay undefined. */
export function sectionNameForGotoAction(id: string): string | undefined {
  switch (id) {
    case "section.goto.chorus":
      return "Chorus";
    case "section.goto.verse":
      return "Verse";
    default:
      return undefined;
  }
}

/** Case-insensitive section lookup by name or id (for `section.goto.*`). */
export function findSectionByName(
  sections: readonly Section[],
  name: string,
): Section | undefined {
  const want = name.toLowerCase();
  return sections.find((s) => s.name.toLowerCase() === want || s.id.toLowerCase() === want);
}
