//! Writing help: keys and modes, chords and progressions with voice
//! leading, motif variations, and re-harmonising a melody.
//!
//! Teaching note: everything here works in *scale steps* where music
//! does. A motif inverted "diatonically" mirrors each interval in steps of
//! the key (a third up becomes a third down, major or minor as the key
//! gives), so the variation stays in the key instead of landing on wrong
//! notes the way a chromatic mirror would. Chords come from Roman numerals
//! read in the key (`ii` in D major is E minor) or plain symbols (`Em7`).
//! Voice leading picks, for each next chord, the voicing that moves the
//! upper voices least, keeps common tones, and avoids parallel fifths and
//! octaves: the textbook rules that make a pad sound smooth.

use serde::{Deserialize, Serialize};

use super::Note;

/// A scale or mode, as semitone offsets from the tonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Major,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Minor,
    Locrian,
    HarmonicMinor,
    MelodicMinor,
    MajorPentatonic,
    MinorPentatonic,
}

impl Mode {
    pub const ALL: [Mode; 11] = [
        Mode::Major,
        Mode::Dorian,
        Mode::Phrygian,
        Mode::Lydian,
        Mode::Mixolydian,
        Mode::Minor,
        Mode::Locrian,
        Mode::HarmonicMinor,
        Mode::MelodicMinor,
        Mode::MajorPentatonic,
        Mode::MinorPentatonic,
    ];

    pub fn intervals(self) -> &'static [u8] {
        match self {
            Mode::Major => &[0, 2, 4, 5, 7, 9, 11],
            Mode::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Mode::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Mode::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Mode::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Mode::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Mode::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Mode::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Mode::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Mode::MajorPentatonic => &[0, 2, 4, 7, 9],
            Mode::MinorPentatonic => &[0, 3, 5, 7, 10],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Major => "major",
            Mode::Dorian => "dorian",
            Mode::Phrygian => "phrygian",
            Mode::Lydian => "lydian",
            Mode::Mixolydian => "mixolydian",
            Mode::Minor => "minor",
            Mode::Locrian => "locrian",
            Mode::HarmonicMinor => "harmonic_minor",
            Mode::MelodicMinor => "melodic_minor",
            Mode::MajorPentatonic => "major_pentatonic",
            Mode::MinorPentatonic => "minor_pentatonic",
        }
    }

    /// Whether the tonic triad is minor (decides `i` vs `I` defaults).
    pub fn minor_tonic(self) -> bool {
        self.intervals().get(2).is_some_and(|&t| t == 3)
            || matches!(self, Mode::MinorPentatonic)
    }

    /// The seven-note scale chords are built on (pentatonics borrow their
    /// parent major/minor).
    pub fn heptatonic(self) -> Mode {
        match self {
            Mode::MajorPentatonic => Mode::Major,
            Mode::MinorPentatonic => Mode::Minor,
            m => m,
        }
    }
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

pub fn pc_name(pc: u8) -> &'static str {
    NOTE_NAMES[(pc % 12) as usize]
}

pub fn pitch_name(p: u8) -> String {
    format!("{}{}", pc_name(p % 12), p as i32 / 12 - 1)
}

/// Pitch class of a note letter with accidentals (`C`, `F#`, `Bb`, `Eb`).
pub fn parse_pc(s: &str) -> Result<u8, String> {
    let mut ch = s.chars();
    let base: i32 = match ch.next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => return Err(format!("{s:?}: want a note letter A-G")),
    };
    let mut acc = 0;
    for c in ch {
        match c {
            '#' | '♯' => acc += 1,
            'b' | '♭' => acc -= 1,
            _ => return Err(format!("{s:?}: unexpected {c:?} after the note letter")),
        }
    }
    Ok((base + acc).rem_euclid(12) as u8)
}

/// A tonic plus a mode, e.g. `D major`, `A dorian`, `F# minor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub tonic: u8,
    pub mode: Mode,
}

impl Key {
    pub fn parse(s: &str) -> Result<Key, String> {
        let t = s.trim();
        let mut it = t.split_whitespace();
        let tonic_s = it
            .next()
            .ok_or_else(|| "key: empty (want e.g. \"D major\" or \"A dorian\")".to_string())?;
        let rest: Vec<&str> = it.collect();
        // `Dm` / `F#m` shorthand.
        let (tonic_s, implied_minor) = match tonic_s.strip_suffix('m') {
            Some(x) if !x.is_empty() && rest.is_empty() => (x, true),
            _ => (tonic_s, false),
        };
        let tonic = parse_pc(tonic_s).map_err(|e| format!("key {t:?}: {e}"))?;
        let mode_s = rest.join("_").to_ascii_lowercase().replace('-', "_");
        let mode = match mode_s.as_str() {
            "" if implied_minor => Mode::Minor,
            "" | "major" | "ionian" | "maj" => Mode::Major,
            "minor" | "aeolian" | "min" | "natural_minor" => Mode::Minor,
            "dorian" => Mode::Dorian,
            "phrygian" => Mode::Phrygian,
            "lydian" => Mode::Lydian,
            "mixolydian" => Mode::Mixolydian,
            "locrian" => Mode::Locrian,
            "harmonic_minor" => Mode::HarmonicMinor,
            "melodic_minor" => Mode::MelodicMinor,
            "major_pentatonic" | "pentatonic" => Mode::MajorPentatonic,
            "minor_pentatonic" => Mode::MinorPentatonic,
            other => {
                return Err(format!(
                    "key {t:?}: unknown mode {other:?} (want one of {})",
                    Mode::ALL.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ")
                ))
            }
        };
        Ok(Key { tonic, mode })
    }

    pub fn name(&self) -> String {
        format!("{} {}", pc_name(self.tonic), self.mode.name().replace('_', " "))
    }

    /// Pitch classes of the scale, tonic first.
    pub fn pcs(&self) -> Vec<u8> {
        self.mode
            .intervals()
            .iter()
            .map(|i| (self.tonic + i) % 12)
            .collect()
    }

    pub fn contains(&self, pitch: u8) -> bool {
        self.pcs().contains(&(pitch % 12))
    }

    /// Absolute scale index of a pitch: `degree + octave * len`, plus the
    /// chromatic remainder (0 when the pitch is in the scale, +1 when it
    /// sits a semitone above the scale note below).
    fn index_of(&self, pitch: u8) -> (i32, i32) {
        let iv = self.mode.intervals();
        let n = iv.len() as i32;
        let rel = pitch as i32 - self.tonic as i32;
        let oct = rel.div_euclid(12);
        let within = rel.rem_euclid(12);
        let mut deg = 0;
        for (d, &i) in iv.iter().enumerate() {
            if i as i32 <= within {
                deg = d as i32;
            }
        }
        (oct * n + deg, within - iv[deg as usize] as i32)
    }

    fn pitch_of(&self, index: i32, chroma: i32) -> i32 {
        let iv = self.mode.intervals();
        let n = iv.len() as i32;
        let oct = index.div_euclid(n);
        let deg = index.rem_euclid(n);
        self.tonic as i32 + oct * 12 + iv[deg as usize] as i32 + chroma
    }

    /// Move a pitch by `steps` scale steps (chromatic notes keep their
    /// offset from the scale note below).
    pub fn step(&self, pitch: u8, steps: i32) -> u8 {
        let (i, c) = self.index_of(pitch);
        self.pitch_of(i + steps, c).clamp(0, 127) as u8
    }

    /// Nearest scale pitch (ties go down).
    pub fn snap(&self, pitch: u8) -> u8 {
        for d in 0..12i32 {
            for p in [pitch as i32 - d, pitch as i32 + d] {
                if (0..=127).contains(&p) && self.contains(p as u8) {
                    return p as u8;
                }
            }
        }
        pitch
    }

    /// Scale degree (1-based) of a pitch class in the key, if diatonic.
    pub fn degree(&self, pc: u8) -> Option<usize> {
        self.pcs().iter().position(|&p| p == pc % 12).map(|d| d + 1)
    }
}

// ---------------------------------------------------------------- chords

/// A chord: root pitch class, intervals above it, optional bass (slash).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chord {
    pub root: u8,
    /// Semitones above the root, root (0) first.
    pub intervals: Vec<u8>,
    /// Bass pitch class when it isn't the root (`C/E`, `IV/3`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bass: Option<u8>,
    /// Plain symbol, e.g. `Em7`.
    pub symbol: String,
    /// Roman numeral in the key, e.g. `ii7` (when read or derived in a key).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeral: Option<String>,
}

impl Chord {
    pub fn pcs(&self) -> Vec<u8> {
        self.intervals.iter().map(|i| (self.root + i) % 12).collect()
    }
    pub fn bass_pc(&self) -> u8 {
        self.bass.unwrap_or(self.root)
    }
    fn has_third(&self) -> Option<u8> {
        self.intervals.iter().copied().find(|&i| i == 3 || i == 4)
    }
    fn quality_suffix(&self) -> &'static str {
        quality_name(&self.intervals)
    }
}

fn quality_name(iv: &[u8]) -> &'static str {
    match iv {
        [0, 4, 7] => "",
        [0, 3, 7] => "m",
        [0, 3, 6] => "dim",
        [0, 4, 8] => "aug",
        [0, 2, 7] => "sus2",
        [0, 5, 7] => "sus4",
        [0, 4, 7, 10] => "7",
        [0, 4, 7, 11] => "maj7",
        [0, 3, 7, 10] => "m7",
        [0, 3, 6, 10] => "m7b5",
        [0, 3, 6, 9] => "dim7",
        [0, 3, 7, 11] => "mMaj7",
        [0, 4, 7, 9] => "6",
        [0, 3, 7, 9] => "m6",
        [0, 4, 7, 14] | [0, 2, 4, 7] => "add9",
        [0, 3, 7, 14] | [0, 2, 3, 7] => "madd9",
        [0, 5, 7, 10] => "7sus4",
        _ => "?",
    }
}

/// Suffix → intervals, for both symbols and numerals.
fn suffix_intervals(base: &[u8], suffix: &str) -> Result<Vec<u8>, String> {
    let minor = base == [0, 3, 7];
    let dim = base == [0, 3, 6];
    Ok(match suffix {
        "" => base.to_vec(),
        "7" if dim => vec![0, 3, 6, 10],
        "7" if minor => vec![0, 3, 7, 10],
        "7" => {
            if base == [0, 4, 8] {
                vec![0, 4, 8, 10]
            } else {
                vec![0, 4, 7, 10]
            }
        }
        "maj7" | "M7" | "Δ7" | "Δ" => {
            if minor {
                vec![0, 3, 7, 11]
            } else {
                vec![0, 4, 7, 11]
            }
        }
        "m7" | "min7" => vec![0, 3, 7, 10],
        "m" | "min" => vec![0, 3, 7],
        "dim" | "°" | "o" => vec![0, 3, 6],
        "dim7" | "°7" | "o7" => vec![0, 3, 6, 9],
        "m7b5" | "ø" | "ø7" => vec![0, 3, 6, 10],
        "aug" | "+" => vec![0, 4, 8],
        "sus2" => vec![0, 2, 7],
        "sus4" | "sus" => vec![0, 5, 7],
        "7sus4" => vec![0, 5, 7, 10],
        "6" => {
            if minor {
                vec![0, 3, 7, 9]
            } else {
                vec![0, 4, 7, 9]
            }
        }
        "m6" => vec![0, 3, 7, 9],
        "add9" | "add2" => {
            let mut v = base.to_vec();
            v.push(14);
            v
        }
        "madd9" => vec![0, 3, 7, 14],
        "9" => {
            let mut v = suffix_intervals(base, "7")?;
            v.push(14);
            v
        }
        "maj9" => vec![0, 4, 7, 11, 14],
        "m9" => vec![0, 3, 7, 10, 14],
        _ => return Err(format!("unknown chord suffix {suffix:?}")),
    })
}

/// Read a chord symbol (`D`, `Em7`, `F#m7b5`, `Bbsus4`, `C/E`).
pub fn parse_symbol(s: &str) -> Result<Chord, String> {
    let (main, slash) = match s.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let mut letter_end = 1;
    let b = main.as_bytes();
    if b.is_empty() {
        return Err("empty chord".into());
    }
    while letter_end < b.len() && (b[letter_end] == b'#' || b[letter_end] == b'b') {
        // `Bb` is B-flat, but `Cb5` is not a thing we read; a `b` right
        // after the letter is always a flat.
        letter_end += 1;
    }
    let root = parse_pc(&main[..letter_end]).map_err(|e| format!("chord {s:?}: {e}"))?;
    let suffix = &main[letter_end..];
    let (base, rest): (&[u8], &str) = if let Some(r) = suffix.strip_prefix("maj") {
        (&[0, 4, 7], if r.is_empty() { "maj7" } else { &suffix[..] })
    } else if suffix.starts_with("mMaj7") {
        (&[0, 3, 7], "maj7")
    } else if let Some(r) = suffix.strip_prefix("m7b5") {
        if !r.is_empty() {
            return Err(format!("chord {s:?}: unexpected {r:?}"));
        }
        (&[0, 3, 6], "7")
    } else if suffix.starts_with("min") || (suffix.starts_with('m') && !suffix.starts_with("maj")) {
        let r = suffix.trim_start_matches("min").trim_start_matches('m');
        (&[0, 3, 7], r)
    } else if let Some(r) = suffix.strip_prefix("dim") {
        (&[0, 3, 6], if r == "7" { "dim7" } else { r })
    } else if let Some(r) = suffix.strip_prefix("aug") {
        (&[0, 4, 8], r)
    } else {
        (&[0, 4, 7], suffix)
    };
    let intervals = suffix_intervals(base, rest).map_err(|e| format!("chord {s:?}: {e}"))?;
    let bass = slash
        .map(|b| parse_pc(b).map_err(|e| format!("chord {s:?} bass: {e}")))
        .transpose()?;
    Ok(make_chord(root, intervals, bass, None))
}

fn make_chord(root: u8, intervals: Vec<u8>, bass: Option<u8>, numeral: Option<String>) -> Chord {
    let mut symbol = format!("{}{}", pc_name(root), quality_name(&intervals));
    if let Some(b) = bass.filter(|&b| b != root) {
        symbol.push('/');
        symbol.push_str(pc_name(b));
    }
    Chord {
        root,
        intervals,
        bass: bass.filter(|&b| b != root),
        symbol,
        numeral,
    }
}

const ROMANS: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];

/// Diatonic triad (or seventh) on scale degree `deg` (0-based) of `key`.
fn diatonic(key: &Key, deg: usize, seventh: bool) -> Vec<u8> {
    let m = key.mode.heptatonic();
    let k = Key { tonic: key.tonic, mode: m };
    let iv = m.intervals();
    let root = iv[deg] as i32;
    let mut out = vec![0u8];
    let steps: &[usize] = if seventh { &[2, 4, 6] } else { &[2, 4] };
    for &s in steps {
        let idx = deg + s;
        let p = iv[idx % 7] as i32 + 12 * (idx / 7) as i32;
        out.push((p - root) as u8);
    }
    let _ = k;
    out
}

/// Read a Roman numeral in `key`: `I`, `ii`, `iii7`, `IVmaj7`, `V7`,
/// `vii°`, `bVII`, `bVI`, `#iv°`, `V/V`, `V7/vi`, `IV/3` (inversion: bass on
/// the chord's third). Case sets quality when it differs from the key's
/// own triad (`iv` in major borrows the minor four).
pub fn parse_numeral(s: &str, key: &Key) -> Result<Chord, String> {
    // Secondary chords: `V/V`, `vii°/ii` (but `IV/3` is an inversion).
    if let Some((head, target)) = s.split_once('/') {
        if target.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let mut c = parse_numeral(head, key)?;
            let which: usize = target.parse().map_err(|_| format!("numeral {s:?}: bad inversion"))?;
            let tone = match which {
                1 => 0,
                3 => c.has_third().ok_or_else(|| format!("numeral {s:?}: no third"))?,
                5 => *c.intervals.get(2).ok_or_else(|| format!("numeral {s:?}: no fifth"))?,
                7 => *c.intervals.get(3).ok_or_else(|| format!("numeral {s:?}: no seventh"))?,
                _ => return Err(format!("numeral {s:?}: inversion wants /3, /5 or /7")),
            };
            let bass = (c.root + tone) % 12;
            let n = c.numeral.clone();
            c = make_chord(c.root, c.intervals, Some(bass), n.map(|n| format!("{n}/{which}")));
            return Ok(c);
        }
        let t = parse_numeral(target, key)?;
        let local = Key { tonic: t.root, mode: Mode::Major };
        let mut c = parse_numeral(head, &local)?;
        c.numeral = Some(s.to_string());
        return Ok(c);
    }
    let mut rest = s;
    let mut shift: i32 = 0;
    while let Some(c) = rest.chars().next() {
        match c {
            'b' | '♭' => shift -= 1,
            '#' | '♯' => shift += 1,
            _ => break,
        }
        rest = &rest[c.len_utf8()..];
    }
    let upper = rest.to_ascii_uppercase();
    let (deg, len) = ROMANS
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, r)| upper.starts_with(*r))
        .max_by_key(|(_, r)| r.len())
        .map(|(i, r)| (i, r.len()))
        .ok_or_else(|| format!("numeral {s:?}: want I..VII (or a chord symbol like Em7)"))?;
    let is_upper = rest[..len].chars().all(|c| c.is_ascii_uppercase());
    let suffix = &rest[len..];
    let m = key.mode.heptatonic();
    // `bVI`, `#iv`: accidentals are relative to the major scale (so `bVI`
    // is the same chord in A minor and A major); a bare numeral follows
    // the key's own scale (`VI` in A minor is F).
    let scale = if shift != 0 { Mode::Major.intervals() } else { m.intervals() };
    let root = (key.tonic as i32 + scale[deg] as i32 + shift).rem_euclid(12) as u8;
    let natural = diatonic(key, deg, false);
    let nat_minor = natural == [0, 3, 7] || natural == [0, 3, 6];
    let base: Vec<u8> = if suffix.starts_with('°') || suffix.starts_with('o') || suffix.starts_with("dim") {
        vec![0, 3, 6]
    } else if suffix.starts_with('ø') {
        vec![0, 3, 6]
    } else if suffix.starts_with('+') || suffix.starts_with("aug") {
        vec![0, 4, 8]
    } else if shift == 0 && is_upper != nat_minor {
        // Case agrees with the key: the diatonic triad.
        natural.clone()
    } else if is_upper {
        vec![0, 4, 7]
    } else {
        vec![0, 3, 7]
    };
    let suf = suffix
        .trim_start_matches("dim")
        .trim_start_matches("aug")
        .trim_start_matches(['°', 'o', '+']);
    let suf = if suffix.starts_with('ø') { "7" } else { suf };
    let intervals = if suf == "7" && shift == 0 && base == natural {
        // Diatonic seventh: V7 dominant in major, i7 minor, IVmaj7 ...
        diatonic(key, deg, true)
    } else {
        suffix_intervals(&base, suf).map_err(|e| format!("numeral {s:?}: {e}"))?
    };
    Ok(make_chord(root, intervals, None, Some(s.to_string())))
}

/// A symbol (`Em7`) or a numeral read in `key` (`ii7`).
pub fn parse_chord(s: &str, key: &Key) -> Result<Chord, String> {
    let s = s.trim();
    let first = s.chars().next().ok_or("empty chord")?;
    if ('A'..='G').contains(&first) {
        let mut c = parse_symbol(s)?;
        c.numeral = Some(numeral_for(&c, key));
        Ok(c)
    } else {
        let mut c = parse_numeral(s, key)?;
        if c.numeral.is_none() {
            c.numeral = Some(numeral_for(&c, key));
        }
        Ok(c)
    }
}

/// Roman numeral naming a chord in a key (borrowed roots get `b`/`#`).
pub fn numeral_for(c: &Chord, key: &Key) -> String {
    let m = key.mode.heptatonic();
    let rel = (c.root as i32 - key.tonic as i32).rem_euclid(12) as u8;
    let iv = m.intervals();
    let (deg, acc) = match iv.iter().position(|&i| i == rel) {
        Some(d) => (d, ""),
        None => match iv.iter().position(|&i| i == (rel + 1) % 12) {
            Some(d) => (d, "b"),
            None => (iv.iter().position(|&i| (i + 11) % 12 == rel).unwrap_or(0), "#"),
        },
    };
    let minor = c.has_third() == Some(3);
    let r = ROMANS[deg];
    let r = if minor { r.to_ascii_lowercase() } else { r.to_string() };
    let suf = match c.quality_suffix() {
        "" | "m" => "",
        "dim" => "°",
        "m7" => "7",
        "m7b5" => "ø7",
        s => s,
    };
    format!("{acc}{r}{suf}")
}

/// Named progressions (familiar shapes), with the mode they're written for.
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("pop", "I V vi IV", "the four chords of countless songs"),
    ("doo_wop", "I vi IV V", "1950s, warm and nostalgic"),
    ("journey", "I IV vi V", "hopeful travelling"),
    ("royal_road", "IVmaj7 V7 iii7 vi", "the J-pop / JRPG 'royal road', bittersweet and familiar"),
    ("canon", "I V vi iii IV I IV V", "Pachelbel, stately"),
    ("plagal_home", "I IV I V", "folk, simple and sunny"),
    ("mixolydian_sail", "I bVII IV I", "seafaring, open-air (major key, borrowed bVII)"),
    ("lydian_wonder", "I II I II", "floaty wonder (the raised fourth)"),
    ("andalusian", "i bVII bVI V", "minor descent, dramatic"),
    ("minor_epic", "i bVI bIII bVII", "minor-key adventure"),
    ("dorian_vamp", "i IV i IV", "mysterious but bright"),
    ("ii_v_i", "ii7 V7 Imaj7 Imaj7", "jazz cadence"),
    ("circle", "vi ii V I", "falling fifths home"),
];

pub fn preset(name: &str) -> Option<&'static str> {
    PRESETS.iter().find(|p| p.0 == name).map(|p| p.1)
}

// --------------------------------------------------------- voice leading

/// One chord voiced and placed in time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoicedChord {
    pub start: f32,
    pub length: f32,
    pub chord: Chord,
    pub bass: u8,
    /// Upper voices, low to high.
    pub voices: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VoicingOpts {
    /// Upper voices (2..=5).
    pub voices: usize,
    /// Upper voices stay within [low, high] (MIDI numbers or names like `G3`).
    #[serde(deserialize_with = "super::de_pitch")]
    pub low: u8,
    #[serde(deserialize_with = "super::de_pitch")]
    pub high: u8,
    /// Bass stays within [bass_low, bass_high].
    #[serde(deserialize_with = "super::de_pitch")]
    pub bass_low: u8,
    #[serde(deserialize_with = "super::de_pitch")]
    pub bass_high: u8,
}

impl Default for VoicingOpts {
    fn default() -> Self {
        // Upper voices around middle C, bass in the cello/bass register.
        VoicingOpts { voices: 3, low: 52, high: 76, bass_low: 36, bass_high: 55 }
    }
}

fn candidates(chord: &Chord, opts: &VoicingOpts) -> Vec<Vec<u8>> {
    let pcs = chord.pcs();
    // Every pitch in range that is a chord tone.
    let pool: Vec<u8> = (opts.low..=opts.high).filter(|p| pcs.contains(&(p % 12))).collect();
    let mut out = Vec::new();
    let n = opts.voices.clamp(1, 6);
    let mut cur = Vec::with_capacity(n);
    fn rec(pool: &[u8], from: usize, n: usize, cur: &mut Vec<u8>, out: &mut Vec<Vec<u8>>) {
        if cur.len() == n {
            out.push(cur.clone());
            return;
        }
        for i in from..pool.len() {
            // Adjacent upper voices within an octave, no unisons.
            if let Some(&last) = cur.last() {
                if pool[i] - last > 12 {
                    break;
                }
            }
            cur.push(pool[i]);
            rec(pool, i + 1, n, cur, out);
            cur.pop();
        }
    }
    rec(&pool, 0, n, &mut cur, &mut out);
    // Must hold the root and the third (when it has one), and every tone
    // when there are enough voices.
    let third = chord.has_third().map(|t| (chord.root + t) % 12);
    out.retain(|v| {
        let have: Vec<u8> = v.iter().map(|p| p % 12).collect();
        let distinct = {
            let mut d = have.clone();
            d.sort();
            d.dedup();
            d.len()
        };
        have.contains(&chord.root)
            && third.is_none_or(|t| have.contains(&t))
            && distinct >= pcs.len().min(n)
    });
    out
}

fn parallel_perfects(a: &[u8], b: &[u8], bass_a: u8, bass_b: u8) -> usize {
    let mut va = vec![bass_a];
    va.extend_from_slice(a);
    let mut vb = vec![bass_b];
    vb.extend_from_slice(b);
    let mut n = 0;
    for i in 0..va.len() {
        for j in i + 1..va.len().min(vb.len()) {
            let ia = (va[j] as i32 - va[i] as i32).rem_euclid(12);
            let ib = (vb[j] as i32 - vb[i] as i32).rem_euclid(12);
            let moved = va[i] != vb[i] && va[j] != vb[j];
            if moved && ia == ib && (ia == 7 || ia == 0) {
                n += 1;
            }
        }
    }
    n
}

fn voicing_cost(prev: Option<&[u8]>, v: &[u8], chord: &Chord, opts: &VoicingOpts, bass_prev: u8, bass: u8) -> f32 {
    let centre = (opts.low as f32 + opts.high as f32) / 2.0;
    let mean = v.iter().map(|&p| p as f32).sum::<f32>() / v.len() as f32;
    let mut cost = 0.15 * (mean - centre).abs();
    // Double the root, not the third.
    let third = chord.has_third().map(|t| (chord.root + t) % 12);
    if let Some(t) = third {
        if v.iter().filter(|p| *p % 12 == t).count() > 1 {
            cost += 3.0;
        }
    }
    // Mud: upper voices too close to a low bass.
    if let Some(&lo) = v.first() {
        if lo < bass + 7 {
            cost += 4.0;
        }
        if lo < 48 && v.get(1).is_some_and(|&n| n - lo < 5) {
            cost += 3.0;
        }
    }
    if let Some(p) = prev {
        if p.len() == v.len() {
            let motion: i32 = p.iter().zip(v).map(|(a, b)| (*a as i32 - *b as i32).abs()).sum();
            cost += motion as f32;
            // Leaps bigger than a fourth in an inner voice.
            cost += p
                .iter()
                .zip(v)
                .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 5)
                .count() as f32
                * 2.0;
            cost += 6.0 * parallel_perfects(p, v, bass_prev, bass) as f32;
        }
    }
    cost
}

fn bass_note(chord: &Chord, prev: Option<u8>, opts: &VoicingOpts) -> u8 {
    let pc = chord.bass_pc();
    let opts_in: Vec<u8> = (opts.bass_low..=opts.bass_high).filter(|p| p % 12 == pc).collect();
    let target = prev.unwrap_or((opts.bass_low + opts.bass_high) / 2 - 3);
    opts_in
        .iter()
        .copied()
        .min_by_key(|&p| (p as i32 - target as i32).abs())
        .unwrap_or(opts.bass_low + (pc + 12 - opts.bass_low % 12) % 12)
}

/// Voice a list of chords, each `beats` long, smoothest motion first.
pub fn voice_lead(chords: &[(Chord, f32, f32)], opts: &VoicingOpts) -> Result<Vec<VoicedChord>, String> {
    let mut out: Vec<VoicedChord> = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    let mut prev_bass: Option<u8> = None;
    for (chord, start, length) in chords {
        let bass = bass_note(chord, prev_bass, opts);
        let cands = candidates(chord, opts);
        if cands.is_empty() {
            return Err(format!(
                "chord {}: no voicing of {} voices fits MIDI {}..={} (widen the range or use fewer voices)",
                chord.symbol, opts.voices, opts.low, opts.high
            ));
        }
        let best = cands
            .into_iter()
            .min_by(|a, b| {
                let ca = voicing_cost(prev.as_deref(), a, chord, opts, prev_bass.unwrap_or(bass), bass);
                let cb = voicing_cost(prev.as_deref(), b, chord, opts, prev_bass.unwrap_or(bass), bass);
                ca.total_cmp(&cb)
            })
            .expect("non-empty");
        out.push(VoicedChord { start: *start, length: *length, chord: chord.clone(), bass, voices: best.clone() });
        prev = Some(best);
        prev_bass = Some(bass);
    }
    Ok(out)
}

/// Parallel-fifth/octave count across a voiced progression (0 = clean).
pub fn parallels(v: &[VoicedChord]) -> usize {
    v.windows(2)
        .map(|w| parallel_perfects(&w[0].voices, &w[1].voices, w[0].bass, w[1].bass))
        .sum()
}

/// How a voiced chord turns into notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// Held chord for its whole length.
    Block,
    /// Chord tones one at a time, upward, at `subdivision`.
    ArpUp,
    /// Up then down.
    ArpUpDown,
    /// Bass-note on the beat, chord on the off-beats (oom-pah / waltz).
    Broken,
    /// The chord repeated at `subdivision`, short (strummed pulse).
    Pulse,
    /// Bass line only: root on the first beat, fifth halfway.
    RootFifth,
    /// Bass line only: the root held.
    Root,
}

fn vel(v: f32) -> f32 {
    v.clamp(0.05, 1.0)
}

/// Notes for one voiced progression in a pattern. `subdivision` in beats
/// (0.5 = eighths); `beats_per_bar` places strong beats.
pub fn realise(v: &[VoicedChord], pattern: Pattern, subdivision: f32, velocity: f32, beats_per_bar: u32) -> Vec<Note> {
    let sub = if subdivision > 0.0 { subdivision } else { 0.5 };
    let mut out = Vec::new();
    for c in v {
        let end = c.start + c.length;
        match pattern {
            Pattern::Block => {
                for &p in &c.voices {
                    out.push(Note { pitch: p, start: c.start, length: c.length, velocity: vel(velocity) });
                }
            }
            Pattern::ArpUp | Pattern::ArpUpDown => {
                let mut seq: Vec<u8> = c.voices.clone();
                if pattern == Pattern::ArpUpDown && seq.len() > 2 {
                    let back: Vec<u8> = seq[1..seq.len() - 1].iter().rev().copied().collect();
                    seq.extend(back);
                }
                let mut t = c.start;
                let mut i = 0;
                while t + 1e-4 < end {
                    let len = sub.min(end - t);
                    let strong = ((t - c.start) % beats_per_bar as f32).abs() < 1e-3;
                    out.push(Note {
                        pitch: seq[i % seq.len()],
                        start: t,
                        length: len,
                        velocity: vel(velocity * if strong { 1.0 } else { 0.85 }),
                    });
                    t += sub;
                    i += 1;
                }
            }
            Pattern::Broken => {
                let mut t = c.start;
                while t + 1e-4 < end {
                    let on_beat = ((t - c.start) % 1.0).abs() < 1e-3 && ((t - c.start) as u32) % 2 == 0;
                    let len = sub.min(end - t);
                    if on_beat {
                        out.push(Note { pitch: c.bass, start: t, length: len, velocity: vel(velocity) });
                    } else {
                        for &p in &c.voices {
                            out.push(Note { pitch: p, start: t, length: len * 0.8, velocity: vel(velocity * 0.75) });
                        }
                    }
                    t += sub;
                }
            }
            Pattern::Pulse => {
                let mut t = c.start;
                while t + 1e-4 < end {
                    let accent = ((t - c.start) % 1.0).abs() < 1e-3;
                    for &p in &c.voices {
                        out.push(Note {
                            pitch: p,
                            start: t,
                            length: (sub * 0.6).min(end - t),
                            velocity: vel(velocity * if accent { 1.0 } else { 0.78 }),
                        });
                    }
                    t += sub;
                }
            }
            Pattern::RootFifth => {
                let half = (c.length / 2.0).max(0.5);
                out.push(Note { pitch: c.bass, start: c.start, length: half.min(c.length), velocity: vel(velocity) });
                if c.length >= 2.0 {
                    let fifth = c.chord.intervals.get(2).copied().unwrap_or(7);
                    let mut f = c.chord.root as i32 + fifth as i32;
                    // Fifth nearest the bass (above or below).
                    while f - c.bass as i32 > 7 {
                        f -= 12;
                    }
                    while (c.bass as i32) - f > 5 {
                        f += 12;
                    }
                    out.push(Note {
                        pitch: f.clamp(0, 127) as u8,
                        start: c.start + half,
                        length: c.length - half,
                        velocity: vel(velocity * 0.85),
                    });
                }
            }
            Pattern::Root => {
                out.push(Note { pitch: c.bass, start: c.start, length: c.length, velocity: vel(velocity) });
            }
        }
    }
    out
}

// ---------------------------------------------------------------- motifs

/// One motif transformation. Steps are scale steps in the key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MotifOp {
    /// Up/down by scale steps (stays in key).
    Transpose { steps: i32 },
    /// Up/down by semitones (leaves the key if you ask it to).
    Chromatic { semitones: i32 },
    /// Mirror each interval in scale steps around `axis` (default: the
    /// first note).
    Invert {
        #[serde(default, deserialize_with = "de_opt_pitch")]
        axis: Option<u8>,
    },
    /// Play it backwards (rhythm reversed too).
    Retrograde,
    /// Stretch time: 2 = augmentation, 0.5 = diminution.
    Augment { factor: f32 },
    /// Repeat it `times` more times, each `steps` scale steps on, back to back.
    Sequence { times: u32, steps: i32 },
    /// Keep notes whose start falls in [from, to) beats (a fragment).
    Fragment { from: f32, to: f32 },
    /// Shift in time by `beats`.
    Displace { beats: f32 },
    /// Octaves up/down.
    Octave { octaves: i32 },
    /// Scale every velocity.
    Dynamics { scale: f32 },
}

fn de_opt_pitch<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u8>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum P {
        N(u8),
        S(String),
    }
    match Option::<P>::deserialize(d)? {
        None => Ok(None),
        Some(P::N(n)) => Ok(Some(n)),
        Some(P::S(s)) => super::parse_pitch(&s).map(Some).map_err(serde::de::Error::custom),
    }
}

fn span(notes: &[Note]) -> f32 {
    notes.iter().map(|n| n.start + n.length).fold(0.0, f32::max)
}

/// Apply transformations in order. Times are relative to the motif start.
pub fn transform(notes: &[Note], key: &Key, ops: &[MotifOp]) -> Result<Vec<Note>, String> {
    let mut cur: Vec<Note> = notes.to_vec();
    cur.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    for op in ops {
        cur = match op {
            MotifOp::Transpose { steps } => cur
                .iter()
                .map(|n| Note { pitch: key.step(n.pitch, *steps), ..n.clone() })
                .collect(),
            MotifOp::Chromatic { semitones } => cur
                .iter()
                .map(|n| Note { pitch: (n.pitch as i32 + semitones).clamp(0, 127) as u8, ..n.clone() })
                .collect(),
            MotifOp::Invert { axis } => {
                let Some(first) = cur.first() else { continue };
                let axis = axis.unwrap_or(first.pitch);
                let (ai, _) = key.index_of(key.snap(axis));
                cur.iter()
                    .map(|n| {
                        let (i, c) = key.index_of(n.pitch);
                        let p = key.pitch_of(2 * ai - i, -c);
                        Note { pitch: p.clamp(0, 127) as u8, ..n.clone() }
                    })
                    .collect()
            }
            MotifOp::Retrograde => {
                let total = span(&cur);
                let mut r: Vec<Note> = cur
                    .iter()
                    .map(|n| Note { start: total - n.start - n.length, ..n.clone() })
                    .collect();
                r.sort_by(|a, b| a.start.total_cmp(&b.start));
                r
            }
            MotifOp::Augment { factor } => {
                if !(*factor > 0.0 && *factor <= 8.0) {
                    return Err(format!("augment factor {factor}: want (0, 8]"));
                }
                cur.iter()
                    .map(|n| Note { start: n.start * factor, length: n.length * factor, ..n.clone() })
                    .collect()
            }
            MotifOp::Sequence { times, steps } => {
                if *times > 32 {
                    return Err(format!("sequence times {times}: want 0..=32"));
                }
                let len = span(&cur);
                let mut out = cur.clone();
                for k in 1..=*times as i32 {
                    out.extend(cur.iter().map(|n| Note {
                        pitch: key.step(n.pitch, steps * k),
                        start: n.start + len * k as f32,
                        ..n.clone()
                    }));
                }
                out
            }
            MotifOp::Fragment { from, to } => {
                let out: Vec<Note> = cur
                    .iter()
                    .filter(|n| n.start >= *from && n.start < *to)
                    .map(|n| Note { start: n.start - from, length: n.length.min(to - n.start), ..n.clone() })
                    .collect();
                if out.is_empty() {
                    return Err(format!("fragment {from}..{to}: no notes start there"));
                }
                out
            }
            MotifOp::Displace { beats } => cur
                .iter()
                .map(|n| Note { start: n.start + beats, ..n.clone() })
                .filter(|n| n.start >= 0.0)
                .collect(),
            MotifOp::Octave { octaves } => cur
                .iter()
                .map(|n| Note { pitch: (n.pitch as i32 + 12 * octaves).clamp(0, 127) as u8, ..n.clone() })
                .collect(),
            MotifOp::Dynamics { scale } => cur
                .iter()
                .map(|n| Note { velocity: (n.velocity * scale).clamp(0.0, 1.0), ..n.clone() })
                .collect(),
        };
    }
    Ok(cur)
}

// --------------------------------------------------------- reharmonise

/// Chord vocabulary for re-harmonising.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    /// The key's own triads (no diminished).
    #[default]
    Plain,
    /// Plus sevenths, borrowed iv / bVII / bVI and V/V, V/vi.
    Colour,
}

fn vocabulary(key: &Key, palette: Palette) -> Vec<Chord> {
    let mut names: Vec<&str> = if key.mode.minor_tonic() {
        vec!["i", "iv", "v", "bIII", "bVI", "bVII", "V"]
    } else if key.mode.heptatonic() == Mode::Mixolydian {
        vec!["I", "IV", "bVII", "v", "ii", "vi"]
    } else if key.mode.heptatonic() == Mode::Lydian {
        vec!["I", "II", "V", "vi", "iii", "vii"]
    } else {
        vec!["I", "ii", "iii", "IV", "V", "vi"]
    };
    if palette == Palette::Colour {
        names.extend(if key.mode.minor_tonic() {
            ["i7", "iv7", "V7", "bVImaj7", "bIIImaj7", "ii°", "V/iv"].as_slice()
        } else {
            ["Imaj7", "ii7", "IVmaj7", "V7", "vi7", "iv", "bVII", "bVI", "V/V", "V/vi", "iii7"].as_slice()
        });
    }
    let mut out = Vec::new();
    for n in names {
        if let Ok(c) = parse_numeral(n, key) {
            if !out.iter().any(|o: &Chord| o.symbol == c.symbol) {
                out.push(c);
            }
        }
    }
    out
}

/// How well chord `c` carries the melody notes in a window (higher = better).
fn fit(c: &Chord, notes: &[(u8, f32, f32)], key: &Key) -> f32 {
    let pcs = c.pcs();
    let mut s = 0.0;
    for &(p, dur, weight) in notes {
        let pc = p % 12;
        let w = dur.min(2.0) * weight;
        if pcs.contains(&pc) {
            s += 2.0 * w;
        } else if pcs.iter().any(|&t| (t + 1) % 12 == pc) && !key.contains(pc) {
            s -= 2.0 * w;
        } else if pcs.iter().any(|&t| (t + 1) % 12 == pc) && weight > 0.9 {
            // A semitone over a chord tone on a strong beat rubs.
            s -= 1.5 * w;
        } else if key.contains(pc) {
            s += 0.0;
        } else {
            s -= 1.0 * w;
        }
    }
    // Borrowed / coloured chords cost a little, so plain wins ties.
    if c.pcs().iter().any(|p| !key.contains(*p)) {
        s -= 0.6;
    }
    s
}

fn transition(a: &Chord, b: &Chord, key: &Key) -> f32 {
    let motion = (b.root as i32 - a.root as i32).rem_euclid(12);
    let mut s = match motion {
        5 => 1.0,      // down a fifth (V-I, ii-V)
        0 => -0.8,     // same chord again
        7 => 0.3,      // up a fifth (I-V)
        2 | 10 => 0.4, // by step (IV-V, bVII-I)
        3 | 9 => 0.2,  // by third
        _ => 0.0,
    };
    let tonic = key.tonic;
    let dom = (key.tonic + 7) % 12;
    if a.root == dom && b.root == tonic {
        s += 0.8;
    }
    // Dominant-function chords resolving elsewhere than down a fifth.
    if a.intervals == [0, 4, 7, 10] && motion != 5 {
        s -= 0.7;
    }
    s
}

/// Pick a chord every `every` beats for `melody` over `beats`, in `key`
/// (Viterbi over fit + transition scores; ends on the tonic when it fits).
pub fn reharmonise(
    melody: &[Note],
    key: &Key,
    beats: f32,
    every: f32,
    beats_per_bar: u32,
    palette: Palette,
) -> Result<Vec<(Chord, f32, f32)>, String> {
    if !(every > 0.0) {
        return Err("every: want a positive number of beats".into());
    }
    let vocab = vocabulary(key, palette);
    let n = (beats / every).ceil().max(1.0) as usize;
    let mut windows: Vec<Vec<(u8, f32, f32)>> = vec![Vec::new(); n];
    for m in melody {
        // A note counts in every window it sounds in, weighted by metre.
        let mut t = m.start;
        let end = m.start + m.length;
        while t < end - 1e-4 {
            let w = ((t / every) as usize).min(n - 1);
            let wend = ((w + 1) as f32 * every).min(end);
            let bar_pos = t % beats_per_bar as f32;
            let weight = if bar_pos.abs() < 1e-3 {
                1.0
            } else if (t % 1.0).abs() < 1e-3 {
                0.8
            } else {
                0.5
            };
            let weight = if (t - m.start).abs() < 1e-3 { weight } else { weight * 0.7 };
            windows[w].push((m.pitch, wend - t, weight));
            t = wend;
        }
    }
    let k = vocab.len();
    let mut score = vec![vec![f32::NEG_INFINITY; k]; n];
    let mut back = vec![vec![0usize; k]; n];
    let tonic_idx = vocab.iter().position(|c| c.root == key.tonic);
    for j in 0..k {
        score[0][j] = fit(&vocab[j], &windows[0], key) + if Some(j) == tonic_idx { 0.8 } else { 0.0 };
    }
    for i in 1..n {
        for j in 0..k {
            let f = fit(&vocab[j], &windows[i], key);
            let (bi, bs) = (0..k)
                .map(|p| (p, score[i - 1][p] + transition(&vocab[p], &vocab[j], key)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .expect("vocab");
            score[i][j] = bs + f;
            back[i][j] = bi;
        }
    }
    // Loops: the last chord should lead back to the first.
    let mut best = (0..k)
        .max_by(|&a, &b| {
            let ea = score[n - 1][a];
            let eb = score[n - 1][b];
            ea.total_cmp(&eb)
        })
        .expect("vocab");
    if n > 1 {
        best = (0..k)
            .max_by(|&a, &b| {
                let first_of = |j: usize| {
                    let mut idx = j;
                    for i in (1..n).rev() {
                        idx = back[i][idx];
                    }
                    idx
                };
                let ea = score[n - 1][a] + transition(&vocab[a], &vocab[first_of(a)], key);
                let eb = score[n - 1][b] + transition(&vocab[b], &vocab[first_of(b)], key);
                ea.total_cmp(&eb)
            })
            .expect("vocab");
    }
    let mut path = vec![best; n];
    for i in (1..n).rev() {
        path[i - 1] = back[i][path[i]];
    }
    Ok(path
        .into_iter()
        .enumerate()
        .map(|(i, j)| {
            let start = i as f32 * every;
            (vocab[j].clone(), start, every.min(beats - start))
        })
        .collect())
}

/// Detect a key from note durations (Krumhansl-Kessler profiles); returns
/// the best few with correlation.
pub fn detect_key_from_weights(pc_weight: &[f64; 12]) -> Vec<(Key, f64)> {
    const MAJ: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MIN: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let corr = |prof: &[f64; 12], t: usize| {
        let mx = pc_weight.iter().sum::<f64>() / 12.0;
        let my = prof.iter().sum::<f64>() / 12.0;
        let (mut num, mut dx, mut dy) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            let x = pc_weight[(i + t) % 12] - mx;
            let y = prof[i] - my;
            num += x * y;
            dx += x * x;
            dy += y * y;
        }
        if dx <= 0.0 || dy <= 0.0 {
            0.0
        } else {
            num / (dx * dy).sqrt()
        }
    };
    let mut all: Vec<(Key, f64)> = (0..12)
        .flat_map(|t| {
            [
                (Key { tonic: t as u8, mode: Mode::Major }, corr(&MAJ, t)),
                (Key { tonic: t as u8, mode: Mode::Minor }, corr(&MIN, t)),
            ]
        })
        .collect();
    all.sort_by(|a, b| b.1.total_cmp(&a.1));
    all.truncate(3);
    all
}

/// Chord that best matches a pitch-class profile (template match over
/// major/minor/7th chords on every root); used by transcription.
pub fn chord_from_weights(w: &[f64; 12], key: &Key) -> Option<(Chord, f64)> {
    let total: f64 = w.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let shapes: [&[u8]; 6] = [&[0, 4, 7], &[0, 3, 7], &[0, 4, 7, 10], &[0, 3, 7, 10], &[0, 4, 7, 11], &[0, 5, 7]];
    let mut best: Option<(Chord, f64)> = None;
    for root in 0..12u8 {
        for (si, sh) in shapes.iter().enumerate() {
            let inside: f64 = sh.iter().map(|i| w[((root + i) % 12) as usize]).sum();
            let outside = total - inside;
            // Prefer triads (simpler) unless the 7th really sounds; prefer diatonic roots.
            // Transcriptions hear overtones as extra notes, so a seventh
            // must clearly sound to beat the triad, and chords outside the
            // key need clear evidence.
            let mut s = inside / total - 0.6 * outside / total - if si >= 2 { 0.15 } else { 0.0 };
            if !key.contains(root) {
                s -= 0.08;
            }
            if sh.iter().any(|i| !key.contains((root + i) % 12)) {
                s -= 0.1;
            }
            if best.as_ref().is_none_or(|b| s > b.1) {
                let mut c = make_chord(root, sh.to_vec(), None, None);
                c.numeral = Some(numeral_for(&c, key));
                best = Some((c, s));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(p: &str, start: f32, length: f32) -> Note {
        Note { pitch: super::super::parse_pitch(p).unwrap(), start, length, velocity: 0.8 }
    }

    #[test]
    fn keys_parse_and_step_in_scale() {
        let d = Key::parse("D major").unwrap();
        assert_eq!(d.pcs(), vec![2, 4, 6, 7, 9, 11, 1]);
        assert_eq!(Key::parse("Dm").unwrap().mode, Mode::Minor);
        assert_eq!(Key::parse("A dorian").unwrap().pcs(), vec![9, 11, 0, 2, 4, 6, 7]);
        // D4 up two steps in D major = F#4; down one = C#4.
        assert_eq!(d.step(62, 2), 66);
        assert_eq!(d.step(62, -1), 61);
        assert_eq!(d.step(62, 7), 74);
        assert!(Key::parse("H major").is_err());
        assert!(Key::parse("D wobbly").is_err());
    }

    #[test]
    fn numerals_read_in_the_key() {
        let d = Key::parse("D major").unwrap();
        assert_eq!(parse_numeral("ii", &d).unwrap().symbol, "Em");
        assert_eq!(parse_numeral("V7", &d).unwrap().symbol, "A7");
        assert_eq!(parse_numeral("IVmaj7", &d).unwrap().symbol, "Gmaj7");
        assert_eq!(parse_numeral("bVII", &d).unwrap().symbol, "C");
        assert_eq!(parse_numeral("iv", &d).unwrap().symbol, "Gm");
        assert_eq!(parse_numeral("vii°", &d).unwrap().symbol, "C#dim");
        assert_eq!(parse_numeral("V/V", &d).unwrap().symbol, "E");
        assert_eq!(parse_numeral("IV/3", &d).unwrap().symbol, "G/B");
        let a = Key::parse("A minor").unwrap();
        assert_eq!(parse_numeral("i", &a).unwrap().symbol, "Am");
        assert_eq!(parse_numeral("bVI", &a).unwrap().symbol, "F");
        assert_eq!(parse_numeral("VI", &a).unwrap().symbol, "F");
        assert_eq!(parse_numeral("bVII", &a).unwrap().symbol, "G");
        assert_eq!(parse_numeral("V", &a).unwrap().symbol, "E");
        assert_eq!(parse_symbol("F#m7b5").unwrap().pcs(), vec![6, 9, 0, 4]);
        assert_eq!(parse_symbol("Bbsus4").unwrap().pcs(), vec![10, 3, 5]);
        assert_eq!(parse_chord("Em7", &d).unwrap().numeral.as_deref(), Some("ii7"));
    }

    #[test]
    fn voice_leading_moves_little_and_avoids_parallels() {
        let d = Key::parse("D major").unwrap();
        let chords: Vec<(Chord, f32, f32)> = ["I", "V", "vi", "IV", "I"]
            .iter()
            .enumerate()
            .map(|(i, s)| (parse_numeral(s, &d).unwrap(), i as f32 * 4.0, 4.0))
            .collect();
        let v = voice_lead(&chords, &VoicingOpts::default()).unwrap();
        assert_eq!(v.len(), 5);
        assert_eq!(parallels(&v), 0, "{v:?}");
        for w in v.windows(2) {
            let motion: i32 = w[0].voices.iter().zip(&w[1].voices).map(|(a, b)| (*a as i32 - *b as i32).abs()).sum();
            assert!(motion <= 7, "upper voices leap: {:?} -> {:?}", w[0].voices, w[1].voices);
        }
        // Every voicing holds the chord's tones; bass is the root.
        for c in &v {
            assert_eq!(c.bass % 12, c.chord.root);
            for p in &c.voices {
                assert!(c.chord.pcs().contains(&(p % 12)));
            }
        }
    }

    #[test]
    fn voicing_ranges_take_note_names() {
        let o: VoicingOpts = serde_json::from_str(r#"{"voices":3,"low":"G3","high":79,"bass_low":"E1","bass_high":"D3"}"#).unwrap();
        assert_eq!((o.low, o.high, o.bass_low, o.bass_high), (55, 79, 28, 50));
    }

    #[test]
    fn motif_inversion_mirrors_in_scale_steps() {
        let d = Key::parse("D major").unwrap();
        // D E F# A  ->  inverted around D: D C# B G
        let m = vec![n("D5", 0.0, 1.0), n("E5", 1.0, 1.0), n("F#5", 2.0, 1.0), n("A5", 3.0, 1.0)];
        let inv = transform(&m, &d, &[MotifOp::Invert { axis: None }]).unwrap();
        let names: Vec<String> = inv.iter().map(|x| pitch_name(x.pitch)).collect();
        assert_eq!(names, ["D5", "C#5", "B4", "G4"]);
        let aug = transform(&m, &d, &[MotifOp::Augment { factor: 2.0 }]).unwrap();
        assert_eq!((aug[3].start, aug[3].length), (6.0, 2.0));
        let retro = transform(&m, &d, &[MotifOp::Retrograde]).unwrap();
        assert_eq!(retro[0].pitch, m[3].pitch);
        assert_eq!(retro[0].start, 0.0);
        let seq = transform(&m, &d, &[MotifOp::Sequence { times: 1, steps: -1 }]).unwrap();
        assert_eq!(seq.len(), 8);
        assert_eq!(pitch_name(seq[4].pitch), "C#5");
        assert_eq!(seq[4].start, 4.0);
        for x in &seq {
            assert!(d.contains(x.pitch));
        }
    }

    #[test]
    fn reharmonise_finds_the_obvious_chords() {
        let c = Key::parse("C major").unwrap();
        // Bar 1 outlines C, bar 2 F, bar 3 G, bar 4 C.
        let mel = vec![
            n("E5", 0.0, 2.0), n("G5", 2.0, 2.0),
            n("F5", 4.0, 2.0), n("A5", 6.0, 2.0),
            n("D5", 8.0, 2.0), n("B4", 10.0, 2.0),
            n("C5", 12.0, 4.0),
        ];
        let chords = reharmonise(&mel, &c, 16.0, 4.0, 4, Palette::Plain).unwrap();
        let syms: Vec<&str> = chords.iter().map(|(c, _, _)| c.symbol.as_str()).collect();
        assert_eq!(syms, ["C", "F", "G", "C"]);
    }

    #[test]
    fn key_detection_from_weights() {
        // D major scale weighted toward its tonic triad.
        let mut w = [0.0f64; 12];
        for (pc, x) in [(2, 5.0), (4, 2.0), (6, 4.0), (7, 2.5), (9, 4.5), (11, 2.0), (1, 1.5)] {
            w[pc] = x;
        }
        let k = detect_key_from_weights(&w);
        assert_eq!(k[0].0.name(), "D major");
    }
}
