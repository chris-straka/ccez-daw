/**
 * Command-palette logic (Track K).
 *
 * Pure functions over the action registry: `filterActions` fuzzy-matches the
 * query against every registered action, and `runPaletteAction` runs any of
 * them by id. The Solid component in `Palette.tsx` is a thin view over these;
 * headless tests drive these directly, so replay proves the palette can run
 * every action in `contracts/action-registry.md`.
 */
import { ACTIONS, type ActionDef, findAction } from "../actions/registry";

export function filterActions(query: string, actions: readonly ActionDef[] = ACTIONS): ActionDef[] {
  const q = query.trim().toLowerCase();
  if (q === "") return [...actions];
  const scored: Array<{ a: ActionDef; score: number }> = [];
  for (const a of actions) {
    const s = fuzzyScore(q, `${a.title} ${a.id}`.toLowerCase());
    if (s !== null) scored.push({ a, score: s });
  }
  scored.sort((x, y) => x.score - y.score);
  return scored.map((s) => s.a);
}

/**
 * Subsequence fuzzy match. Score = total skipped characters (lower is
 * better); a query whose characters do not appear in order does not match.
 */
export function fuzzyScore(query: string, haystack: string): number | null {
  let qi = 0;
  let skipped = 0;
  let lastHit = -1;
  for (let hi = 0; hi < haystack.length && qi < query.length; hi++) {
    if (haystack[hi] === query[qi]) {
      skipped += lastHit === -1 ? hi : hi - lastHit - 1;
      lastHit = hi;
      qi++;
    }
  }
  if (qi < query.length) return null;
  // Prefer matches that start at a word boundary / earlier.
  const firstAt = haystack.indexOf(query[0]);
  return skipped + (firstAt === -1 ? 0 : firstAt * 0.01);
}

export type PaletteRunner = (id: string, args: Record<string, unknown>) => Promise<unknown> | unknown;

export function defaultRunner(id: string, args: Record<string, unknown>): Promise<unknown> {
  const a = findAction(id);
  if (!a) return Promise.reject(new Error(`unknown action: ${id}`));
  return a.run(args);
}

/** Run any registry action by id (every palette row goes through here). */
export function runPaletteAction(
  id: string,
  args: Record<string, unknown> = {},
  runner: PaletteRunner = defaultRunner,
): Promise<unknown> {
  return Promise.resolve(runner(id, args));
}

/** All action ids the palette can run (mirrors the frozen registry table). */
export function paletteActionIds(actions: readonly ActionDef[] = ACTIONS): string[] {
  return actions.map((a) => a.id);
}
