//! Arrange a melody and a chord progression for an ensemble.
//!
//! Teaching note: an arrangement gives each instrument a *role*: one
//! carries the tune, one doubles it an octave up for sparkle, one plays a
//! slower counter-line on the chords' guide tones (thirds and sevenths,
//! the notes that say which chord it is), the bass walks roots and
//! fifths, a pad holds smooth voice-led chords, an arpeggio or pulse keeps
//! time. Each role is placed inside its instrument's comfortable range, so
//! a glockenspiel double never lands where the glockenspiel has no
//! recordings. Roles go on adaptive layers (`calm` holds the bed, `lively`
//! adds rhythm and the doubles) so the game can fade them by intensity.

use serde::{Deserialize, Serialize};

use super::theory::{self, Chord, Key, Pattern, VoicedChord, VoicingOpts};
use super::{library, Composition, Note, Part, PatchKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The tune, as written (moved by octaves into range).
    Melody,
    /// The tune `octave` octaves away (default +1).
    Double,
    /// The tune's strong-beat notes, short, an octave up (bells, harp).
    Sparkle,
    /// A slower line on the chords' guide tones, stepping between them.
    Counter,
    /// Roots and fifths.
    Bass,
    /// Held voice-led chords.
    Pad,
    /// Chord tones up and down in eighths.
    Arp,
    /// The chord repeated short in eighths.
    Pulse,
    /// Bass on the beat, chord off the beat.
    Broken,
    /// Tonic and fifth held under everything (a drone).
    Drone,
    /// A drum groove from the instrument's hits.
    Percussion,
}

/// One instrument's job in the arrangement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    /// Instrument id in the composition.
    pub instrument: String,
    pub role: Role,
    pub layer: String,
    /// Octave shift for `double` (default +1) or any role (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub octave: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<f32>,
    /// Beats per note for arp/pulse/broken (default 0.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdivision: Option<f32>,
    /// Percussion: the [low, high] MIDI keys to play (default: the
    /// instrument's lowest and highest hits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<(u8, u8)>,
    /// Percussion: `sparse` | `normal` | `busy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub section: String,
    /// Default: the composition's key.
    #[serde(default)]
    pub key: Option<String>,
    /// The tune, beats from the section start. May be empty (accompaniment only).
    #[serde(default)]
    pub melody: Vec<Note>,
    /// Repeat the melody to fill the section (default true).
    #[serde(default = "yes")]
    pub fill: bool,
    /// Chords as numerals or symbols, one per `every` beats, cycling to
    /// fill the section. Empty = re-harmonise the melody.
    #[serde(default)]
    pub chords: Vec<String>,
    /// Beats per chord (default: one bar).
    #[serde(default)]
    pub every: Option<f32>,
    #[serde(default)]
    pub palette: theory::Palette,
    pub ensemble: Vec<Member>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChordAt {
    pub start: f32,
    pub length: f32,
    pub symbol: String,
    pub numeral: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Arrangement {
    pub key: String,
    pub chords: Vec<ChordAt>,
    /// One part per member (section/layer/instrument + notes).
    pub parts: Vec<Part>,
    /// Plain-language notes on what had to be bent (range, missing tune).
    pub notes: Vec<String>,
}

/// Comfortable range of an instrument id in `c`.
pub fn range_of(c: &Composition, instrument: &str) -> Option<(u8, u8)> {
    let inst = c.instrument(instrument)?;
    match super::patch_kind(&inst.patch)? {
        PatchKind::Synth(p) => Some(p.range),
        PatchKind::Sampled(e) => Some(e.range),
        PatchKind::Local => Some((21, 108)),
    }
}

/// Octave shift that puts most of `notes` inside `range` (ties: smallest move).
fn fit_octave(notes: &[Note], range: (u8, u8), bias: i32) -> i32 {
    if notes.is_empty() {
        return bias;
    }
    (-4..=4)
        .min_by_key(|k| {
            let out: i32 = notes
                .iter()
                .map(|n| {
                    let p = n.pitch as i32 + 12 * (bias + k);
                    (range.0 as i32 - p).max(0) + (p - range.1 as i32).max(0)
                })
                .sum();
            (out, k.abs())
        })
        .map(|k| bias + k)
        .unwrap_or(bias)
}

fn shift(notes: &[Note], octaves: i32, vel: f32) -> Vec<Note> {
    notes
        .iter()
        .map(|n| Note {
            pitch: (n.pitch as i32 + 12 * octaves).clamp(0, 127) as u8,
            velocity: (n.velocity * vel).clamp(0.05, 1.0),
            ..n.clone()
        })
        .collect()
}

/// Move each note by octaves into `range` (accompaniment roles).
fn fold_into(notes: Vec<Note>, range: (u8, u8)) -> Vec<Note> {
    notes
        .into_iter()
        .map(|mut n| {
            let mut p = n.pitch as i32;
            while p < range.0 as i32 && p + 12 <= 127 {
                p += 12;
            }
            while p > range.1 as i32 && p - 12 >= range.0 as i32 {
                p -= 12;
            }
            n.pitch = p as u8;
            n
        })
        .collect()
}

fn clip_to(notes: Vec<Note>, beats: f32) -> Vec<Note> {
    notes
        .into_iter()
        .filter(|n| n.start < beats - 1e-4)
        .map(|n| Note { length: n.length.min(beats - n.start).max(0.05), ..n })
        .collect()
}

fn opts_in(range: (u8, u8), want_lo: u8, want_hi: u8, voices: usize) -> VoicingOpts {
    let lo = range.0.max(want_lo);
    let hi = range.1.min(want_hi).max(lo + 12);
    VoicingOpts { voices, low: lo, high: hi, bass_low: range.0.max(28), bass_high: range.1.min(55).max(range.0.max(28) + 11) }
}

fn melody_at(melody: &[Note], t: f32) -> Option<u8> {
    melody
        .iter()
        .filter(|n| n.start <= t + 1e-4 && n.start + n.length > t + 1e-4)
        .map(|n| n.pitch)
        .max()
}

/// Guide-tone counter-line: a third or seventh per chord, nearest the last
/// one, away from the tune, stepping through the key toward the next.
fn counter_line(chords: &[(Chord, f32, f32)], key: &Key, melody: &[Note], range: (u8, u8), vel: f32) -> Vec<Note> {
    let centre = ((range.0 as u16 + range.1 as u16) / 2) as i32;
    let mut out = Vec::new();
    let mut prev: i32 = centre;
    let guide = |c: &Chord| -> Vec<u8> {
        let mut g: Vec<u8> = c
            .intervals
            .iter()
            .filter(|&&i| matches!(i, 3 | 4 | 10 | 11 | 9))
            .map(|i| (c.root + i) % 12)
            .collect();
        if g.is_empty() {
            g = c.pcs();
        }
        g
    };
    let pick = |pcs: &[u8], near: i32, avoid: Option<u8>| -> i32 {
        let mut best = (i32::MAX, near);
        for p in range.0 as i32..=range.1 as i32 {
            if !pcs.contains(&((p % 12) as u8)) {
                continue;
            }
            let mut cost = (p - near).abs() * 2 + (p - centre).abs() / 3;
            if let Some(m) = avoid {
                let d = (p - m as i32).abs();
                if d <= 2 || d % 12 == 0 {
                    cost += 12;
                }
                if p > m as i32 {
                    // Stay under the tune.
                    cost += 6;
                }
            }
            if cost < best.0 {
                best = (cost, p);
            }
        }
        best.1
    };
    for (i, (c, start, len)) in chords.iter().enumerate() {
        let p = pick(&guide(c), prev, melody_at(melody, *start));
        let next = chords.get(i + 1).map(|(n, _, _)| pick(&guide(n), p, None));
        let half = len / 2.0;
        match next {
            Some(np) if *len >= 2.0 && (np - p).abs() >= 2 => {
                out.push(Note { pitch: p as u8, start: *start, length: half, velocity: vel });
                // A scale step toward the next guide tone.
                let step = key.step(p as u8, if np > p { 1 } else { -1 });
                out.push(Note { pitch: step, start: start + half, length: len - half, velocity: vel * 0.9 });
            }
            _ => out.push(Note { pitch: p as u8, start: *start, length: *len, velocity: vel }),
        }
        prev = p;
    }
    out
}

fn percussion(beats: f32, beats_per_bar: u32, keys: (u8, u8), density: &str, vel: f32) -> Vec<Note> {
    let (lo, hi) = keys;
    let mut out = Vec::new();
    let bpb = beats_per_bar.max(1) as f32;
    let mut t = 0.0f32;
    while t < beats - 1e-4 {
        let in_bar = t % bpb;
        let on_beat = (in_bar % 1.0).abs() < 1e-3;
        let downbeat = in_bar.abs() < 1e-3;
        let mid = bpb >= 4.0 && (in_bar - bpb / 2.0).abs() < 1e-3;
        let hit = match density {
            "sparse" => (downbeat || mid).then_some((lo, 1.0)),
            "busy" => Some(if downbeat || mid {
                (lo, 1.0)
            } else if on_beat {
                (hi, 0.8)
            } else {
                (hi, 0.55)
            }),
            _ => {
                if downbeat || mid {
                    Some((lo, 1.0))
                } else if !on_beat {
                    Some((hi, 0.6))
                } else {
                    None
                }
            }
        };
        if let Some((k, v)) = hit {
            out.push(Note { pitch: k, start: t, length: 0.5, velocity: (vel * v).clamp(0.05, 1.0) });
        }
        t += 0.5;
    }
    out
}

/// Lowest and highest hit keys of a sampled one-shot drum (downloads it
/// if needed).
fn drum_keys(c: &Composition, instrument: &str) -> Result<Option<(u8, u8)>, String> {
    let Some(inst) = c.instrument(instrument) else { return Ok(None) };
    let Some(PatchKind::Sampled(e)) = super::patch_kind(&inst.patch) else { return Ok(None) };
    if !e.style.one_shot || !matches!(e.family, "percussion" | "hand_drums") {
        return Ok(None);
    }
    let root = library::cache_root();
    library::ensure(&root, &library::lock(), e)?;
    let hits = library::hits(&library::regions(&root, e)?);
    Ok(match (hits.keys().next(), hits.keys().last()) {
        (Some(a), Some(b)) => Some((*a, *b)),
        _ => None,
    })
}

pub fn arrange(c: &Composition, req: &Request) -> Result<Arrangement, String> {
    let sec = c
        .section(&req.section)
        .ok_or_else(|| format!("unknown section {:?} (add it first)", req.section))?;
    let beats = c.section_beats(sec);
    let key_s = req.key.clone().unwrap_or_else(|| c.key.clone());
    if key_s.trim().is_empty() {
        return Err("no key: pass key (e.g. \"D major\") or set the composition's key".into());
    }
    let key = Key::parse(&key_s)?;
    let mut notes_out = Vec::new();
    // The tune, repeated to fill the section.
    let mut melody: Vec<Note> = req.melody.clone();
    melody.sort_by(|a, b| a.start.total_cmp(&b.start));
    if req.fill && !melody.is_empty() {
        let bar = c.beats_per_bar as f32;
        let span = melody.iter().map(|n| n.start + n.length).fold(0.0, f32::max);
        let span = (span / bar).ceil().max(1.0) * bar;
        let once = melody.clone();
        let mut k = 1.0;
        while span * k < beats - 1e-4 {
            melody.extend(once.iter().map(|n| Note { start: n.start + span * k, ..n.clone() }));
            k += 1.0;
        }
    }
    let melody = clip_to(melody, beats);
    // Chords: given (cycled) or found for the tune.
    let every = req.every.unwrap_or(c.beats_per_bar as f32).max(0.5);
    let chords: Vec<(Chord, f32, f32)> = if req.chords.is_empty() {
        if melody.is_empty() {
            return Err("give chords, or a melody to harmonise".into());
        }
        theory::reharmonise(&melody, &key, beats, every, c.beats_per_bar, req.palette)?
    } else {
        let parsed: Vec<Chord> = req
            .chords
            .iter()
            .map(|s| theory::parse_chord(s, &key))
            .collect::<Result<_, _>>()?;
        let n = (beats / every).ceil() as usize;
        (0..n)
            .map(|i| {
                let start = i as f32 * every;
                (parsed[i % parsed.len()].clone(), start, every.min(beats - start))
            })
            .collect()
    };
    let mut parts = Vec::new();
    for m in &req.ensemble {
        if c.instrument(&m.instrument).is_none() {
            return Err(format!("ensemble: unknown instrument {:?} (add it first)", m.instrument));
        }
        if c.layer(&m.layer).is_none() {
            return Err(format!("ensemble: unknown layer {:?} (add it first)", m.layer));
        }
        let range = range_of(c, &m.instrument).unwrap_or((36, 96));
        let sub = m.subdivision.unwrap_or(0.5);
        let oct = m.octave.unwrap_or(0);
        let voiced = |lo: u8, hi: u8, voices: usize| -> Result<Vec<VoicedChord>, String> {
            theory::voice_lead(&chords, &opts_in(range, lo, hi, voices))
        };
        let notes: Vec<Note> = match m.role {
            Role::Melody | Role::Double | Role::Sparkle => {
                if melody.is_empty() {
                    notes_out.push(format!("{}: no melody given, nothing to play", m.instrument));
                    continue;
                }
                let (src, bias, v): (Vec<Note>, i32, f32) = match m.role {
                    Role::Melody => (melody.clone(), oct, m.velocity.map_or(1.0, |v| v / 0.8)),
                    Role::Double => (melody.clone(), m.octave.unwrap_or(1), m.velocity.map_or(0.75, |v| v / 0.8)),
                    _ => (
                        melody
                            .iter()
                            .filter(|n| (n.start % 1.0).abs() < 1e-3)
                            .map(|n| Note { length: n.length.min(1.0), ..n.clone() })
                            .collect(),
                        m.octave.unwrap_or(1),
                        m.velocity.map_or(0.65, |v| v / 0.8),
                    ),
                };
                let k = if m.octave.is_some() && m.role != Role::Melody {
                    // An explicit octave is honoured unless it leaves the range entirely.
                    let k = fit_octave(&src, range, bias);
                    if (k - bias).abs() > 0 {
                        notes_out.push(format!(
                            "{}: octave {bias:+} would leave its range {}..={}; moved {:+}",
                            m.instrument, range.0, range.1, k - bias
                        ));
                    }
                    k
                } else {
                    // Doubles land at least an octave from the tune.
                    let mut k = fit_octave(&src, range, bias);
                    if m.role != Role::Melody && k == 0 {
                        k = if range.1 as i32 >= src.iter().map(|n| n.pitch as i32).max().unwrap_or(0) + 12 { 1 } else { -1 };
                    }
                    k
                };
                let out = shift(&src, k, v);
                let outside = out.iter().filter(|n| n.pitch < range.0 || n.pitch > range.1).count();
                if outside > 0 {
                    notes_out.push(format!(
                        "{}: {outside} note(s) fall outside its range {}..={} even after octave moves",
                        m.instrument, range.0, range.1
                    ));
                }
                out
            }
            Role::Counter => {
                let lo = range.0.max(52);
                let hi = range.1.min(79).max(lo + 12);
                let v = counter_line(&chords, &key, &melody, (lo, hi), m.velocity.unwrap_or(0.55));
                shift(&v, oct, 1.0)
            }
            Role::Bass => {
                let v = voiced(48, 72, 3)?;
                shift(&theory::realise(&v, Pattern::RootFifth, sub, m.velocity.unwrap_or(0.7), c.beats_per_bar), oct, 1.0)
            }
            Role::Pad => {
                let v = voiced(50, 77, if range.1 - range.0 >= 30 { 4 } else { 3 })?;
                shift(&theory::realise(&v, Pattern::Block, sub, m.velocity.unwrap_or(0.5), c.beats_per_bar), oct, 1.0)
            }
            Role::Arp => {
                let v = voiced(55, 84, 3)?;
                shift(&theory::realise(&v, Pattern::ArpUpDown, sub, m.velocity.unwrap_or(0.5), c.beats_per_bar), oct, 1.0)
            }
            Role::Pulse => {
                let v = voiced(52, 76, 3)?;
                shift(&theory::realise(&v, Pattern::Pulse, sub, m.velocity.unwrap_or(0.5), c.beats_per_bar), oct, 1.0)
            }
            Role::Broken => {
                let v = voiced(52, 76, 3)?;
                theory::realise(&v, Pattern::Broken, m.subdivision.unwrap_or(1.0), m.velocity.unwrap_or(0.55), c.beats_per_bar)
            }
            Role::Drone => {
                let root = (range.0..=range.1.min(60)).rev().find(|p| p % 12 == key.tonic).unwrap_or(range.0);
                let vel = m.velocity.unwrap_or(0.45);
                let mut v = vec![Note { pitch: root, start: 0.0, length: beats.min(64.0), velocity: vel }];
                if root + 7 <= range.1 {
                    v.push(Note { pitch: root + 7, start: 0.0, length: beats.min(64.0), velocity: vel * 0.85 });
                }
                v
            }
            Role::Percussion => {
                let keys = match m.keys {
                    Some(k) => k,
                    None => drum_keys(c, &m.instrument)?.unwrap_or((range.0, range.1)),
                };
                percussion(beats, c.beats_per_bar, keys, m.density.as_deref().unwrap_or("normal"), m.velocity.unwrap_or(0.7))
            }
        };
        let notes = match m.role {
            Role::Melody | Role::Double | Role::Sparkle | Role::Percussion => notes,
            _ => fold_into(notes, range),
        };
        parts.push(Part {
            section: req.section.clone(),
            layer: m.layer.clone(),
            instrument: m.instrument.clone(),
            notes: clip_to(notes, beats),
        });
    }
    Ok(Arrangement {
        key: key.name(),
        chords: chords
            .iter()
            .map(|(c, s, l)| ChordAt { start: *s, length: *l, symbol: c.symbol.clone(), numeral: c.numeral.clone() })
            .collect(),
        parts,
        notes: notes_out,
    })
}

/// One row of a game's ensemble (`design/music.md`, `## Ensemble` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnsembleRow {
    /// Instrument id the composition uses.
    pub id: String,
    /// Patch: a sampled instrument or synth patch id.
    pub patch: String,
    pub role: Role,
    pub layer: String,
}

/// Read the `Ensemble` table of a music design doc:
///
/// ```text
/// ## Ensemble
/// | Id | Patch | Role | Layer |
/// |---|---|---|---|
/// | lead | flute | melody | calm |
/// ```
pub fn ensemble_from_markdown(md: &str) -> Result<Vec<EnsembleRow>, String> {
    let mut in_section = false;
    let mut header: Option<Vec<String>> = None;
    let mut rows = Vec::new();
    for line in md.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            if in_section && !rows.is_empty() {
                break;
            }
            in_section = t.trim_start_matches('#').trim().to_ascii_lowercase().starts_with("ensemble");
            header = None;
            continue;
        }
        if !in_section || !t.starts_with('|') {
            continue;
        }
        let cells: Vec<String> = t.trim_matches('|').split('|').map(|c| c.trim().trim_matches('`').to_string()).collect();
        if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':')) {
            continue;
        }
        match &header {
            None => header = Some(cells.iter().map(|c| c.to_ascii_lowercase()).collect()),
            Some(h) => {
                let get = |name: &str| h.iter().position(|x| x == name).and_then(|i| cells.get(i)).cloned();
                let (Some(id), Some(patch), Some(role), Some(layer)) = (get("id"), get("patch"), get("role"), get("layer")) else {
                    return Err(format!("ensemble row {t:?}: want columns Id | Patch | Role | Layer"));
                };
                let role: Role = serde_json::from_value(serde_json::Value::String(role.to_ascii_lowercase()))
                    .map_err(|_| format!("ensemble row {t:?}: unknown role {role:?}"))?;
                rows.push(EnsembleRow { id, patch, role, layer });
            }
        }
    }
    if rows.is_empty() {
        return Err("no `## Ensemble` table (columns Id | Patch | Role | Layer) in the music doc".into());
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{Instrument, LayerDef, Role as SecRole, SectionDef};

    fn comp() -> Composition {
        let mut c = Composition::new("t", 100.0, 4);
        c.key = "D major".into();
        for (id, patch) in [("lead", "flute"), ("bells", "glockenspiel"), ("low", "cellos_pizz"), ("pad", "warm_pad"), ("alto", "clarinet")] {
            c.instruments.push(Instrument { id: id.into(), patch: patch.into(), gain_db: 0.0, reverb: 0.25, pan: None });
        }
        c.layers.push(LayerDef { id: "calm".into(), min_intensity: 1, gain_db: 0.0, ..Default::default() });
        c.layers.push(LayerDef { id: "lively".into(), min_intensity: 3, gain_db: 0.0, ..Default::default() });
        c.sections.push(SectionDef { id: "a".into(), role: SecRole::Loop, bars: 4, next: None });
        c
    }

    fn mel() -> Vec<Note> {
        [("D5", 0.0, 1.0), ("F#5", 1.0, 1.0), ("A5", 2.0, 2.0), ("G5", 4.0, 1.0), ("F#5", 5.0, 1.0), ("E5", 6.0, 2.0)]
            .iter()
            .map(|(p, s, l)| Note { pitch: crate::compose::parse_pitch(p).unwrap(), start: *s, length: *l, velocity: 0.8 })
            .collect()
    }

    fn member(i: &str, role: Role, layer: &str) -> Member {
        Member { instrument: i.into(), role, layer: layer.into(), octave: None, velocity: None, subdivision: None, keys: None, density: None }
    }

    #[test]
    fn arrangement_fills_the_section_in_range_on_layers() {
        let c = comp();
        let req = Request {
            section: "a".into(),
            key: None,
            melody: mel(),
            fill: true,
            chords: vec!["I".into(), "IV".into(), "V".into(), "I".into()],
            every: None,
            palette: Default::default(),
            ensemble: vec![
                member("lead", Role::Melody, "calm"),
                member("bells", Role::Sparkle, "lively"),
                member("low", Role::Bass, "calm"),
                member("pad", Role::Pad, "calm"),
                member("alto", Role::Counter, "lively"),
            ],
        };
        let a = arrange(&c, &req).unwrap();
        assert_eq!(a.chords.iter().map(|c| c.symbol.as_str()).collect::<Vec<_>>(), ["D", "G", "A", "D"]);
        assert_eq!(a.parts.len(), 5);
        let lead = &a.parts[0];
        // 8-beat tune filled to 16 beats.
        assert_eq!(lead.notes.len(), 12);
        for p in &a.parts {
            let r = range_of(&c, &p.instrument).unwrap();
            for n in &p.notes {
                assert!(n.start < 16.0);
                assert!(n.pitch >= r.0 && n.pitch <= r.1, "{} {} outside {r:?}", p.instrument, n.pitch);
            }
        }
        // Glockenspiel sparkle sits above the tune.
        let bells = &a.parts[1];
        assert!(bells.notes.iter().all(|n| n.pitch >= 79));
        // Bass plays chord roots on the downbeats.
        let low = &a.parts[2];
        let d = low.notes.iter().find(|n| n.start == 0.0).unwrap();
        assert_eq!(d.pitch % 12, 2);
        let g = low.notes.iter().find(|n| n.start == 4.0).unwrap();
        assert_eq!(g.pitch % 12, 7);
        assert!(a.notes.is_empty(), "{:?}", a.notes);
    }

    #[test]
    fn harmonises_when_no_chords_given_and_rejects_unknowns() {
        let c = comp();
        let mut req = Request {
            section: "a".into(),
            key: None,
            melody: mel(),
            fill: true,
            chords: vec![],
            every: Some(4.0),
            palette: Default::default(),
            ensemble: vec![member("pad", Role::Pad, "calm")],
        };
        let a = arrange(&c, &req).unwrap();
        assert_eq!(a.chords.len(), 4);
        assert_eq!(a.chords[0].symbol, "D");
        req.ensemble = vec![member("tuba", Role::Bass, "calm")];
        assert!(arrange(&c, &req).unwrap_err().contains("unknown instrument"));
    }

    #[test]
    fn reads_an_ensemble_table() {
        let md = "# Music\n\nintro text\n\n## Ensemble\n\n| Id | Patch | Role | Layer |\n|---|---|---|---|\n| lead | flute | melody | calm |\n| bells | `glockenspiel` | sparkle | lively |\n\n## Other\n| a | b |\n";
        let rows = ensemble_from_markdown(md).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].patch, "glockenspiel");
        assert_eq!(rows[1].role, Role::Sparkle);
        assert!(ensemble_from_markdown("# Music\nNothing yet.").is_err());
    }
}
