//! Native preset library: curated starting points for every native device.
//!
//! Teaching note: a preset is just a named param map — no new schema, no
//! DSP, no downloads. Each [`NativePreset`] names a [`DeviceClass`] plus
//! the param values a musician would dial in first (filter sweeps, sampler
//! starter kits, arp patterns, chord sets, level/comp and groove starting
//! points). Application goes through the existing param path:
//! [`apply_preset`] uses [`set_param_value`](super::class::set_param_value)
//! (clamped, typo-loud), and [`preset_op_payloads`] expands one preset to
//! one [`OpKind::ParamSet`](crate::model::OpKind) payload per param, so
//! stamping a preset from the UI is an ordinary undoable op sequence
//! (`op_apply` … `op_undo`, the engine's existing markers).
//!
//! The JSON sidecar is [`DevicePreset`](super::rack::DevicePreset):
//! [`to_device_preset`] freezes any library entry to JSON and back, so the
//! library ships as code here and travels as JSON everywhere else.
//!
//! Browser search surfaces the same entries through
//! [`preset_library_items`]: one [`LibraryItem`](crate::library::LibraryItem)
//! per preset (mirrored by `NATIVE_PRESETS` in
//! `ui/src/browser/presets.ts` — keep the two tables in sync when editing).

use super::class::{classify, set_param_value, DeviceClass};
use super::kernel::DeviceError;
use super::rack::{DevicePreset, RackError};
use crate::library::{LibraryItem, LibraryKind};
use crate::model::Node;

/// One curated preset: which device it fits, what it is called, what it
/// does, and the exact param values to stamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativePreset {
    /// Stable id, also the browser [`LibraryItem`](crate::library::LibraryItem) id.
    pub id: &'static str,
    /// The one device class this preset applies to.
    pub class: DeviceClass,
    /// Display name shown in the browser.
    pub name: &'static str,
    /// One-line musical description (browser search text).
    pub description: &'static str,
    /// Search tags (genre, feel, use).
    pub tags: &'static [&'static str],
    /// `(param_id, value)` pairs. Every id must exist on the class's
    /// [`default_params`](super::class::default_params) and every value
    /// must sit inside its `[min, max]` — the validity test below pins
    /// exact (unclamped) application, so drift fails loud.
    pub params: &'static [(&'static str, f64)],
}

/// The curated library: 58 entries covering all nine DSP classes
/// (everything except `Container` structure and `Foreign` pass-through,
/// which have no musical params to curate).
pub static NATIVE_PRESETS: &[NativePreset] = &[
    // -- gain: level/comp starting points ----------------------------------
    NativePreset {
        id: "gain_unity",
        class: DeviceClass::Gain,
        name: "Unity Gain",
        description: "Clean 0 dB starting point: gain at exactly 1.0, no color",
        tags: &["gain", "utility", "unity", "level", "clean"],
        params: &[("gain", 1.0)],
    },
    NativePreset {
        id: "gain_vocal_ride",
        class: DeviceClass::Gain,
        name: "Vocal Ride",
        description: "Vocal-up balance lift: a gentle level push that sits a lead over the mix",
        tags: &["gain", "vocal", "level", "mix", "lead"],
        params: &[("gain", 1.4)],
    },
    NativePreset {
        id: "gain_parallel_squash",
        class: DeviceClass::Gain,
        name: "Parallel Squash Blend",
        description: "Parallel-compression starting point: hot makeup level meant to blend beside dry",
        tags: &["gain", "parallel", "compression", "comp", "blend", "drums"],
        params: &[("gain", 2.5)],
    },
    NativePreset {
        id: "gain_ghost_fade",
        class: DeviceClass::Gain,
        name: "Ghost Fade",
        description: "Barely-there texture bed: level tucked down to a whisper under the lead",
        tags: &["gain", "quiet", "bed", "texture", "ambient", "level"],
        params: &[("gain", 0.15)],
    },
    // -- lowpass: filter sweeps --------------------------------------------
    NativePreset {
        id: "lp_warm_pad",
        class: DeviceClass::Lowpass,
        name: "Warm Pad Softener",
        description: "Warm analog-style pad filter: lows kept, harsh top rounded off",
        tags: &["filter", "lowpass", "warm", "pad", "ambient", "sweep"],
        params: &[("cutoff", 800.0)],
    },
    NativePreset {
        id: "lp_sweep_mid",
        class: DeviceClass::Lowpass,
        name: "Sweep Midpoint",
        description: "Filter-sweep midpoint: open enough for presence, ready to automate down or up",
        tags: &["filter", "lowpass", "sweep", "automation", "build", "riser"],
        params: &[("cutoff", 2500.0)],
    },
    NativePreset {
        id: "lp_sub_saver",
        class: DeviceClass::Lowpass,
        name: "Sub Saver",
        description: "Sub-only keeper: everything above the bass floor filtered away for 808s and subs",
        tags: &["filter", "lowpass", "sub", "bass", "808", "dark"],
        params: &[("cutoff", 120.0)],
    },
    // -- highpass: cleanup / air -------------------------------------------
    NativePreset {
        id: "hp_vocal_cleanup",
        class: DeviceClass::Highpass,
        name: "Vocal Cleanup",
        description: "Vocal low-cut starting point: rumble and handling noise gone, body kept",
        tags: &["filter", "highpass", "vocal", "cleanup", "rumble", "mix"],
        params: &[("cutoff", 100.0)],
    },
    NativePreset {
        id: "hp_tighten_low",
        class: DeviceClass::Highpass,
        name: "Low-End Tightener",
        description: "Mix-bus tightening cut: sub mud trimmed so the kick and bass stop fighting",
        tags: &["filter", "highpass", "tight", "mix", "kick", "bass"],
        params: &[("cutoff", 40.0)],
    },
    NativePreset {
        id: "hp_radio_thin",
        class: DeviceClass::Highpass,
        name: "Radio Thin",
        description: "Telephone-verse effect: thin lo-fi band feel for intros and breakdowns",
        tags: &["filter", "highpass", "lofi", "radio", "effect", "verse"],
        params: &[("cutoff", 500.0)],
    },
    // -- delay: groove echoes ----------------------------------------------
    NativePreset {
        id: "dly_slapback",
        class: DeviceClass::Delay,
        name: "Rockabilly Slapback",
        description: "Classic slapback: one short repeat that thickens vocals and guitars",
        tags: &["delay", "slapback", "vocal", "guitar", "rock", "echo"],
        params: &[("delay_samples", 3500.0), ("feedback", 0.15)],
    },
    NativePreset {
        id: "dly_dotted_groove",
        class: DeviceClass::Delay,
        name: "Dotted Groove Echo",
        description: "Dotted-eighth style groove echo: repeats that bounce between the beats",
        tags: &["delay", "groove", "dotted", "echo", "synth", "dance"],
        params: &[("delay_samples", 12000.0), ("feedback", 0.45)],
    },
    NativePreset {
        id: "dly_tight_doubler",
        class: DeviceClass::Delay,
        name: "Tight Doubler",
        description: "Vocal doubler starting point: a barely-delayed shadow that widens without echo",
        tags: &["delay", "doubler", "vocal", "wide", "chorus", "groove"],
        params: &[("delay_samples", 1500.0), ("feedback", 0.3)],
    },
    // -- distortion: grit ladder -------------------------------------------
    NativePreset {
        id: "dist_edge",
        class: DeviceClass::Distortion,
        name: "Just an Edge",
        description: "Barely-breaking-up warmth: a hint of hair for bass and keys",
        tags: &["distortion", "drive", "warm", "subtle", "bass", "saturation"],
        params: &[("drive", 0.6)],
    },
    NativePreset {
        id: "dist_warm_drive",
        class: DeviceClass::Distortion,
        name: "Warm Drive",
        description: "Amp-style warm overdrive: guitars and synths pushed into singing sustain",
        tags: &["distortion", "drive", "overdrive", "guitar", "synth", "warm"],
        params: &[("drive", 2.0)],
    },
    NativePreset {
        id: "dist_fuzz_wall",
        class: DeviceClass::Distortion,
        name: "Fuzz Wall",
        description: "All-out fuzz wall: drums and bass crushed into a solid riff block",
        tags: &["distortion", "fuzz", "drive", "heavy", "drums", "wall"],
        params: &[("drive", 8.0)],
    },
    // -- sampler: starter kits ---------------------------------------------
    NativePreset {
        id: "smp_kick_starter",
        class: DeviceClass::Sampler,
        name: "Kick Starter Kit",
        description: "Punchy kick-drum starting point: instant attack, short release, full-range tone",
        tags: &["sampler", "kick", "drums", "starter", "kit", "punch"],
        params: &[
            ("transpose", 0.0),
            ("gain", 1.0),
            ("attack", 0.001),
            ("release", 0.15),
            ("cutoff", 8000.0),
        ],
    },
    NativePreset {
        id: "smp_soft_keys",
        class: DeviceClass::Sampler,
        name: "Soft Keys Pad",
        description: "Mellow sampled-keys bed: slow attack, long release, darkened tone filter",
        tags: &["sampler", "keys", "pad", "soft", "mellow", "starter"],
        params: &[
            ("transpose", 0.0),
            ("gain", 0.8),
            ("attack", 0.08),
            ("release", 0.8),
            ("cutoff", 3500.0),
        ],
    },
    NativePreset {
        id: "smp_chip_blip",
        class: DeviceClass::Sampler,
        name: "Chip Blip",
        description: "8-bit style blip an octave up: snappy, bright, and slightly tucked back",
        tags: &["sampler", "chip", "8bit", "blip", "lead", "octave"],
        params: &[
            ("transpose", 12.0),
            ("gain", 0.9),
            ("attack", 0.002),
            ("release", 0.1),
            ("cutoff", 20000.0),
        ],
    },
    // -- arpeggiator: arp patterns -----------------------------------------
    NativePreset {
        id: "arp_sparkle_16ths",
        class: DeviceClass::Arpeggiator,
        name: "Sparkle 16ths Up",
        description: "Glassy rising 16th-note arp over two octaves for pop verses and lifts",
        tags: &["arp", "arpeggiator", "16ths", "up", "sparkle", "pop"],
        params: &[
            ("arp_mode", 0.0),
            ("arp_rate", 0.25),
            ("arp_gate", 0.8),
            ("arp_octaves", 2.0),
            ("arp_seed", 0.0),
        ],
    },
    NativePreset {
        id: "arp_neon_down",
        class: DeviceClass::Arpeggiator,
        name: "Neon Down 8ths",
        description: "Falling neon 8th-note arp: synthwave basslines and midnight drive patterns",
        tags: &["arp", "arpeggiator", "down", "8ths", "synthwave", "bass"],
        params: &[
            ("arp_mode", 1.0),
            ("arp_rate", 0.5),
            ("arp_gate", 0.9),
            ("arp_octaves", 1.0),
            ("arp_seed", 0.0),
        ],
    },
    NativePreset {
        id: "arp_casino_random",
        class: DeviceClass::Arpeggiator,
        name: "Casino Random",
        description: "Generative casino arp: seeded-random 16ths across three octaves, same every render",
        tags: &["arp", "arpeggiator", "random", "generative", "ambient", "pattern"],
        params: &[
            ("arp_mode", 3.0),
            ("arp_rate", 0.25),
            ("arp_gate", 0.6),
            ("arp_octaves", 3.0),
            ("arp_seed", 7.0),
        ],
    },
    // -- chord: chord sets ---------------------------------------------------
    NativePreset {
        id: "chd_sad_triad",
        class: DeviceClass::Chord,
        name: "Sad Triad",
        description: "Lo-fi minor triad in close voicing: every held note turns moody",
        tags: &["chord", "minor", "triad", "lofi", "sad", "close"],
        params: &[("chord_type", 1.0), ("chord_inversion", 0.0), ("chord_voicing", 0.0)],
    },
    NativePreset {
        id: "chd_open_hymn",
        class: DeviceClass::Chord,
        name: "Open Hymn",
        description: "Wide-open major spread for ambient beds and worship pads",
        tags: &["chord", "major", "open", "ambient", "hymn", "pad"],
        params: &[("chord_type", 0.0), ("chord_inversion", 0.0), ("chord_voicing", 2.0)],
    },
    NativePreset {
        id: "chd_neo_sus",
        class: DeviceClass::Chord,
        name: "Neo-Soul Sus",
        description: "Neo-soul suspended color: first-inversion sus4 in drop-2 voicing",
        tags: &["chord", "sus4", "neosoul", "rnb", "drop2", "keys"],
        params: &[("chord_type", 3.0), ("chord_inversion", 1.0), ("chord_voicing", 1.0)],
    },
    // -- humanize: groove starting points ------------------------------------
    NativePreset {
        id: "hum_groove_light",
        class: DeviceClass::Humanize,
        name: "Light Groove",
        description: "Barely-felt pocket: quantized parts loosened just enough to breathe",
        tags: &["humanize", "groove", "light", "pocket", "feel", "timing"],
        params: &[("hum_timing", 0.008), ("hum_velocity", 5.0), ("hum_seed", 1.0)],
    },
    NativePreset {
        id: "hum_drunk_drums",
        class: DeviceClass::Humanize,
        name: "Drunk Drums",
        description: "Dilla-style slouch for drum breaks: timing dragged, velocities swung hard",
        tags: &["humanize", "groove", "drums", "swing", "dilla", "lofi"],
        params: &[("hum_timing", 0.03), ("hum_velocity", 14.0), ("hum_seed", 42.0)],
    },
    NativePreset {
        id: "hum_human_keys",
        class: DeviceClass::Humanize,
        name: "Human Keys",
        description: "Played-not-programmed piano feel: gentle timing drift and dynamic touch",
        tags: &["humanize", "keys", "piano", "feel", "timing", "velocity"],
        params: &[("hum_timing", 0.015), ("hum_velocity", 10.0), ("hum_seed", 3.0)],
    },
    // -- gain: more comp/mix starting points ---------------------------------
    NativePreset {
        id: "gain_bass_glue",
        class: DeviceClass::Gain,
        name: "Bass Glue",
        description: "Mix-glue level for bass: tucked just under unity so the low end sits without pumping",
        tags: &["gain", "bass", "glue", "mix", "level"],
        params: &[("gain", 0.8)],
    },
    NativePreset {
        id: "gain_drum_crush",
        class: DeviceClass::Gain,
        name: "Drum Crush Bus",
        description: "Crush-bus makeup level: hot smashed-room tone meant to tuck under the dry drums",
        tags: &["gain", "drums", "crush", "bus", "parallel", "comp"],
        params: &[("gain", 3.0)],
    },
    NativePreset {
        id: "gain_ambient_wash",
        class: DeviceClass::Gain,
        name: "Ambient Wash Bed",
        description: "Reverb-wash bed level: quiet bloom that fills gaps without masking the lead",
        tags: &["gain", "ambient", "wash", "bed", "mix", "level"],
        params: &[("gain", 0.4)],
    },
    // -- lowpass: more filter sweeps -----------------------------------------
    NativePreset {
        id: "lp_dark_drone",
        class: DeviceClass::Lowpass,
        name: "Dark Drone",
        description: "Sub-drone filter: only the lowest rumble survives for dark ambient beds",
        tags: &["filter", "lowpass", "drone", "dark", "ambient", "sub"],
        params: &[("cutoff", 200.0)],
    },
    NativePreset {
        id: "lp_airy_open",
        class: DeviceClass::Lowpass,
        name: "Airy Open Top",
        description: "Wide-open air filter: full brightness kept, just the harshest edge shaved off",
        tags: &["filter", "lowpass", "airy", "open", "bright", "mix"],
        params: &[("cutoff", 12000.0)],
    },
    NativePreset {
        id: "lp_riser_open",
        class: DeviceClass::Lowpass,
        name: "Riser Opener",
        description: "Build-riser starting point: mid-open filter ready to automate wide for the drop",
        tags: &["filter", "lowpass", "riser", "build", "automation", "drop"],
        params: &[("cutoff", 5000.0)],
    },
    // -- highpass: more cleanup / air ----------------------------------------
    NativePreset {
        id: "hp_sub_cleanup",
        class: DeviceClass::Highpass,
        name: "Sub Cleanup",
        description: "Gentle sub-sonic trim: inaudible mud gone, kick weight fully kept",
        tags: &["filter", "highpass", "sub", "cleanup", "kick", "mix"],
        params: &[("cutoff", 60.0)],
    },
    NativePreset {
        id: "hp_hihat_sheen",
        class: DeviceClass::Highpass,
        name: "Hi-Hat Sheen",
        description: "Hat-and-air keeper: only sizzle survives for crisp top loops and shakers",
        tags: &["filter", "highpass", "hihat", "sheen", "air", "crisp"],
        params: &[("cutoff", 8000.0)],
    },
    NativePreset {
        id: "hp_dj_rolloff",
        class: DeviceClass::Highpass,
        name: "DJ Rolloff",
        description: "DJ-style low rolloff: bass thinned for transitions, mids and highs intact",
        tags: &["filter", "highpass", "dj", "transition", "rolloff", "mix"],
        params: &[("cutoff", 150.0)],
    },
    // -- delay: more groove echoes -------------------------------------------
    NativePreset {
        id: "dly_ambient_wash",
        class: DeviceClass::Delay,
        name: "Ambient Wash Echo",
        description: "Washed-out ambient echo: long repeats that melt guitars into pads",
        tags: &["delay", "ambient", "wash", "echo", "guitar", "pad"],
        params: &[("delay_samples", 24000.0), ("feedback", 0.6)],
    },
    NativePreset {
        id: "dly_rhythmic_8th",
        class: DeviceClass::Delay,
        name: "Rhythmic 8th Tap",
        description: "Straight-8th rhythmic tap: repeats that lock to the groove for keys and plucks",
        tags: &["delay", "rhythmic", "groove", "keys", "pluck", "echo"],
        params: &[("delay_samples", 6000.0), ("feedback", 0.35)],
    },
    NativePreset {
        id: "dly_dub_feedback",
        class: DeviceClass::Delay,
        name: "Dub Feedback Throw",
        description: "Dub-throw echo: hot feedback for one-shot sends that spiral into space",
        tags: &["delay", "dub", "feedback", "throw", "reggae", "echo"],
        params: &[("delay_samples", 18000.0), ("feedback", 0.7)],
    },
    // -- distortion: more grit -------------------------------------------------
    NativePreset {
        id: "dist_tape_sat",
        class: DeviceClass::Distortion,
        name: "Tape Saturation",
        description: "Soft tape-style saturation: gentle glue and harmonics for the whole mix bus",
        tags: &["distortion", "tape", "saturation", "glue", "mix", "warm"],
        params: &[("drive", 1.2)],
    },
    NativePreset {
        id: "dist_punk_crunch",
        class: DeviceClass::Distortion,
        name: "Punk Crunch",
        description: "Punk-rock crunch: rhythm guitars chewed into aggressive midrange bark",
        tags: &["distortion", "punk", "crunch", "guitar", "rock", "drive"],
        params: &[("drive", 4.5)],
    },
    NativePreset {
        id: "dist_doom_sustain",
        class: DeviceClass::Distortion,
        name: "Doom Sustain",
        description: "Doom-metal sustain: heavy sagging drive for slow riffs that ring forever",
        tags: &["distortion", "doom", "metal", "sustain", "heavy", "riff"],
        params: &[("drive", 6.0)],
    },
    // -- sampler: more starter kits --------------------------------------------
    // Teaching note: a frozen node holds play params only — the audio
    // itself lives beside the project. Each kit below is the envelope /
    // tone half of a starter kit; pair it with a builtin or recorded-take
    // sample buffer loaded onto the same device id.
    NativePreset {
        id: "smp_boom_808",
        class: DeviceClass::Sampler,
        name: "Boom 808 Kit",
        description: "Boom-808 kit starting point: dropped an octave, long tail, dark tone — pair with a builtin or take 808 sample",
        tags: &["sampler", "808", "bass", "kit", "boom", "starter"],
        params: &[
            ("transpose", -12.0),
            ("gain", 1.2),
            ("attack", 0.004),
            ("release", 0.4),
            ("cutoff", 900.0),
        ],
    },
    NativePreset {
        id: "smp_snare_crack",
        class: DeviceClass::Sampler,
        name: "Snare Crack Kit",
        description: "Cracking snare starting point: instant attack, tight tail, open tone — pair with a builtin or take snare sample",
        tags: &["sampler", "snare", "drums", "kit", "crack", "starter"],
        params: &[
            ("transpose", 0.0),
            ("gain", 1.1),
            ("attack", 0.001),
            ("release", 0.12),
            ("cutoff", 12000.0),
        ],
    },
    NativePreset {
        id: "smp_hihat_tight",
        class: DeviceClass::Sampler,
        name: "Tight Hat Kit",
        description: "Tight closed-hat starting point: pitched up, short tick, bright tone — pair with a builtin or take hat sample",
        tags: &["sampler", "hihat", "hat", "drums", "kit", "tight"],
        params: &[
            ("transpose", 6.0),
            ("gain", 0.7),
            ("attack", 0.001),
            ("release", 0.05),
            ("cutoff", 15000.0),
        ],
    },
    NativePreset {
        id: "smp_vocal_chop",
        class: DeviceClass::Sampler,
        name: "Vocal Chop Kit",
        description: "Chopped-vocal starting point: soft attack, medium tail, smoothed tone — pair with a builtin or take vocal sample",
        tags: &["sampler", "vocal", "chop", "kit", "starter", "sliced"],
        params: &[
            ("transpose", 0.0),
            ("gain", 0.9),
            ("attack", 0.01),
            ("release", 0.5),
            ("cutoff", 6000.0),
        ],
    },
    // -- arpeggiator: more rhythmic patterns -----------------------------------
    NativePreset {
        id: "arp_updown_runner",
        class: DeviceClass::Arpeggiator,
        name: "Up-Down Runner 16ths",
        description: "Endless up-down 16th runner over two octaves for trance gates and synth runs",
        tags: &["arp", "arpeggiator", "updown", "16ths", "trance", "pattern"],
        params: &[
            ("arp_mode", 2.0),
            ("arp_rate", 0.25),
            ("arp_gate", 0.7),
            ("arp_octaves", 2.0),
            ("arp_seed", 0.0),
        ],
    },
    NativePreset {
        id: "arp_slow_bloom",
        class: DeviceClass::Arpeggiator,
        name: "Slow Bloom Whole",
        description: "Slow-blooming whole-note arp: rising pads that open one note per bar",
        tags: &["arp", "arpeggiator", "slow", "bloom", "pad", "ambient"],
        params: &[
            ("arp_mode", 0.0),
            ("arp_rate", 1.0),
            ("arp_gate", 0.9),
            ("arp_octaves", 1.0),
            ("arp_seed", 0.0),
        ],
    },
    NativePreset {
        id: "arp_gallop_dotted",
        class: DeviceClass::Arpeggiator,
        name: "Gallop Dotted 8ths",
        description: "Galloping dotted-8th up-down arp: locked groove for synthwave leads",
        tags: &["arp", "arpeggiator", "gallop", "dotted", "synthwave", "lead"],
        params: &[
            ("arp_mode", 2.0),
            ("arp_rate", 0.75),
            ("arp_gate", 0.5),
            ("arp_octaves", 1.0),
            ("arp_seed", 0.0),
        ],
    },
    NativePreset {
        id: "arp_stardust",
        class: DeviceClass::Arpeggiator,
        name: "Stardust Scatter 8ths",
        description: "Scattered generative 8th-note arp over two octaves: twinkle that never repeats its bar",
        tags: &["arp", "arpeggiator", "random", "generative", "twinkle", "pattern"],
        params: &[
            ("arp_mode", 3.0),
            ("arp_rate", 0.5),
            ("arp_gate", 0.5),
            ("arp_octaves", 2.0),
            ("arp_seed", 11.0),
        ],
    },
    // -- chord: progression-flavored chord sets ----------------------------------
    // Teaching note: the chord device voices one held root (type /
    // inversion / voicing only — no key param), so each progression
    // starting point below is the voicing color for that degree: stamp
    // the ii shape, then the V, then the I across your clips in any key.
    NativePreset {
        id: "chd_ii_v_i_major",
        class: DeviceClass::Chord,
        name: "ii-V-I Bright Major",
        description: "Bright ii-V-I major color in C: first-inversion close major for the I (transpose the clips for other keys)",
        tags: &["chord", "major", "two-five-one", "progression", "jazz", "pop"],
        params: &[("chord_type", 0.0), ("chord_inversion", 1.0), ("chord_voicing", 0.0)],
    },
    NativePreset {
        id: "chd_ii_v_i_minor",
        class: DeviceClass::Chord,
        name: "ii-V-i Smoky Minor",
        description: "Smoky ii-V-i minor color in A minor: open minor with the fifth up top for the i (transpose the clips for other keys)",
        tags: &["chord", "minor", "two-five-one", "progression", "jazz", "lofi"],
        params: &[("chord_type", 1.0), ("chord_inversion", 1.0), ("chord_voicing", 2.0)],
    },
    NativePreset {
        id: "chd_dim_passing",
        class: DeviceClass::Chord,
        name: "Dim Passing Chord",
        description: "Diminished passing color in G: close dim7 that walks between verse chords in any key",
        tags: &["chord", "dim", "passing", "progression", "jazz", "walk"],
        params: &[("chord_type", 2.0), ("chord_inversion", 0.0), ("chord_voicing", 1.0)],
    },
    NativePreset {
        id: "chd_gospel_shout",
        class: DeviceClass::Chord,
        name: "Gospel Shout Drop-2",
        description: "Gospel shout color in F: second-inversion major in drop-2 for uplifting turnarounds in any key",
        tags: &["chord", "major", "gospel", "drop2", "progression", "uplift"],
        params: &[("chord_type", 0.0), ("chord_inversion", 2.0), ("chord_voicing", 1.0)],
    },
    // -- humanize: more groove starting points -----------------------------------
    NativePreset {
        id: "hum_tight_pocket",
        class: DeviceClass::Humanize,
        name: "Tight Pocket",
        description: "Near-grid pocket: just enough timing and velocity nudge to dodge the robot feel",
        tags: &["humanize", "groove", "tight", "pocket", "timing", "pop"],
        params: &[("hum_timing", 0.004), ("hum_velocity", 3.0), ("hum_seed", 5.0)],
    },
    NativePreset {
        id: "hum_lofi_swing",
        class: DeviceClass::Humanize,
        name: "Lofi Swing Dust",
        description: "Dusty lo-fi swing: swung timing and soft-hit velocities for sleepy boom-bap loops",
        tags: &["humanize", "groove", "lofi", "swing", "boombap", "dust"],
        params: &[("hum_timing", 0.02), ("hum_velocity", 12.0), ("hum_seed", 9.0)],
    },
    NativePreset {
        id: "hum_live_drums",
        class: DeviceClass::Humanize,
        name: "Live Kit Energy",
        description: "Live-kit energy: hard timing slop and dynamic hits for rock drum takes",
        tags: &["humanize", "groove", "drums", "live", "rock", "energy"],
        params: &[("hum_timing", 0.045), ("hum_velocity", 18.0), ("hum_seed", 17.0)],
    },
];

/// All curated presets (see [`NATIVE_PRESETS`]).
pub fn native_presets() -> &'static [NativePreset] {
    NATIVE_PRESETS
}

/// Look up one preset by id. Unknown ids are `None`, never an error —
/// the browser just shows no preset row.
pub fn find_preset(id: &str) -> Option<&'static NativePreset> {
    NATIVE_PRESETS.iter().find(|p| p.id == id)
}

/// Stamp a preset onto a live device node: every param set through the
/// existing clamped path, so out-of-range values settle instead of
/// corrupting and unknown ids surface as [`DeviceError::UnknownParam`].
/// A preset only fits its own class — stamping across classes is the
/// same typo-loud error, never a silent retune.
pub fn apply_preset(node: &mut Node, preset: &NativePreset) -> Result<(), DeviceError> {
    if classify(node) != preset.class {
        return Err(DeviceError::UnknownParam(format!(
            "preset {} needs {:?}, node {} is {:?}",
            preset.id,
            preset.class,
            node.id,
            classify(node)
        )));
    }
    for (param, value) in preset.params {
        set_param_value(node, param, *value)?;
    }
    Ok(())
}

/// Expand one preset to undoable op payloads: one
/// [`OpKind::ParamSet`](crate::model::OpKind) `(target, value_json)` pair
/// per param, where `target` is the `device_id:param` address the engine's
/// `apply` already accepts. Each op undoes independently through the
/// existing `op_undo` markers — no new op kind, no migration.
pub fn preset_op_payloads(device_id: &str, preset: &NativePreset) -> Vec<(String, String)> {
    preset
        .params
        .iter()
        .map(|(param, value)| (format!("{device_id}:{param}"), value.to_string()))
        .collect()
}

/// Freeze one library entry as a [`DevicePreset`] (the JSON sidecar):
/// a fresh node of the preset's class, stamped, then captured. Fails
/// only when the table itself drifts from the class defaults — which is
/// exactly the failure the validity test wants to hear about.
pub fn to_device_preset(preset: &NativePreset) -> Result<DevicePreset, RackError> {
    let mut node = super::class::instantiate(preset.class, "preset_probe", preset.name);
    apply_preset(&mut node, preset).map_err(|e| RackError::BadRack(e.to_string()))?;
    DevicePreset::capture(&node)
}

/// One [`LibraryItem`] per preset: the browser-search surface. `text` is
/// the musical description so concept queries ("warm pad filter",
/// "drunk drum groove") rank the right starting point.
pub fn preset_library_items() -> Vec<LibraryItem> {
    NATIVE_PRESETS
        .iter()
        .map(|p| LibraryItem {
            id: p.id.to_string(),
            kind: LibraryKind::Preset,
            name: p.name.to_string(),
            tags: p.tags.iter().map(|t| t.to_string()).collect(),
            text: p.description.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::class::instantiate;
    use super::*;
    use crate::engine::Engine;
    use crate::model::{OpKind, Project};

    fn scratch(name: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ccez-presets-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn every_preset_applies_cleanly_to_its_device_class() {
        assert!(!NATIVE_PRESETS.is_empty());
        for preset in NATIVE_PRESETS {
            let mut node = instantiate(preset.class, "probe", preset.name);
            apply_preset(&mut node, preset)
                .unwrap_or_else(|e| panic!("preset {} failed to apply: {e}", preset.id));
            // Exact values, not clamped ones: proves every id exists and
            // every value sits inside its [min, max].
            for (param, value) in preset.params {
                let got = node
                    .params
                    .iter()
                    .find(|p| p.id == *param)
                    .unwrap_or_else(|| panic!("preset {} lost param {param}", preset.id));
                assert_eq!(
                    got.value, *value,
                    "preset {} param {param} clamped (table drift?)",
                    preset.id
                );
            }
            // Stamping never changes the class tag.
            assert_eq!(classify(&node), preset.class, "preset {}", preset.id);
        }
    }

    #[test]
    fn every_preset_covers_a_real_class_and_ids_are_unique() {
        let mut ids: Vec<&str> = NATIVE_PRESETS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "duplicate preset ids");
        for preset in NATIVE_PRESETS {
            assert_ne!(preset.class, DeviceClass::Foreign, "preset {}", preset.id);
            assert_ne!(preset.class, DeviceClass::Container, "preset {}", preset.id);
            assert!(!preset.name.is_empty(), "preset {}", preset.id);
            assert!(!preset.description.is_empty(), "preset {}", preset.id);
            assert!(!preset.tags.is_empty(), "preset {}", preset.id);
            assert!(!preset.params.is_empty(), "preset {}", preset.id);
        }
        // Every DSP class has at least two starting points.
        for class in [
            DeviceClass::Gain,
            DeviceClass::Lowpass,
            DeviceClass::Highpass,
            DeviceClass::Delay,
            DeviceClass::Distortion,
            DeviceClass::Sampler,
            DeviceClass::Arpeggiator,
            DeviceClass::Chord,
            DeviceClass::Humanize,
        ] {
            let n = NATIVE_PRESETS.iter().filter(|p| p.class == class).count();
            assert!(n >= 2, "{class:?} has only {n} presets");
        }
    }

    #[test]
    fn every_preset_round_trips_through_device_preset_json() {
        for preset in NATIVE_PRESETS {
            let frozen = to_device_preset(preset).expect("freeze");
            let json = frozen.to_json().expect("to json");
            let back = DevicePreset::from_json(&json).expect("from json");
            assert_eq!(frozen, back, "preset {}", preset.id);
            let node = back.instantiate("restored").expect("instantiate");
            assert_eq!(classify(&node), preset.class, "preset {}", preset.id);
            for (param, value) in preset.params {
                let got = node.params.iter().find(|p| p.id == *param).expect("param");
                assert_eq!(got.value, *value, "preset {} param {param}", preset.id);
            }
        }
    }

    #[test]
    fn preset_stamping_is_undoable_param_sets() {
        let preset = find_preset("dly_dotted_groove").expect("seeded preset");
        let dir = scratch("undo");
        let mut project = Project::new("p", "P");
        project.devices.push(instantiate(DeviceClass::Delay, "dly1", "Echo"));
        let before: Vec<f64> = project.devices[0].params.iter().map(|p| p.value).collect();
        let mut e = Engine::create(&dir, project).expect("create");
        let mut seqs = Vec::new();
        for (target, value_json) in preset_op_payloads("dly1", preset) {
            // Every payload parses as a bare number: the engine's ParamSet path.
            assert!(serde_json::from_str::<f64>(&value_json).is_ok(), "{value_json}");
            seqs.push(e.apply("ui", OpKind::ParamSet, &target, &value_json).expect("apply"));
        }
        assert_eq!(seqs.len(), preset.params.len());
        let live = e.project().devices.iter().find(|d| d.id == "dly1").expect("device");
        for (param, value) in preset.params {
            let got = live.params.iter().find(|p| p.id == *param).expect("param");
            assert_eq!(got.value, *value);
        }
        for _ in &seqs {
            e.undo().expect("undo");
        }
        let restored = e.project().devices.iter().find(|d| d.id == "dly1").expect("device");
        let after: Vec<f64> = restored.params.iter().map(|p| p.value).collect();
        assert_eq!(after, before, "undo must restore exact defaults");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_class_and_unknown_ids_are_loud() {
        let preset = find_preset("lp_warm_pad").expect("seeded preset");
        let mut other = instantiate(DeviceClass::Gain, "g", "G");
        assert!(apply_preset(&mut other, preset).is_err());
        assert!(find_preset("no_such_preset").is_none());
        // A hand-corrupted preset (bad param id) surfaces, not vanishes.
        let bad = NativePreset { id: "bad", params: &[("nope", 1.0)], ..*preset };
        let mut node = instantiate(preset.class, "lp", "LP");
        assert!(apply_preset(&mut node, &bad).is_err());
    }

    #[test]
    fn preset_library_items_are_searchable_presets() {
        let items = preset_library_items();
        assert_eq!(items.len(), NATIVE_PRESETS.len());
        for item in &items {
            assert_eq!(item.kind, LibraryKind::Preset);
            assert!(!item.text.is_empty(), "{}", item.id);
        }
        // A concept query over seeds + presets finds the groove preset.
        let mut lib = crate::library::seed_library();
        lib.extend(items);
        let hits = crate::library::search("drunk swung drum groove with slouch", &lib, 3);
        assert!(
            hits.iter().any(|h| h.id == "hum_drunk_drums"),
            "groove preset should rank top-3, hits: {hits:?}"
        );
    }
}
