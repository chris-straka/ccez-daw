import {
  AdaptiveCueSchema,
  GameStateParamSchema,
  SfxBankSchema,
  type AdaptiveCue,
  type GameStateParam,
  type SfxBank,
} from "../generated/project";
import { validateCue } from "./cues";

/**
 * S-1 fixture: the demo TP-like package as in-DAW authoring data (pure TS).
 *
 * This mirrors `ga-fixture/src/main.rs` (the Rust builder that renders the
 * shippable bytes through the frozen GA-4 path) so authors can preview,
 * describe, and pre-validate the fixture bank without leaving the DAW. The
 * shipped stems stay rendered by Rust — this module never synthesizes audio,
 * it only builds and checks the same ids, layers, states, params, and RTPC
 * bindings the package carries. Anything here must validate against the
 * generated v1 schemas, so `bun run typegen -- --check` stays green.
 */

export const FIXTURE_PACKAGE_NAME = "demo-tp-v1" as const;
export const FIXTURE_BANK_ID = "bank_tp" as const;
/** Game states the demo covers (LoZ:TP mold: field/combat/boss + village/night). */
export const FIXTURE_STATES: readonly string[] = ["field", "combat", "boss", "village", "night"];
/** Cue ids shipped in the fixture package, in manifest order. */
export const FIXTURE_CUE_IDS: readonly string[] = ["cue_field", "cue_boss", "cue_stinger_boss"];

/** The four game params the template wires to sliders. */
export function fixtureParams(): GameStateParam[] {
  return [
    { id: "threat", label: "Threat", min: 0, max: 1, default: 0, unit: "" },
    { id: "time_of_day", label: "Time of day", min: 0, max: 24, default: 12, unit: "h" },
    { id: "health_low", label: "Health low", min: 0, max: 1, default: 0, unit: "" },
    { id: "mounted", label: "Mounted", min: 0, max: 1, default: 0, unit: "" },
  ];
}

function layer(
  id: string,
  name: string,
  clip_ids: string[],
  states: string[],
  volume: number,
): AdaptiveCue["layers"][number] {
  return { id, name, clip_ids, states, volume };
}

/** Field/combat cue (104 BPM): bed + field/village strings + combat drums + night pad. */
export function cueField(): AdaptiveCue {
  return {
    schema_version: 1,
    id: "cue_field",
    name: "Field",
    tempo: 104,
    default_state: "field",
    layers: [
      layer("bed", "Field Bed", ["clip_field_bed"], [], 0.8),
      layer("strings", "Field Strings", ["clip_field_strings"], ["field", "village"], 0.75),
      layer("drums", "Combat Drums", ["clip_field_drums"], ["combat"], 0.9),
      layer("nightpad", "Night Pad", ["clip_field_night"], ["night"], 0.7),
    ],
    transitions: [
      { id: "t_field_combat", from_state: "field", to_state: "combat", kind: "Fade", fade_beats: 2, stinger_cue_id: "" },
      { id: "t_combat_field", from_state: "combat", to_state: "field", kind: "Fade", fade_beats: 4, stinger_cue_id: "" },
      { id: "t_night_field", from_state: "night", to_state: "field", kind: "Fade", fade_beats: 2, stinger_cue_id: "" },
      { id: "t_field_boss", from_state: "field", to_state: "boss", kind: "Stinger", fade_beats: 0, stinger_cue_id: "cue_stinger_boss" },
    ],
  };
}

/** Boss cue (132 BPM): bed plus brass + choir stacks that only sound on `boss`. */
export function cueBoss(): AdaptiveCue {
  return {
    schema_version: 1,
    id: "cue_boss",
    name: "Boss",
    tempo: 132,
    default_state: "boss",
    layers: [
      layer("bed", "Boss Bed", ["clip_boss_bed"], [], 0.8),
      layer("brass", "Boss Brass", ["clip_boss_brass"], ["boss"], 0.85),
      layer("choir", "Boss Choir", ["clip_boss_choir"], ["boss"], 0.7),
    ],
    transitions: [
      { id: "t_boss_field", from_state: "boss", to_state: "field", kind: "Fade", fade_beats: 4, stinger_cue_id: "" },
    ],
  };
}

/** The one-shot hit the field->boss `Stinger` rule fires (a real cue, so rule 3 resolves). */
export function cueStingerBoss(): AdaptiveCue {
  return {
    schema_version: 1,
    id: "cue_stinger_boss",
    name: "Boss Stinger",
    tempo: 132,
    default_state: "boss",
    layers: [layer("hit", "Stinger Hit", ["clip_stinger_hit"], ["boss"], 0.9)],
    transitions: [],
  };
}

export function fixtureCues(): AdaptiveCue[] {
  return [cueField(), cueBoss(), cueStingerBoss()];
}

/** Demo SFX bank: footsteps + sword bent by `threat`, UI confirm, gallop bent by `mounted`. */
export function fixtureBank(): SfxBank {
  return {
    schema_version: 1,
    id: FIXTURE_BANK_ID,
    name: "TP Demo Bank",
    events: [
      {
        id: "player.footstep",
        name: "Footstep",
        clip_ids: ["clip_step_a", "clip_step_b"],
        volume: 0.7,
        volume_random: 0.05,
        pitch_random: 1,
        cooldown_ms: 90,
        max_polyphony: 4,
        rtpc: [{ param: "threat", target_node: "bus_sfx", target_param: "volume", min: 0.5, max: 1 }],
      },
      {
        id: "sword.swing",
        name: "Sword Swing",
        clip_ids: ["clip_swing_a", "clip_swing_b"],
        volume: 0.85,
        volume_random: 0.08,
        pitch_random: 2,
        cooldown_ms: 120,
        max_polyphony: 3,
        rtpc: [{ param: "threat", target_node: "bus_sfx", target_param: "volume", min: 0.6, max: 1 }],
      },
      {
        id: "ui.confirm",
        name: "UI Confirm",
        clip_ids: ["clip_ui_a"],
        volume: 0.6,
        volume_random: 0.03,
        pitch_random: 0.5,
        cooldown_ms: 50,
        max_polyphony: 4,
        rtpc: [],
      },
      {
        id: "horse.gallop",
        name: "Horse Gallop",
        clip_ids: ["clip_gal_a", "clip_gal_b"],
        volume: 0.75,
        volume_random: 0.06,
        pitch_random: 1.5,
        cooldown_ms: 140,
        max_polyphony: 2,
        rtpc: [{ param: "mounted", target_node: "trk_sfx", target_param: "volume", min: 0.4, max: 1 }],
      },
    ],
  };
}

/** Every v0 clip id the fixture cues and bank pool reference. */
export function fixtureClipIds(): string[] {
  const ids = new Set<string>();
  for (const cue of fixtureCues()) {
    for (const l of cue.layers) for (const c of l.clip_ids) ids.add(c);
  }
  for (const e of fixtureBank().events) for (const c of e.clip_ids) ids.add(c);
  return [...ids].sort();
}

export interface FixtureValidationOptions {
  /** Known v0 clip ids; dangling `clip_ids` are errors when given. */
  clipIds?: string[];
  /** Known `target_node:target_param` mix addresses (`node:param` strings). */
  mixTargets?: string[];
}

/**
 * Human-readable problems with a fixture bank + cues; empty = shippable.
 * Mirrors validator rules 2–3 in the DAW so authors fix banks before export:
 * schema parse, dangling clips, empty pools, duplicate events, unknown
 * stinger refs, and typo'd `node:param` mix targets. Never throws.
 */
export function validateFixture(
  cues: AdaptiveCue[] = fixtureCues(),
  bank: SfxBank = fixtureBank(),
  params: GameStateParam[] = fixtureParams(),
  opts: FixtureValidationOptions = {},
): string[] {
  const problems: string[] = [];
  const clipIds = new Set(opts.clipIds ?? fixtureClipIds());
  const cueIds = new Set(cues.map((c) => c.id));
  const paramIds = new Set(params.map((p) => p.id));

  for (const cue of cues) {
    const parsed = AdaptiveCueSchema.safeParse(cue);
    if (!parsed.success) {
      problems.push(`cue \`${cue.id}\` fails schema: ${parsed.error.issues[0]?.message ?? "invalid"}`);
      continue;
    }
    for (const p of validateCue(cue, { clipIds: [...clipIds], cueIds: [...cueIds] })) {
      problems.push(`cue \`${cue.id}\`: ${p}`);
    }
  }

  const bankParsed = SfxBankSchema.safeParse(bank);
  if (!bankParsed.success) {
    problems.push(`bank \`${bank.id}\` fails schema: ${bankParsed.error.issues[0]?.message ?? "invalid"}`);
    return problems;
  }
  const seenEvents = new Set<string>();
  for (const e of bank.events) {
    if (seenEvents.has(e.id)) problems.push(`bank \`${bank.id}\`: duplicate event id \`${e.id}\``);
    else seenEvents.add(e.id);
    if (e.clip_ids.length === 0) problems.push(`bank \`${bank.id}\` event \`${e.id}\` has an empty clip pool`);
    for (const c of e.clip_ids) {
      if (!clipIds.has(c)) problems.push(`bank \`${bank.id}\` event \`${e.id}\` references unknown clip \`${c}\``);
    }
    for (const b of e.rtpc) {
      if (!paramIds.has(b.param)) {
        problems.push(`bank \`${bank.id}\` event \`${e.id}\` binds unknown game param \`${b.param}\``);
      }
      if (opts.mixTargets && !opts.mixTargets.includes(`${b.target_node}:${b.target_param}`)) {
        problems.push(
          `bank \`${bank.id}\` event \`${e.id}\` binds unknown mix target \`${b.target_node}:${b.target_param}\``,
        );
      }
    }
  }

  for (const p of params) {
    const parsed = GameStateParamSchema.safeParse(p);
    if (!parsed.success) {
      problems.push(`param \`${p.id}\` fails schema: ${parsed.error.issues[0]?.message ?? "invalid"}`);
    }
  }
  return problems;
}

/** One-line-per-fact summary of the fixture for the authoring panel. */
export function describeFixture(
  cues: AdaptiveCue[] = fixtureCues(),
  bank: SfxBank = fixtureBank(),
  params: GameStateParam[] = fixtureParams(),
): string[] {
  const lines: string[] = [];
  lines.push(`${FIXTURE_PACKAGE_NAME}: ${cues.length} cues, ${bank.events.length} events, ${params.length} params`);
  for (const cue of cues) {
    const layers = cue.layers.map((l) => `${l.id} (${l.states.length === 0 ? "bed" : l.states.join("+")})`).join(", ");
    lines.push(`cue \`${cue.id}\` @ ${cue.tempo} BPM: ${layers}`);
  }
  for (const e of bank.events) {
    const rtpc = e.rtpc.map((b) => `${b.param}->${b.target_node}:${b.target_param}`).join(", ") || "no RTPC";
    lines.push(`event \`${e.id}\` x${e.clip_ids.length} clips, poly ${e.max_polyphony}: ${rtpc}`);
  }
  return lines;
}
