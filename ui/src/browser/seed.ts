import { LibraryItemSchema } from "../generated/project";
import type { LibraryItem } from "../generated/project";

/**
 * Track J: seeded demo library.
 *
 * Item-for-item mirror of `seed_library()` in `core/src/library.rs` — same
 * ids, kinds, names, tags, and text. Every entry is validated against the
 * generated Zod schema at module load so a seed typo fails fast and loud.
 * Keep the two files in sync when editing seeds.
 */

const RAW_SEED: LibraryItem[] = [
  {
    id: "preset_warm_pad",
    kind: "Preset",
    name: "Warm Analog Pad",
    tags: ["warm", "analog", "pad", "ambient", "synth", "lush"],
    text: "Lush warm analog-style pad with slow attack for ambient intros and beds",
  },
  {
    id: "preset_punchy_bass",
    kind: "Preset",
    name: "Punchy Synth Bass",
    tags: ["bass", "synth", "punchy", "mono", "reese"],
    text: "Punchy mono synth bass with fast filter envelope for drive sections",
  },
  {
    id: "sample_boomy_kick",
    kind: "Sample",
    name: "Boomy 808 Kick",
    tags: ["kick", "808", "boom", "drums", "sub"],
    text: "Deep boomy 808 kick drum sample with long sub decay",
  },
  {
    id: "sample_crispy_snare",
    kind: "Sample",
    name: "Crispy Snare",
    tags: ["snare", "drums", "crisp", "backbeat", "clap"],
    text: "Crispy acoustic snare with bright transient for backbeats",
  },
  {
    id: "plugin_vintage_verb",
    kind: "Plugin",
    name: "Vintage Hall Reverb",
    tags: ["reverb", "hall", "space", "vintage", "send"],
    text: "Vintage hall reverb plugin for sends, long lush tails",
  },
  {
    id: "plugin_tape_delay",
    kind: "Plugin",
    name: "Tape Echo Delay",
    tags: ["delay", "echo", "tape", "dotted", "send"],
    text: "Tape-style echo delay plugin with wow flutter and dotted repeats",
  },
  {
    id: "project_midnight_demo",
    kind: "Project",
    name: "Midnight Drive Demo",
    tags: ["demo", "synthwave", "night", "driving", "template"],
    text: "Synthwave demo project with driving bass and neon pads template",
  },
  {
    id: "project_lofi_sketch",
    kind: "Project",
    name: "Lofi Sketch",
    tags: ["lofi", "hiphop", "chill", "keys", "template"],
    text: "Chill lofi hiphop sketch project with dusty keys and soft drums template",
  },
];

for (const raw of RAW_SEED) {
  LibraryItemSchema.parse(raw);
}

/** The seeded library (a fresh validated copy per call). */
export function seedLibrary(): LibraryItem[] {
  return RAW_SEED.map((item) => LibraryItemSchema.parse({ ...item }));
}
