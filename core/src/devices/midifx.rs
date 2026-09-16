//! Native MIDI FX devices: musical transforms with no new dependencies.
//!
//! Teaching note: audio devices transform samples; MIDI FX transform
//! *notes*. A frozen [`Node`](crate::model::Node) still carries only
//! numeric params, so each FX reads a small param struct from its node
//! (see [`ArpParams::from_node`], [`ChordParams::from_node`],
//! [`HumanizeParams::from_node`]) and maps a [`MidiClip`] to a new
//! [`MidiClip`]. Like every kernel in [`crate::devices::kernel`], the
//! transforms are pure functions of their inputs plus explicit params —
//! no hidden state, no allocation discipline beyond ordinary `Vec`
//! building (note lists are control data, not the realtime sample path),
//! and no RNG dependency: every random choice derives from a stateless
//! splitmix-style hash over `(seed, index)`, the same deterministic-gate
//! precedent [`crate::midi::transport::expand`] sets for probability.
//!
//! Chain compatibility: MIDI FX ride the same insert chain as audio
//! devices (same [`DeviceClass`](super::class::DeviceClass) tag
//! convention, same [`DevicePreset`](super::rack::DevicePreset) capture).
//! The audio renderer treats them as pass-through — they shape the notes
//! *before* the instrument, never the samples after it. Use
//! [`apply_midi_chain`] to run a track's MIDI FX in chain order over a
//! clip; audio devices in the chain are skipped, the way
//! [`crate::plugins::chain::chain_devices`] skips ids with no state.
//!
//! Devices:
//!
//! - Arpeggiator ([`arp_pattern`], [`arp_render`]): held notes become a
//!   stepped pattern (`up` / `down` / `up-down` / `random`), paced by
//!   `rate` beats per step, note length `gate` of a step, `octaves` of
//!   range, all deterministic under `seed`.
//! - Chord generator ([`chord_spell`], [`chord_render`]): one input note
//!   becomes a chord by type (`maj` / `min` / `dim7` / `sus4`), with
//!   `inversion` rotation and `voicing` spread.
//! - Velocity/humanize ([`humanize_render`]): deterministic per-note
//!   timing and velocity jitter under `seed`, always clamped back into
//!   the [`MidiNote`](crate::midi::MidiNote) valid ranges.

use crate::midi::{MidiClip, MidiNote};
use crate::model::Node;

use super::class::{classify, param_value, DeviceClass};

// -- param ids ---------------------------------------------------------------

/// Arpeggiator pattern mode code (0 = up, 1 = down, 2 = up-down, 3 = random).
pub const ARP_MODE_PARAM: &str = "arp_mode";
/// Arpeggiator step length in beats (0.0625..=4.0, default 0.25 = 16th).
pub const ARP_RATE_PARAM: &str = "arp_rate";
/// Arpeggiator note length as a fraction of one step (0.05..=1.0, default 0.8).
pub const ARP_GATE_PARAM: &str = "arp_gate";
/// Arpeggiator octave range (1..=4, default 1).
pub const ARP_OCTAVES_PARAM: &str = "arp_octaves";
/// Arpeggiator determinism seed (0..=2^32-1, default 0).
pub const ARP_SEED_PARAM: &str = "arp_seed";

/// Chord type code (0 = maj, 1 = min, 2 = dim7, 3 = sus4).
pub const CHORD_TYPE_PARAM: &str = "chord_type";
/// Chord inversion: which chord tone sits in the bass (0..=3, default 0).
pub const CHORD_INVERSION_PARAM: &str = "chord_inversion";
/// Chord voicing (0 = close, 1 = drop-2, 2 = open).
pub const CHORD_VOICING_PARAM: &str = "chord_voicing";

/// Humanize timing jitter in beats, peak (0.0..=0.25, default 0.01).
/// Applied onto `timing_offset_beats`, which the model clamps at ±0.25.
pub const HUM_TIMING_PARAM: &str = "hum_timing";
/// Humanize velocity jitter in velocity steps, peak (0.0..=64.0, default 8.0).
pub const HUM_VELOCITY_PARAM: &str = "hum_velocity";
/// Humanize determinism seed (0..=2^32-1, default 0).
pub const HUM_SEED_PARAM: &str = "hum_seed";

// -- deterministic hash ------------------------------------------------------

/// Stateless uniform hash in `[0, 1)`: the whole "randomness" story for
/// MIDI FX. Same `(key, seed)` always yields the same draw, so renders
/// are reproducible with no RNG state and no new dependency. This is the
/// splitmix64 finalizer, matching the probability gate in
/// [`crate::midi::transport`].
fn hash01(key: u64, seed: u64) -> f64 {
    let mut z = key
        .wrapping_add(0x9E3779B97F4A7C15)
        .wrapping_add(seed.wrapping_mul(0xBF58476D1CE4E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    ((z >> 11) as f64) / ((1u64 << 53) as f64)
}

// -- arpeggiator ---------------------------------------------------------------

/// Arpeggiator pattern direction, stored as [`ARP_MODE_PARAM`] 0..=3.
/// Out-of-range codes clamp (clamp-not-error, the engine `ParamSet` rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpMode {
    Up,
    Down,
    UpDown,
    Random,
}

impl ArpMode {
    pub fn code(self) -> f64 {
        match self {
            Self::Up => 0.0,
            Self::Down => 1.0,
            Self::UpDown => 2.0,
            Self::Random => 3.0,
        }
    }

    pub fn from_code(code: f64) -> Self {
        match code.round().clamp(0.0, 3.0) as i64 {
            1 => Self::Down,
            2 => Self::UpDown,
            3 => Self::Random,
            _ => Self::Up,
        }
    }
}

/// Arpeggiator play params, resolved from a frozen device node.
#[derive(Debug, Clone, PartialEq)]
pub struct ArpParams {
    pub mode: ArpMode,
    pub rate_beats: f64,
    pub gate: f64,
    pub octaves: u8,
    pub seed: u64,
}

impl ArpParams {
    pub fn from_node(node: &Node) -> Self {
        debug_assert_eq!(classify(node), DeviceClass::Arpeggiator);
        Self {
            mode: ArpMode::from_code(param_value(node, ARP_MODE_PARAM, 0.0)),
            rate_beats: param_value(node, ARP_RATE_PARAM, 0.25).clamp(0.0625, 4.0),
            gate: param_value(node, ARP_GATE_PARAM, 0.8).clamp(0.05, 1.0),
            octaves: param_value(node, ARP_OCTAVES_PARAM, 1.0).clamp(1.0, 4.0) as u8,
            seed: param_value(node, ARP_SEED_PARAM, 0.0).clamp(0.0, 4294967295.0) as u64,
        }
    }
}

/// One arpeggiator step count helper: how many `rate`-long steps cover
/// `length_beats` (always at least one; a clip always sounds something).
fn arp_step_count(length_beats: f64, rate_beats: f64) -> usize {
    ((length_beats / rate_beats).ceil().max(1.0)) as usize
}

/// Pure pitch pattern: `steps` pitches walked from the held notes.
///
/// The pool is the sorted held pitches extended over `octaves`
/// (`pitch + 12 * o`, octave copies at 128+ are dropped — MIDI stops at
/// 127). `Up` cycles low to high, `Down` high to low, `UpDown`
/// ping-pongs without repeating the end points (a single-note pool just
/// repeats), and `Random` draws `hash01(step, seed)` per step. Empty
/// `held` yields no steps — silence in, silence out.
pub fn arp_pattern(held: &[u8], mode: ArpMode, octaves: u8, seed: u64, steps: usize) -> Vec<u8> {
    let mut base: Vec<u8> = held.to_vec();
    base.sort_unstable();
    base.dedup();
    if base.is_empty() || steps == 0 {
        return Vec::new();
    }
    let octaves = octaves.clamp(1, 4) as usize;
    let mut pool: Vec<u8> = Vec::new();
    for o in 0..octaves {
        for p in &base {
            if let Some(q) = p.checked_add(12 * o as u8) {
                if q <= 127 {
                    pool.push(q);
                }
            }
        }
    }
    if pool.is_empty() {
        return Vec::new();
    }
    let n = pool.len();
    match mode {
        ArpMode::Up => (0..steps).map(|i| pool[i % n]).collect(),
        ArpMode::Down => (0..steps).map(|i| pool[n - 1 - (i % n)]).collect(),
        ArpMode::UpDown => {
            if n == 1 {
                return vec![pool[0]; steps];
            }
            let cycle = 2 * n - 2;
            (0..steps)
                .map(|i| {
                    let k = i % cycle;
                    if k < n { pool[k] } else { pool[cycle - k] }
                })
                .collect()
        }
        ArpMode::Random => (0..steps)
            .map(|i| {
                let draw = hash01(i as u64, seed);
                pool[(draw * n as f64).floor() as usize % n]
            })
            .collect(),
    }
}

/// Render a clip through the arpeggiator: the sorted unique pitches of
/// the clip's unmuted notes are the held chord; the output spans the
/// clip length in `rate` steps, each `rate * gate` beats long, at the
/// held notes' first velocity. Output notes are re-id'd from 1 in step
/// order (the arp is one gesture, like ratchet sub-notes sharing an id
/// family — here fresh ids keep [`MidiClip`] uniqueness trivially).
pub fn arp_render(clip: &MidiClip, params: &ArpParams) -> MidiClip {
    let mut held: Vec<u8> = Vec::new();
    let mut velocity = 100u8;
    let mut first = true;
    let mut ordered: Vec<&MidiNote> = clip.notes.iter().filter(|n| !n.muted).collect();
    ordered.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pitch.cmp(&b.pitch))
    });
    for n in &ordered {
        if first {
            velocity = n.velocity;
            first = false;
        }
        if !held.contains(&n.pitch) {
            held.push(n.pitch);
        }
    }
    let steps = arp_step_count(clip.length_beats, params.rate_beats);
    let pitches = arp_pattern(&held, params.mode, params.octaves, params.seed, steps);
    let mut out = MidiClip::new(clip.length_beats);
    for (i, pitch) in pitches.iter().enumerate() {
        let start = i as f64 * params.rate_beats;
        let len = (params.rate_beats * params.gate).max(1.0 / 480.0);
        let _ = out.add_note(MidiNote::new((i + 1) as u32, *pitch, velocity, start, len));
    }
    out
}

// -- chord generator -----------------------------------------------------------

/// Chord quality, stored as [`CHORD_TYPE_PARAM`] 0..=3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordType {
    /// Major triad: root, major third, fifth.
    Major,
    /// Minor triad: root, minor third, fifth.
    Minor,
    /// Diminished seventh: root, minor third, tritone, sixth.
    Dim7,
    /// Suspended fourth: root, fourth, fifth.
    Sus4,
}

impl ChordType {
    pub fn code(self) -> f64 {
        match self {
            Self::Major => 0.0,
            Self::Minor => 1.0,
            Self::Dim7 => 2.0,
            Self::Sus4 => 3.0,
        }
    }

    pub fn from_code(code: f64) -> Self {
        match code.round().clamp(0.0, 3.0) as i64 {
            1 => Self::Minor,
            2 => Self::Dim7,
            3 => Self::Sus4,
            _ => Self::Major,
        }
    }

    /// Semitone offsets above the root, bass first.
    pub fn intervals(self) -> &'static [i8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Dim7 => &[0, 3, 6, 9],
            Self::Sus4 => &[0, 5, 7],
        }
    }
}

/// Chord voicing spread, stored as [`CHORD_VOICING_PARAM`] 0..=2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voicing {
    /// All chord tones in one octave.
    Close,
    /// Second voice from the top drops an octave (classic drop-2).
    Drop2,
    /// Every other upper voice rises an octave (open spread).
    Open,
}

impl Voicing {
    pub fn code(self) -> f64 {
        match self {
            Self::Close => 0.0,
            Self::Drop2 => 1.0,
            Self::Open => 2.0,
        }
    }

    pub fn from_code(code: f64) -> Self {
        match code.round().clamp(0.0, 2.0) as i64 {
            1 => Self::Drop2,
            2 => Self::Open,
            _ => Self::Close,
        }
    }
}

/// Chord play params, resolved from a frozen device node.
#[derive(Debug, Clone, PartialEq)]
pub struct ChordParams {
    pub chord: ChordType,
    /// Which chord tone sits in the bass (rotates low notes up an
    /// octave); clamps to the chord's own tone count.
    pub inversion: u8,
    pub voicing: Voicing,
}

impl ChordParams {
    pub fn from_node(node: &Node) -> Self {
        debug_assert_eq!(classify(node), DeviceClass::Chord);
        Self {
            chord: ChordType::from_code(param_value(node, CHORD_TYPE_PARAM, 0.0)),
            inversion: param_value(node, CHORD_INVERSION_PARAM, 0.0).clamp(0.0, 3.0) as u8,
            voicing: Voicing::from_code(param_value(node, CHORD_VOICING_PARAM, 0.0)),
        }
    }
}

/// Spell one chord above `root`: intervals, then `inversion` low notes
/// lifted an octave, then the `voicing` spread. Tones landing above 127
/// are dropped (a chord never wraps around into a bass rumble); when
/// everything drops out, the root alone survives so silence never
/// replaces a note.
pub fn chord_spell(root: u8, chord: ChordType, inversion: u8, voicing: Voicing) -> Vec<u8> {
    let iv = chord.intervals();
    let mut tones: Vec<i16> = iv.iter().map(|s| root as i16 + *s as i16).collect();
    let inv = (inversion as usize).min(tones.len().saturating_sub(1));
    for t in tones.iter_mut().take(inv) {
        *t += 12;
    }
    if inv > 0 {
        tones.sort_unstable();
    }
    match voicing {
        Voicing::Close => {}
        Voicing::Drop2 => {
            if tones.len() >= 2 {
                let k = tones.len() - 2;
                tones[k] -= 12;
                tones.sort_unstable();
            }
        }
        Voicing::Open => {
            for (i, t) in tones.iter_mut().enumerate() {
                if i % 2 == 1 {
                    *t += 12;
                }
            }
            tones.sort_unstable();
        }
    }
    let mut out: Vec<u8> = tones
        .into_iter()
        .filter_map(|t| u8::try_from(t).ok())
        .collect();
    if out.is_empty() {
        out.push(root.min(127));
    }
    out
}

/// Render a clip through the chord generator: every unmuted input note
/// keeps its timing/velocity/expression and gains chord siblings above
/// it. Muted notes pass through untouched (kept for editing, like
/// [`crate::midi::transport::expand`] keeps them out of playback).
/// Sibling ids derive from the parent (`parent * 8 + index`, wrapping on
/// overflow); on the vanishing chance of a collision the sibling keeps
/// the next free id, so the output always decodes.
pub fn chord_render(clip: &MidiClip, params: &ChordParams) -> MidiClip {
    let mut out = MidiClip::new(clip.length_beats);
    for note in &clip.notes {
        if note.muted {
            let _ = out.add_note(note.clone());
            continue;
        }
        let tones = chord_spell(note.pitch, params.chord, params.inversion, params.voicing);
        for (k, pitch) in tones.iter().enumerate() {
            let mut sib = note.clone();
            if k > 0 {
                sib.note_id = note.note_id.wrapping_mul(8).wrapping_add(k as u32);
                while out.notes.iter().any(|n| n.note_id == sib.note_id) {
                    sib.note_id = sib.note_id.wrapping_add(1);
                }
            }
            sib.pitch = *pitch;
            let _ = out.add_note(sib);
        }
    }
    out
}

// -- velocity / humanize --------------------------------------------------------

/// Humanize params, resolved from a frozen device node.
#[derive(Debug, Clone, PartialEq)]
pub struct HumanizeParams {
    /// Peak timing jitter in beats (0.0..=0.25).
    pub timing_beats: f64,
    /// Peak velocity jitter in velocity steps (0.0..=64.0).
    pub velocity: f64,
    pub seed: u64,
}

impl HumanizeParams {
    pub fn from_node(node: &Node) -> Self {
        debug_assert_eq!(classify(node), DeviceClass::Humanize);
        Self {
            timing_beats: param_value(node, HUM_TIMING_PARAM, 0.01).clamp(0.0, 0.25),
            velocity: param_value(node, HUM_VELOCITY_PARAM, 8.0).clamp(0.0, 64.0),
            seed: param_value(node, HUM_SEED_PARAM, 0.0).clamp(0.0, 4294967295.0) as u64,
        }
    }
}

/// Humanize one clip: every unmuted note gets a deterministic bipolar
/// draw for timing (`timing_offset_beats`, clamped to the model's
/// ±0.25) and one for velocity (rounded, clamped to 1..=127 — velocity
/// 0 stays reserved for note-off). Draw `2k` drives timing and `2k+1`
/// velocity so the two dimensions stay independent. Muted notes pass
/// through byte-identical. Same `(clip, params)` always renders the
/// same take.
pub fn humanize_render(clip: &MidiClip, params: &HumanizeParams) -> MidiClip {
    let mut out = MidiClip::new(clip.length_beats);
    for note in &clip.notes {
        if note.muted {
            let _ = out.add_note(note.clone());
            continue;
        }
        let mut h = note.clone();
        let t_draw = hash01(note.note_id as u64 * 2, params.seed);
        let v_draw = hash01(note.note_id as u64 * 2 + 1, params.seed);
        h.timing_offset_beats = (note.timing_offset_beats
            + (t_draw * 2.0 - 1.0) * params.timing_beats)
            .clamp(-0.25, 0.25);
        let v_shift = ((v_draw * 2.0 - 1.0) * params.velocity).round() as i16;
        h.velocity = (note.velocity as i16 + v_shift).clamp(1, 127) as u8;
        let _ = out.add_note(h);
    }
    out
}

// -- chain apply ---------------------------------------------------------------

/// Run a track's MIDI FX devices in chain order over `clip`.
///
/// Each device whose class is Arpeggiator/Chord/Humanize transforms the
/// clip in turn; every other node (audio devices, containers, foreign
/// nodes) is skipped — the mirror of the audio renderer's pass-through
/// for classes it does not shape. Unknown tracks are an error (the
/// [`crate::plugins::chain::chain_order`] rule); an empty FX chain
/// returns the clip unchanged.
pub fn apply_midi_chain(
    project: &crate::model::Project,
    track_id: &str,
    clip: &MidiClip,
) -> Result<MidiClip, String> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| format!("unknown midi chain track `{track_id}`"))?;
    let by_id: std::collections::BTreeMap<&str, &Node> = project
        .devices
        .iter()
        .map(|d| (d.id.as_str(), d))
        .collect();
    let mut out = clip.clone();
    for id in &track.device_ids {
        let Some(node) = by_id.get(id.as_str()).copied() else {
            continue;
        };
        out = match classify(node) {
            DeviceClass::Arpeggiator => arp_render(&out, &ArpParams::from_node(node)),
            DeviceClass::Chord => chord_render(&out, &ChordParams::from_node(node)),
            DeviceClass::Humanize => humanize_render(&out, &HumanizeParams::from_node(node)),
            _ => out,
        };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Project, Track};

    fn clip_with(chord: &[u8]) -> MidiClip {
        let mut clip = MidiClip::new(4.0);
        for (i, p) in chord.iter().enumerate() {
            clip.add_note(MidiNote::new((i + 1) as u32, *p, 100, 0.0, 1.0)).unwrap();
        }
        clip
    }

    #[test]
    fn arp_up_walks_low_to_high_and_repeats() {
        assert_eq!(
            arp_pattern(&[67, 60, 64], ArpMode::Up, 1, 0, 5),
            vec![60, 64, 67, 60, 64]
        );
    }

    #[test]
    fn arp_down_walks_high_to_low() {
        assert_eq!(
            arp_pattern(&[60, 64, 67], ArpMode::Down, 1, 0, 4),
            vec![67, 64, 60, 67]
        );
    }

    #[test]
    fn arp_updown_ping_pongs_without_repeating_tips() {
        assert_eq!(
            arp_pattern(&[60, 64, 67], ArpMode::UpDown, 1, 0, 7),
            vec![60, 64, 67, 64, 60, 64, 67]
        );
        // A single held note just repeats in every mode.
        assert_eq!(arp_pattern(&[60], ArpMode::UpDown, 1, 0, 3), vec![60, 60, 60]);
    }

    #[test]
    fn arp_random_is_deterministic_under_seed() {
        let a = arp_pattern(&[60, 64, 67], ArpMode::Random, 1, 1234, 8);
        let b = arp_pattern(&[60, 64, 67], ArpMode::Random, 1, 1234, 8);
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        assert!(a.iter().all(|p| [60, 64, 67].contains(p)));
        // A different seed draws a different take (near-certainly).
        let c = arp_pattern(&[60, 64, 67], ArpMode::Random, 1, 999, 8);
        assert_ne!(a, c);
    }

    #[test]
    fn arp_octaves_extend_the_pool_and_clamp_at_127() {
        assert_eq!(
            arp_pattern(&[60], ArpMode::Up, 2, 0, 3),
            vec![60, 72, 60]
        );
        // G9 (127) has no octave-up copy: the pool holds one pitch.
        assert_eq!(
            arp_pattern(&[127], ArpMode::Up, 4, 0, 2),
            vec![127, 127]
        );
        assert!(arp_pattern(&[], ArpMode::Up, 1, 0, 4).is_empty());
    }

    #[test]
    fn arp_render_spans_the_clip_at_rate_with_gate() {
        let clip = clip_with(&[60, 64, 67]);
        let params = ArpParams {
            mode: ArpMode::Up,
            rate_beats: 0.5,
            gate: 0.5,
            octaves: 1,
            seed: 0,
        };
        let out = arp_render(&clip, &params);
        // 4 beats at 0.5/step = 8 steps, each 0.25 long.
        assert_eq!(out.notes.len(), 8);
        let pitches: Vec<u8> = out.notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 64, 67, 60, 64, 67, 60, 64]);
        for (i, n) in out.notes.iter().enumerate() {
            assert_eq!((n.start_beats, n.len_beats), (i as f64 * 0.5, 0.25));
            assert_eq!(n.velocity, 100);
            assert!(n.validate().is_ok());
        }
    }

    #[test]
    fn chord_intervals_spell_standard_qualities() {
        assert_eq!(ChordType::Major.intervals(), &[0, 4, 7]);
        assert_eq!(ChordType::Minor.intervals(), &[0, 3, 7]);
        assert_eq!(ChordType::Dim7.intervals(), &[0, 3, 6, 9]);
        assert_eq!(ChordType::Sus4.intervals(), &[0, 5, 7]);
    }

    #[test]
    fn chord_spellings_cover_black_keys() {
        // Eb major (a black-key root): Eb G Bb.
        assert_eq!(
            chord_spell(63, ChordType::Major, 0, Voicing::Close),
            vec![63, 67, 70]
        );
        // F# minor: F# A C#.
        assert_eq!(
            chord_spell(66, ChordType::Minor, 0, Voicing::Close),
            vec![66, 69, 73]
        );
        // C dim7: C Eb Gb A.
        assert_eq!(
            chord_spell(60, ChordType::Dim7, 0, Voicing::Close),
            vec![60, 63, 66, 69]
        );
        // G sus4: G C D.
        assert_eq!(
            chord_spell(67, ChordType::Sus4, 0, Voicing::Close),
            vec![67, 72, 74]
        );
    }

    #[test]
    fn chord_inversion_puts_tones_in_the_bass() {
        // C major first inversion: E G C. Second: G C E.
        assert_eq!(
            chord_spell(60, ChordType::Major, 1, Voicing::Close),
            vec![64, 67, 72]
        );
        assert_eq!(
            chord_spell(60, ChordType::Major, 2, Voicing::Close),
            vec![67, 72, 76]
        );
        // Root position is untouched.
        assert_eq!(
            chord_spell(60, ChordType::Major, 0, Voicing::Close),
            vec![60, 64, 67]
        );
    }

    #[test]
    fn chord_render_expands_each_note_and_keeps_timing() {
        let clip = clip_with(&[63]);
        let params = ChordParams {
            chord: ChordType::Major,
            inversion: 0,
            voicing: Voicing::Close,
        };
        let out = chord_render(&clip, &params);
        let pitches: Vec<u8> = out.notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![63, 67, 70]);
        for n in &out.notes {
            assert_eq!((n.start_beats, n.len_beats, n.velocity), (0.0, 1.0, 100));
            assert!(n.validate().is_ok());
        }
        // Ids stay unique so the result always decodes.
        let mut ids: Vec<u32> = out.notes.iter().map(|n| n.note_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn chord_render_never_wraps_past_127() {
        let clip = clip_with(&[127]);
        let params = ChordParams {
            chord: ChordType::Major,
            inversion: 0,
            voicing: Voicing::Close,
        };
        let out = chord_render(&clip, &params);
        assert!(!out.notes.is_empty());
        assert!(out.notes.iter().all(|n| n.pitch <= 127));
    }

    #[test]
    fn humanize_is_deterministic_and_bounded() {
        let clip = clip_with(&[60, 61, 62, 63, 64]);
        let params = HumanizeParams {
            timing_beats: 0.02,
            velocity: 12.0,
            seed: 42,
        };
        let a = humanize_render(&clip, &params);
        let b = humanize_render(&clip, &params);
        assert_eq!(a, b, "same seed must render the same take");
        for n in &a.notes {
            assert!(n.timing_offset_beats.abs() <= 0.02 + 1e-12);
            assert!((1..=127).contains(&n.velocity));
            assert!(n.validate().is_ok());
        }
        // Zero amounts are a no-op (exact equality, not just bounds).
        let flat = HumanizeParams { timing_beats: 0.0, velocity: 0.0, seed: 7 };
        assert_eq!(humanize_render(&clip, &flat).notes, clip.notes);
    }

    #[test]
    fn humanize_clamps_extreme_notes_into_range() {
        let mut clip = MidiClip::new(4.0);
        clip.add_note(MidiNote::new(1, 60, 1, 0.0, 1.0)).unwrap();
        clip.add_note(MidiNote::new(2, 64, 127, 1.0, 1.0)).unwrap();
        let params = HumanizeParams { timing_beats: 0.25, velocity: 64.0, seed: 5 };
        let out = humanize_render(&clip, &params);
        for n in &out.notes {
            assert!(n.timing_offset_beats.abs() <= 0.25);
            assert!((1..=127).contains(&n.velocity));
            assert!(n.validate().is_ok());
        }
    }

    #[test]
    fn midi_chain_runs_fx_in_order_and_skips_audio() {
        let mut p = Project::new("p", "FX chain");
        p.tracks.push(Track {
            id: "trk".to_string(),
            name: "Keys".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![
                "chord".to_string(),
                "gain".to_string(),
                "hum".to_string(),
            ],
        });
        p.devices.push(super::super::class::instantiate(
            DeviceClass::Chord,
            "chord",
            "Chord",
        ));
        p.devices.push(super::super::class::instantiate(
            DeviceClass::Gain,
            "gain",
            "Trim",
        ));
        p.devices.push(super::super::class::instantiate(
            DeviceClass::Humanize,
            "hum",
            "Feel",
        ));
        let clip = clip_with(&[60]);
        let out = apply_midi_chain(&p, "trk", &clip).expect("chain");
        // Chord triad survived the (skipped) gain stage; humanize kept it valid.
        assert_eq!(out.notes.len(), 3);
        assert!(out.notes.iter().all(|n| n.validate().is_ok()));
        assert!(apply_midi_chain(&p, "nope", &clip).is_err());
    }

    #[test]
    fn fx_nodes_round_trip_class_and_json() {
        for class in [
            DeviceClass::Arpeggiator,
            DeviceClass::Chord,
            DeviceClass::Humanize,
        ] {
            let node = super::super::class::instantiate(class, "f", "F");
            assert_eq!(classify(&node), class, "{class:?}");
            let json = serde_json::to_string(&node).expect("serialize");
            let back: Node = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(classify(&back), class);
        }
    }
}
