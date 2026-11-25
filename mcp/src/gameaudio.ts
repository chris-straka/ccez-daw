/**
 * GA-5: game-audio MCP data + evaluation (additive; v0 tools untouched).
 *
 * Shapes mirror the frozen v1 machine truth (`core/src/game_audio.rs`) and
 * the tables in `contracts/game-state.md`, `contracts/adaptive-cue-schema.md`,
 * `contracts/sfx-bank-schema.md`, and `contracts/export-package.md`, using the
 * same snake_case field names so the MCP surface cannot drift from the
 * contracts. Nothing here writes the project: audition/trigger are
 * evaluation-only (plan Key Decision 6); only a green `gameaudio_export`
 * records an op, with actor `gameaudio:export`.
 *
 * The four tool names below are the frozen rows from
 * `contracts/export-package.md` ("Game-audio MCP tools"); each tool's
 * `action` id uses the `gameaudio.*` verb family
 * (`trigger_event`, `audition_cue`, `export_bank`, …) mirroring the
 * action-registry naming style.
 */

/** Schema version stamped on every game-audio document (v1 frozen). */
export const GAME_AUDIO_SCHEMA_VERSION = 1;

/** Validator version recorded on approved packages (`"1"` in v1). */
export const GAME_AUDIO_VALIDATOR_VERSION = "1";

export type TransitionKind = "Cut" | "Fade" | "BarWait" | "Stinger";
export type StemKind = "MusicLayer" | "SfxClip";

export interface GameStateParamDecl {
  id: string;
  label: string;
  min: number;
  max: number;
  default: number;
  unit: string;
}

export interface GameStateValue {
  param: string;
  value: number;
}

export interface GameStateSnapshot {
  state: string;
  values: GameStateValue[];
}

export interface CueLayer {
  id: string;
  name: string;
  clip_ids: string[];
  states: string[];
  volume: number;
}

export interface TransitionRule {
  id: string;
  from_state: string;
  to_state: string;
  kind: TransitionKind;
  fade_beats: number;
  stinger_cue_id: string;
}

export interface AdaptiveCue {
  schema_version: number;
  id: string;
  name: string;
  tempo: number;
  default_state: string;
  layers: CueLayer[];
  transitions: TransitionRule[];
}

export interface RtpcBinding {
  param: string;
  target_node: string;
  target_param: string;
  min: number;
  max: number;
}

export interface SfxEvent {
  id: string;
  name: string;
  clip_ids: string[];
  volume: number;
  volume_random: number;
  pitch_random: number;
  cooldown_ms: number;
  max_polyphony: number;
  rtpc: RtpcBinding[];
}

export interface SfxBank {
  schema_version: number;
  id: string;
  name: string;
  events: SfxEvent[];
}

export interface ExportStem {
  path: string;
  source_id: string;
  source_layer_id: string;
  kind: StemKind;
  loop_start_beats: number;
  loop_end_beats: number;
}

export interface ExportPackage {
  schema_version: number;
  name: string;
  cue_ids: string[];
  bank_ids: string[];
  stems: ExportStem[];
  event_bank_path: string;
  validator_version: string;
}

export type RuleStatus = "pass" | "fail" | "na";

export interface ValidatorRuleResult {
  rule: number;
  status: RuleStatus;
  detail: string;
}

export interface ValidatorReport {
  ok: boolean;
  rules: ValidatorRuleResult[];
  package: ExportPackage;
}

/** Environment the validator resolves references against. Clip/node ids
 *  come from the live project; `stemExists` is absent in MCP (no render
 *  path — GA-4's CLI owns file + byte-render checks), which yields `na`. */
export interface ValidatorEnv {
  clipIds: Set<string>;
  nodeIds: Set<string>;
  stemExists?: (path: string) => boolean;
}

export interface AuditionCueResult {
  cue_id: string;
  audible_layers: string[];
  transition: {
    kind: TransitionKind;
    fade_beats: number;
    from_state: string;
    to_state: string;
  } | null;
}

export interface AuditionResult {
  state: string;
  values: GameStateValue[];
  cues: AuditionCueResult[];
}

export interface TriggerResult {
  event_id: string;
  dropped: boolean;
  reason: string | null;
  clip_id: string | null;
  volume: number;
  pitch_semitones: number;
  voices: number;
  rtpc: { target: string; value: number }[];
}

export const DEMO_CUE_ID = "cue_demo";
export const DEMO_BANK_ID = "bank_demo";
export const DEMO_EVENT_ID = "player.footstep";

/** Deterministic seeded RNG (mulberry32) so same seed = same pick sequence. */
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function canonicalJson(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([k, v]) => `${JSON.stringify(k)}:${canonicalJson(v)}`);
  return `{${entries.join(",")}}`;
}

function clamp01(t: number): number {
  return Math.min(1, Math.max(0, t));
}

export interface GameAudioStoreOptions {
  seed?: number;
  nowMs?: () => number;
}

interface VoiceState {
  lastMs: number | null;
  voices: { clipId: string; startedMs: number }[];
}

/** In-memory cue/bank store + evaluation. One per MCP server instance. */
export class GameAudioStore {
  private cues = new Map<string, AdaptiveCue>();
  private banks = new Map<string, SfxBank>();
  private params: GameStateParamDecl[] = [];
  private lastSnapshot: GameStateSnapshot | null = null;
  private triggerState = new Map<string, VoiceState>();
  private rng: () => number;
  private nowMs: () => number;

  constructor(options: GameAudioStoreOptions = {}) {
    this.rng = mulberry32(options.seed ?? 0x9e3779b9);
    this.nowMs = options.nowMs ?? (() => Date.now());
    this.seedDemo();
  }

  private seedDemo(): void {
    this.params = [
      {
        id: "threat",
        label: "Threat",
        min: 0,
        max: 1,
        default: 0,
        unit: "",
      },
    ];
    this.cues.set(DEMO_CUE_ID, {
      schema_version: GAME_AUDIO_SCHEMA_VERSION,
      id: DEMO_CUE_ID,
      name: "Demo Cue",
      tempo: 120,
      default_state: "explore",
      layers: [
        {
          id: "bed",
          name: "Bed",
          clip_ids: ["clip_bed"],
          states: [],
          volume: 0.8,
        },
        {
          id: "drums",
          name: "Drums",
          clip_ids: ["clip_drums"],
          states: ["combat"],
          volume: 0.9,
        },
      ],
      transitions: [
        {
          id: "t_explore_combat",
          from_state: "explore",
          to_state: "combat",
          kind: "Fade",
          fade_beats: 4,
          stinger_cue_id: "",
        },
      ],
    });
    this.banks.set(DEMO_BANK_ID, {
      schema_version: GAME_AUDIO_SCHEMA_VERSION,
      id: DEMO_BANK_ID,
      name: "Demo Bank",
      events: [
        {
          id: DEMO_EVENT_ID,
          name: "Footstep",
          clip_ids: ["clip_step_a", "clip_step_b"],
          volume: 0.7,
          volume_random: 0.05,
          pitch_random: 1.0,
          cooldown_ms: 120,
          max_polyphony: 4,
          rtpc: [
            {
              param: "threat",
              target_node: "bus_sfx",
              target_param: "volume",
              min: 0.5,
              max: 1.0,
            },
          ],
        },
      ],
    });
  }

  /** Author a cue (test/authoring seam; not an MCP tool — audition-safe). */
  addCue(cue: AdaptiveCue): void {
    if (cue.schema_version !== GAME_AUDIO_SCHEMA_VERSION) {
      throw new Error(`cue schema_version must be ${GAME_AUDIO_SCHEMA_VERSION}`);
    }
    if (!cue.id) throw new Error("cue id must be a non-empty string");
    if (this.cues.has(cue.id)) throw new Error(`duplicate cue id: ${cue.id}`);
    this.cues.set(cue.id, structuredClone(cue));
  }

  /** Author a bank (test/authoring seam; not an MCP tool — audition-safe). */
  addBank(bank: SfxBank): void {
    if (bank.schema_version !== GAME_AUDIO_SCHEMA_VERSION) {
      throw new Error(`bank schema_version must be ${GAME_AUDIO_SCHEMA_VERSION}`);
    }
    if (!bank.id) throw new Error("bank id must be a non-empty string");
    if (this.banks.has(bank.id)) throw new Error(`duplicate bank id: ${bank.id}`);
    this.banks.set(bank.id, structuredClone(bank));
  }

  listCues(): {
    id: string;
    name: string;
    tempo: number;
    default_state: string;
    layers: { id: string; name: string; states: string[] }[];
    transition_count: number;
  }[] {
    return [...this.cues.values()].map((c) => ({
      id: c.id,
      name: c.name,
      tempo: c.tempo,
      default_state: c.default_state,
      layers: c.layers.map((l) => ({ id: l.id, name: l.name, states: [...l.states] })),
      transition_count: c.transitions.length,
    }));
  }

  getCue(id: string): AdaptiveCue {
    const cue = this.cues.get(id);
    if (!cue) throw new Error(`unknown cue: ${id}`);
    return cue;
  }

  getBank(id: string): SfxBank {
    const bank = this.banks.get(id);
    if (!bank) throw new Error(`unknown bank: ${id}`);
    return bank;
  }

  getEvent(eventId: string): SfxEvent {
    for (const bank of this.banks.values()) {
      const event = bank.events.find((e) => e.id === eventId);
      if (event) return event;
    }
    throw new Error(`unknown event: ${eventId}`);
  }

  /** Layers audible in `state` (empty `states` = always-on bed). */
  layersForState(cue: AdaptiveCue, state: string): CueLayer[] {
    return cue.layers.filter((l) => l.states.length === 0 || l.states.includes(state));
  }

  /** First rule matching a state change; absent rule = clean cut (null). */
  transitionFor(cue: AdaptiveCue, from: string, to: string): TransitionRule | null {
    return cue.transitions.find((t) => t.from_state === from && t.to_state === to) ?? null;
  }

  /** Post a snapshot (evaluated + remembered for RTPC/transitions, never
   *  stored as project data) and return the audible layer set per cue. */
  audition(snapshot: GameStateSnapshot): AuditionResult {
    if (!snapshot || typeof snapshot.state !== "string" || !snapshot.state) {
      throw new Error("state must be a non-empty string");
    }
    if (!Array.isArray(snapshot.values)) throw new Error("values must be an array");
    for (const v of snapshot.values) {
      if (typeof v?.param !== "string" || !v.param) {
        throw new Error("each value needs a non-empty param name");
      }
      if (typeof v?.value !== "number" || !Number.isFinite(v.value)) {
        throw new Error(`value for param "${v?.param}" must be a finite number`);
      }
    }
    const prev = this.lastSnapshot;
    const next: GameStateSnapshot = structuredClone(snapshot);
    this.lastSnapshot = next;
    const cues: AuditionCueResult[] = [...this.cues.values()].map((cue) => {
      const from = prev?.state ?? cue.default_state;
      const rule = from === next.state ? null : this.transitionFor(cue, from, next.state);
      return {
        cue_id: cue.id,
        audible_layers: this.layersForState(cue, next.state).map((l) => l.id),
        transition: rule
          ? {
              kind: rule.kind,
              fade_beats: rule.fade_beats,
              from_state: rule.from_state,
              to_state: rule.to_state,
            }
          : null,
      };
    });
    return { state: next.state, values: next.values, cues };
  }

  /** Trigger one SFX event (audition only): uniform pool pick on the seeded
   *  RNG, per-trigger humanization, cooldown drop, oldest-steal polyphony,
   *  per-trigger RTPC evaluation. Unknown *game* params are ignored quietly;
   *  unknown *mix* targets are the validator's job (loud there). */
  trigger(eventId: string, values?: GameStateValue[]): TriggerResult {
    const event = this.getEvent(eventId);
    if (event.clip_ids.length === 0) {
      throw new Error(`event "${eventId}" has an empty pool (validator rule 2)`);
    }
    const now = this.nowMs();
    let st = this.triggerState.get(eventId);
    if (!st) {
      st = { lastMs: null, voices: [] };
      this.triggerState.set(eventId, st);
    }
    if (st.lastMs !== null && now - st.lastMs < event.cooldown_ms) {
      return {
        event_id: eventId,
        dropped: true,
        reason: "cooldown",
        clip_id: null,
        volume: 0,
        pitch_semitones: 0,
        voices: st.voices.length,
        rtpc: [],
      };
    }
    const clipId = event.clip_ids[Math.floor(this.rng() * event.clip_ids.length)]!;
    const volume = Math.max(0, event.volume + (this.rng() * 2 - 1) * event.volume_random);
    const pitchSemitones = (this.rng() * 2 - 1) * event.pitch_random;
    st.lastMs = now;
    st.voices.push({ clipId, startedMs: now });
    while (st.voices.length > Math.max(1, event.max_polyphony)) st.voices.shift();
    const live = values ?? this.lastSnapshot?.values ?? [];
    const rtpc = event.rtpc.flatMap((b) => {
      const hit = live.find((v) => v.param === b.param);
      if (!hit) return [];
      const decl = this.params.find((p) => p.id === b.param);
      const t = decl && decl.max > decl.min ? (hit.value - decl.min) / (decl.max - decl.min) : hit.value;
      return [{ target: `${b.target_node}:${b.target_param}`, value: b.min + clamp01(t) * (b.max - b.min) }];
    });
    return {
      event_id: eventId,
      dropped: false,
      reason: null,
      clip_id: clipId,
      volume,
      pitch_semitones: pitchSemitones,
      voices: st.voices.length,
      rtpc,
    };
  }

  /** Build an export manifest: one MusicLayer stem per cue layer plus one
   *  SfxClip stem per event pool clip. Loop points ship as `0, 0`
   *  (one-shot markers) — GA-4's render path fills real beat loop points;
   *  rule 4 accepts `0, 0` as one-shot, never as a loop. */
  buildPackage(name: string, cueIds: string[], bankIds: string[]): ExportPackage {
    if (!name) throw new Error("name must be a non-empty string");
    const cues = cueIds.map((id) => this.getCue(id));
    const banks = bankIds.map((id) => this.getBank(id));
    const stems: ExportStem[] = [];
    for (const cue of cues) {
      for (const layer of cue.layers) {
        stems.push({
          path: `stems/${cue.id}_${layer.id}.wav`,
          source_id: cue.id,
          source_layer_id: layer.id,
          kind: "MusicLayer",
          loop_start_beats: 0,
          loop_end_beats: 0,
        });
      }
    }
    for (const bank of banks) {
      for (const event of bank.events) {
        event.clip_ids.forEach((_clipId, n) => {
          stems.push({
            path: `sfx/${event.id}_${n}.wav`,
            source_id: event.id,
            source_layer_id: "",
            kind: "SfxClip",
            loop_start_beats: 0,
            loop_end_beats: 0,
          });
        });
      }
    }
    return {
      schema_version: GAME_AUDIO_SCHEMA_VERSION,
      name,
      cue_ids: [...cueIds],
      bank_ids: [...bankIds],
      stems,
      event_bank_path: banks.length === 1 ? `bank_${banks[0]!.id}.json` : "banks.json",
      validator_version: GAME_AUDIO_VALIDATOR_VERSION,
    };
  }

  /** Five-rule validator (`contracts/export-package.md`). Rules 1–3 and the
   *  loop-range half of rule 4 run fully; stem file existence (rule 4) and
   *  stem-byte re-render (rule 5) need GA-4's render path, so without
   *  `env.stemExists` rule 4 file checks report `na`, and rule 5 checks
   *  manifest determinism (rebuild-from-same-inputs byte equality) while
   *  naming GA-4 as the stem-byte owner. `ok` = zero `fail` statuses. */
  validatePackage(pkg: ExportPackage, env: ValidatorEnv): ValidatorReport {
    const rules: ValidatorRuleResult[] = [];

    const missingCues = pkg.cue_ids.filter((id) => !this.cues.has(id));
    const missingBanks = pkg.bank_ids.filter((id) => !this.banks.has(id));
    rules.push({
      rule: 1,
      status: missingCues.length === 0 && missingBanks.length === 0 ? "pass" : "fail",
      detail:
        missingCues.length === 0 && missingBanks.length === 0
          ? `all ${pkg.cue_ids.length} cue(s) + ${pkg.bank_ids.length} bank(s) resolve`
          : `unresolved: ${[...missingCues, ...missingBanks].join(", ")}`,
    });

    const problems: string[] = [];
    for (const id of pkg.cue_ids) {
      const cue = this.cues.get(id);
      if (!cue) continue;
      for (const layer of cue.layers) {
        for (const clip of layer.clip_ids) {
          if (!env.clipIds.has(clip)) problems.push(`cue "${id}" layer "${layer.id}" dangling clip "${clip}"`);
        }
      }
    }
    for (const id of pkg.bank_ids) {
      const bank = this.banks.get(id);
      if (!bank) continue;
      const seen = new Set<string>();
      for (const event of bank.events) {
        if (seen.has(event.id)) problems.push(`bank "${id}" duplicate event id "${event.id}"`);
        seen.add(event.id);
        if (event.clip_ids.length === 0) problems.push(`bank "${id}" event "${event.id}" has an empty pool`);
        for (const clip of event.clip_ids) {
          if (!env.clipIds.has(clip)) problems.push(`bank "${id}" event "${event.id}" dangling clip "${clip}"`);
        }
      }
    }
    rules.push({
      rule: 2,
      status: problems.length === 0 ? "pass" : "fail",
      detail: problems.length === 0 ? "all layer/event clips resolve; pools non-empty; event ids unique" : problems.join("; "),
    });

    const refProblems: string[] = [];
    for (const id of pkg.cue_ids) {
      const cue = this.cues.get(id);
      if (!cue) continue;
      for (const t of cue.transitions) {
        if (t.kind === "Stinger" && !this.cues.has(t.stinger_cue_id)) {
          refProblems.push(`cue "${id}" rule "${t.id}" dangling stinger "${t.stinger_cue_id}"`);
        }
      }
    }
    for (const id of pkg.bank_ids) {
      const bank = this.banks.get(id);
      if (!bank) continue;
      for (const event of bank.events) {
        for (const b of event.rtpc) {
          if (!env.nodeIds.has(b.target_node) || !b.target_param) {
            refProblems.push(
              `bank "${id}" event "${event.id}" unknown mix target "${b.target_node}:${b.target_param}"`,
            );
          }
        }
      }
    }
    rules.push({
      rule: 3,
      status: refProblems.length === 0 ? "pass" : "fail",
      detail:
        refProblems.length === 0
          ? "all stinger + RTPC mix targets resolve"
          : refProblems.join("; "),
    });

    const loopProblems = pkg.stems.flatMap((s) => {
      const { loop_start_beats: a, loop_end_beats: b } = s;
      const oneShot = a === 0 && b === 0;
      return oneShot || (a >= 0 && a < b) ? [] : [`stem "${s.path}" bad loop range ${a}..${b}`];
    });
    const missingFiles = env.stemExists ? pkg.stems.filter((s) => !env.stemExists!(s.path)).map((s) => s.path) : null;
    const fileProblems = (missingFiles ?? []).map((p) => `stem "${p}" missing`);
    const allLoopProblems = [...loopProblems, ...fileProblems];
    rules.push({
      rule: 4,
      status:
        allLoopProblems.length > 0
          ? "fail"
          : missingFiles === null
            ? "na"
            : "pass",
      detail:
        allLoopProblems.length > 0
          ? allLoopProblems.join("; ")
          : missingFiles === null
            ? `loop ranges ok (${pkg.stems.length} stem(s)); file existence unchecked — no render path in MCP (GA-4 CLI owns it)`
            : `all ${pkg.stems.length} stem(s) exist and parse as WAV; loop ranges ok`,
    });

    const rebuilt = this.buildPackage(pkg.name, pkg.cue_ids, pkg.bank_ids);
    const deterministic = canonicalJson({ ...rebuilt, validator_version: pkg.validator_version }) ===
      canonicalJson(pkg);
    rules.push({
      rule: 5,
      status: deterministic ? "pass" : "fail",
      detail: deterministic
        ? "rebuild from same inputs is byte-identical (manifest); stem-byte re-render owned by GA-4"
        : "rebuild differs — package is not deterministic",
    });

    return { ok: rules.every((r) => r.status !== "fail"), rules, package: pkg };
  }
}
