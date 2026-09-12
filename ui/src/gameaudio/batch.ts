// Batch stem variants (S-4): one-pass export of all layer/state mixes.
//
// The engine deliverable stays the GA-4 base package (`export.ts`: per-layer
// stems + `package.json`). The batch adds one mix WAV per (cue, state) for
// the TP-like state set plus a single `batch.json` manifest. This module is
// a pure view over that manifest shape — it never edits contracts or
// generated code. Three jobs:
//
// - `variantPath`: where one (cue, state) mix lives (`variants/<cue>_<state>.wav`).
// - `expectedVariants`: the complete (cue × state) set one batch must ship.
// - `checkBatchCompleteness`: missing/extra variants + wrong layer sets.
//
// Mix rendering itself lives in `core/src/batchexport.rs` (sample-sums of
// the shipped layer stems); the browser never renders audio here.

/** TP-like game state set batch exports cover by default. */
export const BATCH_STATES = ["field", "combat", "dungeon", "boss", "village", "night"] as const;
export type BatchState = (typeof BATCH_STATES)[number];
/** Batch manifest filename (package-relative, next to `package.json`). */
export const BATCH_MANIFEST_FILENAME = "batch.json";
/** Package-relative directory holding one mix WAV per (cue, state). */
export const BATCH_VARIANTS_DIR = "variants";

/** Package-relative mix path for one (cue, state) variant. */
export function variantPath(cueId: string, state: string): string {
  return `${BATCH_VARIANTS_DIR}/${cueId}_${state}.wav`;
}

/** One state mix inside a batch manifest (mirrors `BatchVariant` in `batchexport.rs`). */
export interface BatchVariant {
  cue_id: string;
  state: string;
  path: string;
  layer_ids: string[];
  loop_end_beats: number;
}

/** Minimal cue view the completeness check needs (id + layer state gates). */
export interface BatchCueView {
  id: string;
  layers: { id: string; states: string[] }[];
}

/** Layers audible in `state` (empty `states` = always-on bed), in cue order. */
export function audibleLayerIds(cue: BatchCueView, state: string): string[] {
  return cue.layers.filter((l) => l.states.length === 0 || l.states.includes(state)).map((l) => l.id);
}

/** The complete (cue × state) key set one batch must ship. */
export function expectedVariants(cueIds: string[], states: string[]): { cueId: string; state: string }[] {
  const out: { cueId: string; state: string }[] = [];
  for (const cueId of cueIds) for (const state of states) out.push({ cueId, state });
  return out;
}

/** Batch-manifest completeness report. Empty arrays = approved to ship. */
export interface BatchCompleteness {
  /** Expected `cue:state` pairs with no manifest entry. */
  missing: string[];
  /** Manifest entries outside the expected (cue × state) set, as `cue:state`. */
  extra: string[];
  /** Entries whose layer set differs from the audible set, with both sides. */
  layerMismatches: { cueId: string; state: string; have: string[]; want: string[] }[];
  /** Entries living off the canonical `variants/<cue>_<state>.wav` path. */
  pathMismatches: { cueId: string; state: string; path: string; want: string }[];
  /** Entries with a non-finite or non-positive loop length. */
  badLoops: { cueId: string; state: string; loopEndBeats: number }[];
}

/** Check one batch manifest against its cues: completeness + layer sets + paths + loops. */
export function checkBatchCompleteness(
  variants: BatchVariant[],
  cues: BatchCueView[],
  cueIds: string[],
  states: string[],
): BatchCompleteness {
  const cueById = new Map(cues.map((c) => [c.id, c]));
  const expected = new Set(expectedVariants(cueIds, states).map(({ cueId, state }) => `${cueId}:${state}`));
  const seen = new Set<string>();
  const missing: string[] = [];
  const extra: string[] = [];
  const layerMismatches: BatchCompleteness["layerMismatches"] = [];
  const pathMismatches: BatchCompleteness["pathMismatches"] = [];
  const badLoops: BatchCompleteness["badLoops"] = [];
  for (const v of variants) {
    const key = `${v.cue_id}:${v.state}`;
    if (!expected.has(key)) extra.push(key);
    seen.add(key);
    const cue = cueById.get(v.cue_id);
    if (cue) {
      const want = audibleLayerIds(cue, v.state);
      if (JSON.stringify(v.layer_ids) !== JSON.stringify(want)) {
        layerMismatches.push({ cueId: v.cue_id, state: v.state, have: [...v.layer_ids], want });
      }
    }
    if (v.path !== variantPath(v.cue_id, v.state)) {
      pathMismatches.push({ cueId: v.cue_id, state: v.state, path: v.path, want: variantPath(v.cue_id, v.state) });
    }
    if (!(v.loop_end_beats > 0) || !Number.isFinite(v.loop_end_beats)) {
      badLoops.push({ cueId: v.cue_id, state: v.state, loopEndBeats: v.loop_end_beats });
    }
  }
  for (const key of expected) if (!seen.has(key)) missing.push(key);
  missing.sort();
  extra.sort();
  return { missing, extra, layerMismatches, pathMismatches, badLoops };
}

/** True when the report carries no violations. */
export function isBatchComplete(report: BatchCompleteness): boolean {
  return (
    report.missing.length === 0 &&
    report.extra.length === 0 &&
    report.layerMismatches.length === 0 &&
    report.pathMismatches.length === 0 &&
    report.badLoops.length === 0
  );
}
