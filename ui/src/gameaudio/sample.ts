import type {
  AdaptiveCue,
  GameStateParam,
  GameStateSnapshot,
  SfxBank,
} from "../generated/project";

/** Demo fixtures for the audition simulator: one cue, one bank, three params. */
export function sampleParams(): GameStateParam[] {
  return [
    { id: "threat", label: "Threat", min: 0, max: 1, default: 0, unit: "" },
    { id: "speed", label: "Speed", min: 0, max: 12, default: 3, unit: "m/s" },
    { id: "health", label: "Health", min: 0, max: 100, default: 100, unit: "hp" },
  ];
}

export function defaultSnapshot(): GameStateSnapshot {
  return { state: "explore", values: [] };
}

export function sampleCue(): AdaptiveCue {
  return {
    schema_version: 1,
    id: "cue_demo",
    name: "Demo",
    tempo: 120,
    default_state: "explore",
    layers: [
      { id: "bed", name: "Bed", clip_ids: ["clip_a"], states: [], volume: 0.8 },
      { id: "drums", name: "Drums", clip_ids: ["clip_b"], states: ["combat"], volume: 0.9 },
      { id: "brass", name: "Brass", clip_ids: ["clip_c"], states: ["combat"], volume: 0.7 },
    ],
    transitions: [
      { id: "t_fade", from_state: "explore", to_state: "combat", kind: "Fade", fade_beats: 4, stinger_cue_id: "" },
      { id: "t_bar", from_state: "combat", to_state: "explore", kind: "BarWait", fade_beats: 0, stinger_cue_id: "" },
    ],
  };
}

export function sampleBank(): SfxBank {
  return {
    schema_version: 1,
    id: "bank_demo",
    name: "Demo",
    events: [
      {
        id: "ui.click",
        name: "Click",
        clip_ids: ["clip_click_a", "clip_click_b"],
        volume: 0.7,
        volume_random: 0.05,
        pitch_random: 1,
        cooldown_ms: 50,
        max_polyphony: 4,
        rtpc: [],
      },
      {
        id: "player.footstep",
        name: "Footstep",
        clip_ids: ["clip_step_a", "clip_step_b", "clip_step_c"],
        volume: 0.8,
        volume_random: 0.1,
        pitch_random: 2,
        cooldown_ms: 90,
        max_polyphony: 2,
        rtpc: [
          { param: "threat", target_node: "bus_sfx", target_param: "volume", min: 0.5, max: 1 },
        ],
      },
    ],
  };
}
