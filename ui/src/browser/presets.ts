import { LibraryItemSchema, type LibraryItem, type Node, type Op } from "../generated/project";
import { paramSetOp } from "../scripting/ops";

/**
 * Track D: curated native preset library (TypeScript mirror).
 *
 * Item-for-item mirror of `NATIVE_PRESETS` in
 * `core/src/devices/presets.rs` — same ids, classes, names, descriptions,
 * tags, and param values. The Rust tests pin exact application of every
 * preset to its device class; `ui/tests/presets.test.ts` pins the same
 * here. Keep the two tables in sync when editing presets.
 *
 * Application goes through the existing op path: {@link presetParamOps}
 * expands one preset to one `ParamSet` op per param (undoable via the
 * existing `op_undo`), and {@link applyPresetToNode} stamps a local node
 * copy for instant preview. {@link seedNativePresetLibrary} exposes the
 * same entries to the browser search palette.
 */

export type NativeDeviceClass =
  | "gain"
  | "lowpass"
  | "highpass"
  | "delay"
  | "distortion"
  | "sampler"
  | "arpeggiator"
  | "chord"
  | "humanize";

export interface NativePreset {
  id: string;
  deviceClass: NativeDeviceClass;
  name: string;
  description: string;
  tags: string[];
  params: Record<string, number>;
}

/** Numeric class codes. Mirrors `CLASS_*` in `core/src/devices/class.rs`. */
const CLASS_CODES: Record<NativeDeviceClass, number> = {
  gain: 1,
  lowpass: 2,
  highpass: 3,
  delay: 4,
  distortion: 5,
  sampler: 7,
  arpeggiator: 8,
  chord: 9,
  humanize: 10,
};

export interface ParamRange {
  id: string;
  min: number;
  max: number;
}

/**
 * Param ids and ranges per class. Mirrors `default_params` in
 * `core/src/devices/class.rs` (ids and `[min, max]` only — defaults live
 * in Rust; the validity test stamps exact values, so drift fails loud).
 */
export const DEVICE_PARAM_RANGES: Record<NativeDeviceClass, ParamRange[]> = {
  gain: [{ id: "gain", min: 0, max: 4 }],
  lowpass: [{ id: "cutoff", min: 20, max: 20000 }],
  highpass: [{ id: "cutoff", min: 20, max: 20000 }],
  delay: [
    { id: "delay_samples", min: 0, max: 48000 },
    { id: "feedback", min: 0, max: 0.95 },
  ],
  distortion: [{ id: "drive", min: 0, max: 10 }],
  sampler: [
    { id: "transpose", min: -48, max: 48 },
    { id: "gain", min: 0, max: 4 },
    { id: "attack", min: 0, max: 10 },
    { id: "release", min: 0, max: 10 },
    { id: "cutoff", min: 20, max: 20000 },
  ],
  arpeggiator: [
    { id: "arp_mode", min: 0, max: 3 },
    { id: "arp_rate", min: 0.0625, max: 4 },
    { id: "arp_gate", min: 0.05, max: 1 },
    { id: "arp_octaves", min: 1, max: 4 },
    { id: "arp_seed", min: 0, max: 4294967295 },
  ],
  chord: [
    { id: "chord_type", min: 0, max: 3 },
    { id: "chord_inversion", min: 0, max: 3 },
    { id: "chord_voicing", min: 0, max: 2 },
  ],
  humanize: [
    { id: "hum_timing", min: 0, max: 0.25 },
    { id: "hum_velocity", min: 0, max: 64 },
    { id: "hum_seed", min: 0, max: 4294967295 },
  ],
};

export const NATIVE_PRESETS: NativePreset[] = [
  // -- gain: level/comp starting points --
  {
    id: "gain_unity",
    deviceClass: "gain",
    name: "Unity Gain",
    description: "Clean 0 dB starting point: gain at exactly 1.0, no color",
    tags: ["gain", "utility", "unity", "level", "clean"],
    params: { gain: 1.0 },
  },
  {
    id: "gain_vocal_ride",
    deviceClass: "gain",
    name: "Vocal Ride",
    description: "Vocal-up balance lift: a gentle level push that sits a lead over the mix",
    tags: ["gain", "vocal", "level", "mix", "lead"],
    params: { gain: 1.4 },
  },
  {
    id: "gain_parallel_squash",
    deviceClass: "gain",
    name: "Parallel Squash Blend",
    description: "Parallel-compression starting point: hot makeup level meant to blend beside dry",
    tags: ["gain", "parallel", "compression", "comp", "blend", "drums"],
    params: { gain: 2.5 },
  },
  {
    id: "gain_ghost_fade",
    deviceClass: "gain",
    name: "Ghost Fade",
    description: "Barely-there texture bed: level tucked down to a whisper under the lead",
    tags: ["gain", "quiet", "bed", "texture", "ambient", "level"],
    params: { gain: 0.15 },
  },
  // -- lowpass: filter sweeps --
  {
    id: "lp_warm_pad",
    deviceClass: "lowpass",
    name: "Warm Pad Softener",
    description: "Warm analog-style pad filter: lows kept, harsh top rounded off",
    tags: ["filter", "lowpass", "warm", "pad", "ambient", "sweep"],
    params: { cutoff: 800 },
  },
  {
    id: "lp_sweep_mid",
    deviceClass: "lowpass",
    name: "Sweep Midpoint",
    description: "Filter-sweep midpoint: open enough for presence, ready to automate down or up",
    tags: ["filter", "lowpass", "sweep", "automation", "build", "riser"],
    params: { cutoff: 2500 },
  },
  {
    id: "lp_sub_saver",
    deviceClass: "lowpass",
    name: "Sub Saver",
    description: "Sub-only keeper: everything above the bass floor filtered away for 808s and subs",
    tags: ["filter", "lowpass", "sub", "bass", "808", "dark"],
    params: { cutoff: 120 },
  },
  // -- highpass: cleanup / air --
  {
    id: "hp_vocal_cleanup",
    deviceClass: "highpass",
    name: "Vocal Cleanup",
    description: "Vocal low-cut starting point: rumble and handling noise gone, body kept",
    tags: ["filter", "highpass", "vocal", "cleanup", "rumble", "mix"],
    params: { cutoff: 100 },
  },
  {
    id: "hp_tighten_low",
    deviceClass: "highpass",
    name: "Low-End Tightener",
    description: "Mix-bus tightening cut: sub mud trimmed so the kick and bass stop fighting",
    tags: ["filter", "highpass", "tight", "mix", "kick", "bass"],
    params: { cutoff: 40 },
  },
  {
    id: "hp_radio_thin",
    deviceClass: "highpass",
    name: "Radio Thin",
    description: "Telephone-verse effect: thin lo-fi band feel for intros and breakdowns",
    tags: ["filter", "highpass", "lofi", "radio", "effect", "verse"],
    params: { cutoff: 500 },
  },
  // -- delay: groove echoes --
  {
    id: "dly_slapback",
    deviceClass: "delay",
    name: "Rockabilly Slapback",
    description: "Classic slapback: one short repeat that thickens vocals and guitars",
    tags: ["delay", "slapback", "vocal", "guitar", "rock", "echo"],
    params: { delay_samples: 3500, feedback: 0.15 },
  },
  {
    id: "dly_dotted_groove",
    deviceClass: "delay",
    name: "Dotted Groove Echo",
    description: "Dotted-eighth style groove echo: repeats that bounce between the beats",
    tags: ["delay", "groove", "dotted", "echo", "synth", "dance"],
    params: { delay_samples: 12000, feedback: 0.45 },
  },
  {
    id: "dly_tight_doubler",
    deviceClass: "delay",
    name: "Tight Doubler",
    description: "Vocal doubler starting point: a barely-delayed shadow that widens without echo",
    tags: ["delay", "doubler", "vocal", "wide", "chorus", "groove"],
    params: { delay_samples: 1500, feedback: 0.3 },
  },
  // -- distortion: grit ladder --
  {
    id: "dist_edge",
    deviceClass: "distortion",
    name: "Just an Edge",
    description: "Barely-breaking-up warmth: a hint of hair for bass and keys",
    tags: ["distortion", "drive", "warm", "subtle", "bass", "saturation"],
    params: { drive: 0.6 },
  },
  {
    id: "dist_warm_drive",
    deviceClass: "distortion",
    name: "Warm Drive",
    description: "Amp-style warm overdrive: guitars and synths pushed into singing sustain",
    tags: ["distortion", "drive", "overdrive", "guitar", "synth", "warm"],
    params: { drive: 2.0 },
  },
  {
    id: "dist_fuzz_wall",
    deviceClass: "distortion",
    name: "Fuzz Wall",
    description: "All-out fuzz wall: drums and bass crushed into a solid riff block",
    tags: ["distortion", "drive", "fuzz", "heavy", "drums", "wall"],
    params: { drive: 8.0 },
  },
  // -- sampler: starter kits --
  {
    id: "smp_kick_starter",
    deviceClass: "sampler",
    name: "Kick Starter Kit",
    description: "Punchy kick-drum starting point: instant attack, short release, full-range tone",
    tags: ["sampler", "kick", "drums", "starter", "kit", "punch"],
    params: { transpose: 0, gain: 1.0, attack: 0.001, release: 0.15, cutoff: 8000 },
  },
  {
    id: "smp_soft_keys",
    deviceClass: "sampler",
    name: "Soft Keys Pad",
    description: "Mellow sampled-keys bed: slow attack, long release, darkened tone filter",
    tags: ["sampler", "keys", "pad", "soft", "mellow", "starter"],
    params: { transpose: 0, gain: 0.8, attack: 0.08, release: 0.8, cutoff: 3500 },
  },
  {
    id: "smp_chip_blip",
    deviceClass: "sampler",
    name: "Chip Blip",
    description: "8-bit style blip an octave up: snappy, bright, and slightly tucked back",
    tags: ["sampler", "chip", "8bit", "blip", "lead", "octave"],
    params: { transpose: 12, gain: 0.9, attack: 0.002, release: 0.1, cutoff: 20000 },
  },
  // -- arpeggiator: arp patterns --
  {
    id: "arp_sparkle_16ths",
    deviceClass: "arpeggiator",
    name: "Sparkle 16ths Up",
    description: "Glassy rising 16th-note arp over two octaves for pop verses and lifts",
    tags: ["arp", "arpeggiator", "16ths", "up", "sparkle", "pop"],
    params: { arp_mode: 0, arp_rate: 0.25, arp_gate: 0.8, arp_octaves: 2, arp_seed: 0 },
  },
  {
    id: "arp_neon_down",
    deviceClass: "arpeggiator",
    name: "Neon Down 8ths",
    description: "Falling neon 8th-note arp: synthwave basslines and midnight drive patterns",
    tags: ["arp", "arpeggiator", "down", "8ths", "synthwave", "bass"],
    params: { arp_mode: 1, arp_rate: 0.5, arp_gate: 0.9, arp_octaves: 1, arp_seed: 0 },
  },
  {
    id: "arp_casino_random",
    deviceClass: "arpeggiator",
    name: "Casino Random",
    description: "Generative casino arp: seeded-random 16ths across three octaves, same every render",
    tags: ["arp", "arpeggiator", "random", "generative", "ambient", "pattern"],
    params: { arp_mode: 3, arp_rate: 0.25, arp_gate: 0.6, arp_octaves: 3, arp_seed: 7 },
  },
  // -- chord: chord sets --
  {
    id: "chd_sad_triad",
    deviceClass: "chord",
    name: "Sad Triad",
    description: "Lo-fi minor triad in close voicing: every held note turns moody",
    tags: ["chord", "minor", "triad", "lofi", "sad", "close"],
    params: { chord_type: 1, chord_inversion: 0, chord_voicing: 0 },
  },
  {
    id: "chd_open_hymn",
    deviceClass: "chord",
    name: "Open Hymn",
    description: "Wide-open major spread for ambient beds and worship pads",
    tags: ["chord", "major", "open", "ambient", "hymn", "pad"],
    params: { chord_type: 0, chord_inversion: 0, chord_voicing: 2 },
  },
  {
    id: "chd_neo_sus",
    deviceClass: "chord",
    name: "Neo-Soul Sus",
    description: "Neo-soul suspended color: first-inversion sus4 in drop-2 voicing",
    tags: ["chord", "sus4", "neosoul", "rnb", "drop2", "keys"],
    params: { chord_type: 3, chord_inversion: 1, chord_voicing: 1 },
  },
  // -- humanize: groove starting points --
  {
    id: "hum_groove_light",
    deviceClass: "humanize",
    name: "Light Groove",
    description: "Barely-felt pocket: quantized parts loosened just enough to breathe",
    tags: ["humanize", "groove", "light", "pocket", "feel", "timing"],
    params: { hum_timing: 0.008, hum_velocity: 5, hum_seed: 1 },
  },
  {
    id: "hum_drunk_drums",
    deviceClass: "humanize",
    name: "Drunk Drums",
    description: "Dilla-style slouch for drum breaks: timing dragged, velocities swung hard",
    tags: ["humanize", "groove", "drums", "swing", "dilla", "lofi"],
    params: { hum_timing: 0.03, hum_velocity: 14, hum_seed: 42 },
  },
  {
    id: "hum_human_keys",
    deviceClass: "humanize",
    name: "Human Keys",
    description: "Played-not-programmed piano feel: gentle timing drift and dynamic touch",
    tags: ["humanize", "keys", "piano", "feel", "timing", "velocity"],
    params: { hum_timing: 0.015, hum_velocity: 10, hum_seed: 3 },
  },
  // -- gain: more comp/mix starting points --
  {
    id: "gain_bass_glue",
    deviceClass: "gain",
    name: "Bass Glue",
    description: "Mix-glue level for bass: tucked just under unity so the low end sits without pumping",
    tags: ["gain", "bass", "glue", "mix", "level"],
    params: { gain: 0.8 },
  },
  {
    id: "gain_drum_crush",
    deviceClass: "gain",
    name: "Drum Crush Bus",
    description: "Crush-bus makeup level: hot smashed-room tone meant to tuck under the dry drums",
    tags: ["gain", "drums", "crush", "bus", "parallel", "comp"],
    params: { gain: 3.0 },
  },
  {
    id: "gain_ambient_wash",
    deviceClass: "gain",
    name: "Ambient Wash Bed",
    description: "Reverb-wash bed level: quiet bloom that fills gaps without masking the lead",
    tags: ["gain", "ambient", "wash", "bed", "mix", "level"],
    params: { gain: 0.4 },
  },
  // -- lowpass: more filter sweeps --
  {
    id: "lp_dark_drone",
    deviceClass: "lowpass",
    name: "Dark Drone",
    description: "Sub-drone filter: only the lowest rumble survives for dark ambient beds",
    tags: ["filter", "lowpass", "drone", "dark", "ambient", "sub"],
    params: { cutoff: 200 },
  },
  {
    id: "lp_airy_open",
    deviceClass: "lowpass",
    name: "Airy Open Top",
    description: "Wide-open air filter: full brightness kept, just the harshest edge shaved off",
    tags: ["filter", "lowpass", "airy", "open", "bright", "mix"],
    params: { cutoff: 12000 },
  },
  {
    id: "lp_riser_open",
    deviceClass: "lowpass",
    name: "Riser Opener",
    description: "Build-riser starting point: mid-open filter ready to automate wide for the drop",
    tags: ["filter", "lowpass", "riser", "build", "automation", "drop"],
    params: { cutoff: 5000 },
  },
  // -- highpass: more cleanup / air --
  {
    id: "hp_sub_cleanup",
    deviceClass: "highpass",
    name: "Sub Cleanup",
    description: "Gentle sub-sonic trim: inaudible mud gone, kick weight fully kept",
    tags: ["filter", "highpass", "sub", "cleanup", "kick", "mix"],
    params: { cutoff: 60 },
  },
  {
    id: "hp_hihat_sheen",
    deviceClass: "highpass",
    name: "Hi-Hat Sheen",
    description: "Hat-and-air keeper: only sizzle survives for crisp top loops and shakers",
    tags: ["filter", "highpass", "hihat", "sheen", "air", "crisp"],
    params: { cutoff: 8000 },
  },
  {
    id: "hp_dj_rolloff",
    deviceClass: "highpass",
    name: "DJ Rolloff",
    description: "DJ-style low rolloff: bass thinned for transitions, mids and highs intact",
    tags: ["filter", "highpass", "dj", "transition", "rolloff", "mix"],
    params: { cutoff: 150 },
  },
  // -- delay: more groove echoes --
  {
    id: "dly_ambient_wash",
    deviceClass: "delay",
    name: "Ambient Wash Echo",
    description: "Washed-out ambient echo: long repeats that melt guitars into pads",
    tags: ["delay", "ambient", "wash", "echo", "guitar", "pad"],
    params: { delay_samples: 24000, feedback: 0.6 },
  },
  {
    id: "dly_rhythmic_8th",
    deviceClass: "delay",
    name: "Rhythmic 8th Tap",
    description: "Straight-8th rhythmic tap: repeats that lock to the groove for keys and plucks",
    tags: ["delay", "rhythmic", "groove", "keys", "pluck", "echo"],
    params: { delay_samples: 6000, feedback: 0.35 },
  },
  {
    id: "dly_dub_feedback",
    deviceClass: "delay",
    name: "Dub Feedback Throw",
    description: "Dub-throw echo: hot feedback for one-shot sends that spiral into space",
    tags: ["delay", "dub", "feedback", "throw", "reggae", "echo"],
    params: { delay_samples: 18000, feedback: 0.7 },
  },
  // -- distortion: more grit --
  {
    id: "dist_tape_sat",
    deviceClass: "distortion",
    name: "Tape Saturation",
    description: "Soft tape-style saturation: gentle glue and harmonics for the whole mix bus",
    tags: ["distortion", "tape", "saturation", "glue", "mix", "warm"],
    params: { drive: 1.2 },
  },
  {
    id: "dist_punk_crunch",
    deviceClass: "distortion",
    name: "Punk Crunch",
    description: "Punk-rock crunch: rhythm guitars chewed into aggressive midrange bark",
    tags: ["distortion", "punk", "crunch", "guitar", "rock", "drive"],
    params: { drive: 4.5 },
  },
  {
    id: "dist_doom_sustain",
    deviceClass: "distortion",
    name: "Doom Sustain",
    description: "Doom-metal sustain: heavy sagging drive for slow riffs that ring forever",
    tags: ["distortion", "doom", "metal", "sustain", "heavy", "riff"],
    params: { drive: 6.0 },
  },
  // -- sampler: more starter kits (envelope/tone half; pair with a builtin or take sample) --
  {
    id: "smp_boom_808",
    deviceClass: "sampler",
    name: "Boom 808 Kit",
    description: "Boom-808 kit starting point: dropped an octave, long tail, dark tone — pair with a builtin or take 808 sample",
    tags: ["sampler", "808", "bass", "kit", "boom", "starter"],
    params: { transpose: -12, gain: 1.2, attack: 0.004, release: 0.4, cutoff: 900 },
  },
  {
    id: "smp_snare_crack",
    deviceClass: "sampler",
    name: "Snare Crack Kit",
    description: "Cracking snare starting point: instant attack, tight tail, open tone — pair with a builtin or take snare sample",
    tags: ["sampler", "snare", "drums", "kit", "crack", "starter"],
    params: { transpose: 0, gain: 1.1, attack: 0.001, release: 0.12, cutoff: 12000 },
  },
  {
    id: "smp_hihat_tight",
    deviceClass: "sampler",
    name: "Tight Hat Kit",
    description: "Tight closed-hat starting point: pitched up, short tick, bright tone — pair with a builtin or take hat sample",
    tags: ["sampler", "hihat", "hat", "drums", "kit", "tight"],
    params: { transpose: 6, gain: 0.7, attack: 0.001, release: 0.05, cutoff: 15000 },
  },
  {
    id: "smp_vocal_chop",
    deviceClass: "sampler",
    name: "Vocal Chop Kit",
    description: "Chopped-vocal starting point: soft attack, medium tail, smoothed tone — pair with a builtin or take vocal sample",
    tags: ["sampler", "vocal", "chop", "kit", "starter", "sliced"],
    params: { transpose: 0, gain: 0.9, attack: 0.01, release: 0.5, cutoff: 6000 },
  },
  // -- arpeggiator: more rhythmic patterns --
  {
    id: "arp_updown_runner",
    deviceClass: "arpeggiator",
    name: "Up-Down Runner 16ths",
    description: "Endless up-down 16th runner over two octaves for trance gates and synth runs",
    tags: ["arp", "arpeggiator", "updown", "16ths", "trance", "pattern"],
    params: { arp_mode: 2, arp_rate: 0.25, arp_gate: 0.7, arp_octaves: 2, arp_seed: 0 },
  },
  {
    id: "arp_slow_bloom",
    deviceClass: "arpeggiator",
    name: "Slow Bloom Whole",
    description: "Slow-blooming whole-note arp: rising pads that open one note per bar",
    tags: ["arp", "arpeggiator", "slow", "bloom", "pad", "ambient"],
    params: { arp_mode: 0, arp_rate: 1.0, arp_gate: 0.9, arp_octaves: 1, arp_seed: 0 },
  },
  {
    id: "arp_gallop_dotted",
    deviceClass: "arpeggiator",
    name: "Gallop Dotted 8ths",
    description: "Galloping dotted-8th up-down arp: locked groove for synthwave leads",
    tags: ["arp", "arpeggiator", "gallop", "dotted", "synthwave", "lead"],
    params: { arp_mode: 2, arp_rate: 0.75, arp_gate: 0.5, arp_octaves: 1, arp_seed: 0 },
  },
  {
    id: "arp_stardust",
    deviceClass: "arpeggiator",
    name: "Stardust Scatter 8ths",
    description: "Scattered generative 8th-note arp over two octaves: twinkle that never repeats its bar",
    tags: ["arp", "arpeggiator", "random", "generative", "twinkle", "pattern"],
    params: { arp_mode: 3, arp_rate: 0.5, arp_gate: 0.5, arp_octaves: 2, arp_seed: 11 },
  },
  // -- chord: progression-flavored chord sets (voicing color per degree; transpose clips for key) --
  {
    id: "chd_ii_v_i_major",
    deviceClass: "chord",
    name: "ii-V-I Bright Major",
    description: "Bright ii-V-I major color in C: first-inversion close major for the I (transpose the clips for other keys)",
    tags: ["chord", "major", "two-five-one", "progression", "jazz", "pop"],
    params: { chord_type: 0, chord_inversion: 1, chord_voicing: 0 },
  },
  {
    id: "chd_ii_v_i_minor",
    deviceClass: "chord",
    name: "ii-V-i Smoky Minor",
    description: "Smoky ii-V-i minor color in A minor: open minor with the fifth up top for the i (transpose the clips for other keys)",
    tags: ["chord", "minor", "two-five-one", "progression", "jazz", "lofi"],
    params: { chord_type: 1, chord_inversion: 1, chord_voicing: 2 },
  },
  {
    id: "chd_dim_passing",
    deviceClass: "chord",
    name: "Dim Passing Chord",
    description: "Diminished passing color in G: close dim7 that walks between verse chords in any key",
    tags: ["chord", "dim", "passing", "progression", "jazz", "walk"],
    params: { chord_type: 2, chord_inversion: 0, chord_voicing: 1 },
  },
  {
    id: "chd_gospel_shout",
    deviceClass: "chord",
    name: "Gospel Shout Drop-2",
    description: "Gospel shout color in F: second-inversion major in drop-2 for uplifting turnarounds in any key",
    tags: ["chord", "major", "gospel", "drop2", "progression", "uplift"],
    params: { chord_type: 0, chord_inversion: 2, chord_voicing: 1 },
  },
  // -- humanize: more groove starting points --
  {
    id: "hum_tight_pocket",
    deviceClass: "humanize",
    name: "Tight Pocket",
    description: "Near-grid pocket: just enough timing and velocity nudge to dodge the robot feel",
    tags: ["humanize", "groove", "tight", "pocket", "timing", "pop"],
    params: { hum_timing: 0.004, hum_velocity: 3, hum_seed: 5 },
  },
  {
    id: "hum_lofi_swing",
    deviceClass: "humanize",
    name: "Lofi Swing Dust",
    description: "Dusty lo-fi swing: swung timing and soft-hit velocities for sleepy boom-bap loops",
    tags: ["humanize", "groove", "lofi", "swing", "boombap", "dust"],
    params: { hum_timing: 0.02, hum_velocity: 12, hum_seed: 9 },
  },
  {
    id: "hum_live_drums",
    deviceClass: "humanize",
    name: "Live Kit Energy",
    description: "Live-kit energy: hard timing slop and dynamic hits for rock drum takes",
    tags: ["humanize", "groove", "drums", "live", "rock", "energy"],
    params: { hum_timing: 0.045, hum_velocity: 18, hum_seed: 17 },
  },
];

/** Look up one preset by id. Unknown ids are `undefined`, never an error. */
export function findNativePreset(id: string): NativePreset | undefined {
  return NATIVE_PRESETS.find((p) => p.id === id);
}

/** Read a stub node's device class from its `device_class` tag param. */
function nodeClass(node: Pick<Node, "params">): NativeDeviceClass {
  const tag = node.params.find((p) => p.id === "device_class");
  for (const [name, code] of Object.entries(CLASS_CODES)) {
    if (tag?.value === code) return name as NativeDeviceClass;
  }
  throw new Error(`node is not a native device (device_class ${tag?.value})`);
}

/**
 * Stamp a preset onto a local device node (instant preview path): every
 * param set in place, clamped to its range. Throws on unknown presets,
 * class mismatches, and unknown param ids — typos must surface, not
 * vanish. Durable application goes through {@link presetParamOps}.
 */
export function applyPresetToNode(node: Node, preset: NativePreset): void {
  const className = nodeClass(node);
  if (className !== preset.deviceClass) {
    throw new Error(
      `preset ${preset.id} needs ${preset.deviceClass}, node is ${className}`,
    );
  }
  for (const [param, value] of Object.entries(preset.params)) {
    const p = node.params.find((q) => q.id === param);
    if (!p) throw new Error(`unknown device param ${node.id}:${param}`);
    p.value = Math.min(p.max, Math.max(p.min, value));
  }
}

/**
 * Expand one preset to undoable ops: one `ParamSet` op per param against
 * the `deviceId:param` address. Apply each through `op_apply`; each
 * undoes independently through the existing `op_undo`.
 */
export function presetParamOps(actor: string, deviceId: string, presetId: string): Op[] {
  const preset = findNativePreset(presetId);
  if (!preset) throw new Error(`unknown native preset ${presetId}`);
  return Object.entries(preset.params).map(([param, value]) =>
    paramSetOp(actor, deviceId, param, value),
  );
}

/** One `LibraryItem` per preset: the browser-search surface. */
export function seedNativePresetLibrary(): LibraryItem[] {
  return NATIVE_PRESETS.map((p) =>
    LibraryItemSchema.parse({
      id: p.id,
      kind: "Preset",
      name: p.name,
      tags: p.tags,
      text: p.description,
    }),
  );
}
