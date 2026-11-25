import {
  AdaptiveCueSchema,
  CueLayerSchema,
  type AdaptiveCue,
  type CueLayer,
  type TransitionKind,
  type TransitionRule,
} from "../generated/project";

/**
 * GA-3 cue/transition editor model (UI-local, pure functions).
 *
 * Operates only on the frozen v1 `AdaptiveCue` shape
 * (`contracts/adaptive-cue-schema.md`, machine truth
 * `core/src/game_audio.rs`): vertical `CueLayer` stacks plus exact-match
 * `(from_state, to_state)` `TransitionRule`s. Nothing here changes the
 * schema — every constructor emits plain values that validate against the
 * generated `AdaptiveCueSchema`, so `bun run typegen -- --check` stays
 * green. Horizontal timing semantics (bar grids, stinger one-shots) live
 * in `core/src/adaptive/reseq.rs`; this module only edits the rulebook.
 */

export const CUE_SCHEMA_VERSION = 1 as const;

export const TRANSITION_KINDS: TransitionKind[] = ["Cut", "Fade", "BarWait", "Stinger"];

let seq = 0;
function uid(prefix: string): string {
  seq += 1;
  return `${prefix}_${seq}`;
}

/** Blank cue with schema_version 1 and no layers/rules (sparse by design). */
export function makeCue(id: string, name: string, tempo = 120, defaultState = "explore"): AdaptiveCue {
  if (!id) throw new Error("cue id must be non-empty");
  return { schema_version: 1, id, name, tempo, default_state: defaultState, layers: [], transitions: [] };
}

/** Blank vertical layer: empty `states` = always-on bed. */
export function makeLayer(id: string, name: string): CueLayer {
  if (!id) throw new Error("layer id must be non-empty");
  return { id, name, clip_ids: [], states: [], volume: 1 };
}

/** Blank transition rule; kind-specific fields fixed up by `normalizeRule`. */
export function makeTransition(fromState: string, toState: string, kind: TransitionKind = "Cut"): TransitionRule {
  return normalizeRule({ id: uid("trx"), from_state: fromState, to_state: toState, kind, fade_beats: 0, stinger_cue_id: "" });
}

/**
 * Enforce the contract's kind invariants: `stinger_cue_id` is non-empty
 * iff `kind` is `Stinger`; `fade_beats` is meaningful only for `Fade`
 * (zeroed otherwise so stale values never linger).
 */
export function normalizeRule(rule: TransitionRule): TransitionRule {
  const next = { ...rule };
  if (next.kind === "Stinger") {
    next.fade_beats = 0;
    if (!next.stinger_cue_id) next.stinger_cue_id = "stinger_default";
  } else if (next.kind === "Fade") {
    next.stinger_cue_id = "";
    if (!Number.isFinite(next.fade_beats) || next.fade_beats < 0) next.fade_beats = 0;
  } else {
    next.fade_beats = 0;
    next.stinger_cue_id = "";
  }
  return next;
}

/** All known states: default + every layer state + every rule endpoint, sorted. */
export function collectStates(cue: AdaptiveCue): string[] {
  const set = new Set<string>([cue.default_state]);
  for (const l of cue.layers) for (const s of l.states) if (s) set.add(s);
  for (const t of cue.transitions) {
    if (t.from_state) set.add(t.from_state);
    if (t.to_state) set.add(t.to_state);
  }
  return [...set].sort();
}

/** Layers audible in `state` (empty `states` = always-on bed). Mirrors `AdaptiveCue::layers_for_state`. */
export function audibleLayers(cue: AdaptiveCue, state: string): CueLayer[] {
  return cue.layers.filter((l) => l.states.length === 0 || l.states.includes(state));
}

/** Layer matrix: for each layer id, audibility per state. */
export function layerMatrix(cue: AdaptiveCue, states: string[] = collectStates(cue)): Record<string, Record<string, boolean>> {
  const out: Record<string, Record<string, boolean>> = {};
  for (const l of cue.layers) {
    out[l.id] = {};
    for (const s of states) out[l.id][s] = l.states.length === 0 || l.states.includes(s);
  }
  return out;
}

/** Rule for a state change, or null = Cut fallback (sparse graph, per contract). */
export function transitionFor(cue: AdaptiveCue, from: string, to: string): TransitionRule | null {
  return cue.transitions.find((t) => t.from_state === from && t.to_state === to) ?? null;
}

/** Human one-liner for the rule list (null = unwritten Cut fallback). */
export function describeTransition(rule: TransitionRule | null): string {
  if (!rule) return "Cut (no rule — sparse fallback)";
  switch (rule.kind) {
    case "Cut": return "Cut now";
    case "Fade": return `Fade over ${rule.fade_beats} beats`;
    case "BarWait": return "Hold until next bar line, then cut";
    case "Stinger": return `Stinger \`${rule.stinger_cue_id}\`, then cut on its downbeat`;
  }
}

/** All stinger slots: `Stinger` rules and the one-shot each fires. */
export function stingerSlots(cue: AdaptiveCue): { ruleId: string; from: string; to: string; stingerId: string }[] {
  return cue.transitions
    .filter((t) => t.kind === "Stinger")
    .map((t) => ({ ruleId: t.id, from: t.from_state, to: t.to_state, stingerId: t.stinger_cue_id }));
}

function layerById(cue: AdaptiveCue, id: string): CueLayer {
  const l = cue.layers.find((x) => x.id === id);
  if (!l) throw new Error(`unknown layer \`${id}\``);
  return l;
}

/** Toggle one state cell in the layer matrix (bed layers stay bed until given a state). */
export function toggleLayerState(cue: AdaptiveCue, layerId: string, state: string): void {
  const l = layerById(cue, layerId);
  if (!state) throw new Error("state must be non-empty");
  l.states = l.states.includes(state) ? l.states.filter((s) => s !== state) : [...l.states, state];
}

export function setLayerStates(cue: AdaptiveCue, layerId: string, states: string[]): void {
  const l = layerById(cue, layerId);
  if (new Set(states).size !== states.length) throw new Error(`layer \`${layerId}\` has duplicate states`);
  if (states.some((s) => !s)) throw new Error(`layer \`${layerId}\` has an empty state`);
  l.states = [...states];
}

export function setLayerClips(cue: AdaptiveCue, layerId: string, clipIds: string[]): void {
  const l = layerById(cue, layerId);
  if (new Set(clipIds).size !== clipIds.length) throw new Error(`layer \`${layerId}\` has duplicate clip ids`);
  l.clip_ids = [...clipIds];
}

export function setLayerVolume(cue: AdaptiveCue, layerId: string, volume: number): void {
  const l = layerById(cue, layerId);
  if (!Number.isFinite(volume) || volume < 0 || volume > 4) {
    throw new Error(`volume ${volume} out of [0, 4]`);
  }
  l.volume = volume;
}

export function addLayer(cue: AdaptiveCue, layer: CueLayer): void {
  CueLayerRef.parse(layer);
  if (cue.layers.some((l) => l.id === layer.id)) throw new Error(`duplicate layer id \`${layer.id}\``);
  cue.layers.push({ ...layer, clip_ids: [...layer.clip_ids], states: [...layer.states] });
}

export function removeLayer(cue: AdaptiveCue, layerId: string): void {
  const i = cue.layers.findIndex((l) => l.id === layerId);
  if (i < 0) throw new Error(`unknown layer \`${layerId}\``);
  cue.layers.splice(i, 1);
}

const CueLayerRef = CueLayerSchema;

/** Insert or replace the exact `(from, to)` rule; returns the stored rule. */
export function upsertTransition(cue: AdaptiveCue, rule: TransitionRule): TransitionRule {
  if (!rule.id) throw new Error("rule id must be non-empty");
  if (!rule.from_state || !rule.to_state) throw new Error("rule endpoints must be non-empty");
  const next = normalizeRule(rule);
  const dup = cue.transitions.find((t) => t.id === next.id && !(t.from_state === next.from_state && t.to_state === next.to_state));
  if (dup) throw new Error(`duplicate rule id \`${next.id}\``);
  const i = cue.transitions.findIndex((t) => t.from_state === next.from_state && t.to_state === next.to_state);
  if (i >= 0) cue.transitions[i] = next;
  else cue.transitions.push(next);
  return next;
}

export function removeTransition(cue: AdaptiveCue, from: string, to: string): void {
  const i = cue.transitions.findIndex((t) => t.from_state === from && t.to_state === to);
  if (i < 0) throw new Error(`no rule ${from} -> ${to}`);
  cue.transitions.splice(i, 1);
}

export interface CueValidationOptions {
  /** Known v0 clip ids; when given, dangling `clip_ids` are errors. */
  clipIds?: string[];
  /** Known cue/one-shot ids; when given, unknown stinger refs are errors. */
  cueIds?: string[];
}

/** Human-readable problems; empty = valid. Never throws. */
export function validateCue(cue: AdaptiveCue, opts: CueValidationOptions = {}): string[] {
  const problems: string[] = [];
  if (cue.schema_version !== 1) problems.push(`schema_version ${cue.schema_version} must be 1`);
  if (!cue.id) problems.push("cue id must be non-empty");
  if (!Number.isFinite(cue.tempo) || cue.tempo <= 0) problems.push(`tempo ${cue.tempo} must be positive`);
  if (!cue.default_state) problems.push("default_state must be non-empty");
  const layerIds = new Set<string>();
  for (const l of cue.layers) {
    if (!l.id) problems.push("layer id must be non-empty");
    else if (layerIds.has(l.id)) problems.push(`duplicate layer id \`${l.id}\``);
    else layerIds.add(l.id);
    if (!Number.isFinite(l.volume) || l.volume < 0 || l.volume > 4) problems.push(`layer \`${l.id}\` volume ${l.volume} out of [0, 4]`);
    if (new Set(l.clip_ids).size !== l.clip_ids.length) problems.push(`layer \`${l.id}\` has duplicate clip ids`);
    if (opts.clipIds) {
      const known = new Set(opts.clipIds);
      for (const c of l.clip_ids) if (!known.has(c)) problems.push(`layer \`${l.id}\` references unknown clip \`${c}\``);
    }
  }
  const pairs = new Set<string>();
  const ruleIds = new Set<string>();
  for (const t of cue.transitions) {
    if (!t.id) problems.push("rule id must be non-empty");
    else if (ruleIds.has(t.id)) problems.push(`duplicate rule id \`${t.id}\``);
    else ruleIds.add(t.id);
    const pair = `${t.from_state} -> ${t.to_state}`;
    if (pairs.has(pair)) problems.push(`duplicate rule ${pair}`);
    else pairs.add(pair);
    if (!t.from_state || !t.to_state) problems.push(`rule \`${t.id}\` has an empty endpoint`);
    if (t.kind === "Stinger" && !t.stinger_cue_id) problems.push(`rule \`${t.id}\` (Stinger) needs a stinger_cue_id`);
    if (t.kind !== "Stinger" && t.stinger_cue_id) problems.push(`rule \`${t.id}\` (${t.kind}) must not carry a stinger_cue_id`);
    if (t.kind === "Fade" && (!Number.isFinite(t.fade_beats) || t.fade_beats <= 0)) {
      problems.push(`rule \`${t.id}\` (Fade) needs fade_beats > 0`);
    }
    if (t.kind === "Stinger" && opts.cueIds && !opts.cueIds.includes(t.stinger_cue_id)) {
      problems.push(`rule \`${t.id}\` references unknown stinger \`${t.stinger_cue_id}\``);
    }
  }
  return problems;
}

/** Serialize for save/export (Zod-validated before stringify). */
export function serializeCue(cue: AdaptiveCue): string {
  return JSON.stringify(AdaptiveCueSchema.parse(cue));
}

/** Parse back what `serializeCue` wrote; throws a human message on bad input. */
export function parseCue(json: string): AdaptiveCue {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    throw new Error("cue JSON does not parse");
  }
  return AdaptiveCueSchema.parse(raw);
}

/** Deep copy for editor drafts (mutate the copy, validate, then commit). */
export function cloneCue(cue: AdaptiveCue): AdaptiveCue {
  return AdaptiveCueSchema.parse(JSON.parse(JSON.stringify(cue))) as AdaptiveCue;
}
