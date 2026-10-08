//! Game-music composition: notes in, audioforge-ready stems out.
//!
//! Teaching note: game music ships as *stems*, not a song. A
//! [`Composition`] names sections (an `intro` that plays once and hands
//! off, `loop` sections the game switches between, an `outro` that plays
//! once then goes silent) and adaptive *layers* (a bed that always plays,
//! more layers that fade in as the game's intensity rises 1..=5). Every
//! (section, layer) pair with notes renders to one mono stem; the game
//! runtime (audioforge, `~/SWE/audio/audioforge`) mixes the stems by game state.
//!
//! The document is plain JSON so agents (the MCP `compose_*` tools) and
//! people author the same thing:
//!
//! ```json
//! { "name": "theme", "tempo": 84, "beats_per_bar": 4, "key": "D minor",
//!   "instruments": [{ "id": "keys", "patch": "felt_piano" }],
//!   "layers": [{ "id": "bed", "min_intensity": 1 }],
//!   "sections": [{ "id": "calm", "role": "loop", "bars": 4 }],
//!   "parts": [{ "section": "calm", "layer": "bed", "instrument": "keys",
//!               "notes": [{ "pitch": "D4", "start": 0, "length": 2 }] }] }
//! ```
//!
//! Pieces: [`synth`] (instrument patches), [`render`] (stems with loop
//! tails folded onto the loop start, so loops are seamless by
//! construction; a mono preview mixdown with stats agents can read), and
//! [`audioforge`] (the export package audioforge's interactive music
//! loads: `score.json` + loop-kind events + stems + `mix.json`).

pub mod audioforge;
pub mod render;
pub mod synth;

use serde::{Deserialize, Deserializer, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const SAMPLE_RATE: u32 = 48_000;
pub const MAX_INTENSITY: u8 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    #[serde(default = "schema_v1")]
    pub schema_version: u32,
    /// Lowercase id (`[a-z0-9_]+`); prefixes the exported event names.
    pub name: String,
    /// Beats per minute, [40, 300].
    pub tempo: f32,
    #[serde(default = "four")]
    pub beats_per_bar: u32,
    /// Informational (e.g. `D minor`); pitches are absolute.
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub instruments: Vec<Instrument>,
    #[serde(default)]
    pub layers: Vec<LayerDef>,
    #[serde(default)]
    pub sections: Vec<SectionDef>,
    #[serde(default)]
    pub parts: Vec<Part>,
}

fn schema_v1() -> u32 {
    SCHEMA_VERSION
}
fn four() -> u32 {
    4
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instrument {
    pub id: String,
    /// One of [`synth::PATCHES`].
    pub patch: String,
    #[serde(default)]
    pub gain_db: f32,
    /// Reverb send, [0, 1].
    #[serde(default = "default_reverb")]
    pub reverb: f32,
}

fn default_reverb() -> f32 {
    0.25
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerDef {
    pub id: String,
    /// Lowest game intensity (1..=5) at which this layer sounds.
    #[serde(default = "one")]
    pub min_intensity: u8,
    #[serde(default)]
    pub gain_db: f32,
}

fn one() -> u8 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Plays once, then hands off to `next` on its last beat.
    Intro,
    /// Loops until the game switches section.
    Loop,
    /// Plays once, then silence.
    Outro,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionDef {
    pub id: String,
    pub role: Role,
    pub bars: u32,
    /// Intro only: the section that follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub section: String,
    pub layer: String,
    pub instrument: String,
    #[serde(default)]
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// MIDI note number, or a name like `D4`, `F#3`, `Bb2` (C4 = 60).
    #[serde(deserialize_with = "de_pitch")]
    pub pitch: u8,
    /// Start in beats from the section start.
    pub start: f32,
    /// Length in beats.
    pub length: f32,
    /// [0, 1].
    #[serde(default = "default_velocity")]
    pub velocity: f32,
}

fn default_velocity() -> f32 {
    0.8
}

fn de_pitch<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum P {
        N(f64),
        S(String),
    }
    match P::deserialize(d)? {
        P::N(n) if n.fract() == 0.0 && (0.0..=127.0).contains(&n) => Ok(n as u8),
        P::N(n) => Err(serde::de::Error::custom(format!(
            "pitch {n} is not a MIDI note 0..=127"
        ))),
        P::S(s) => parse_pitch(&s).map_err(serde::de::Error::custom),
    }
}

/// `C4` = 60. Accepts `#`/`b` accidentals and octaves -1..=9.
pub fn parse_pitch(s: &str) -> Result<u8, String> {
    let t = s.trim();
    let mut chars = t.chars();
    let letter = chars.next().ok_or_else(|| "empty pitch".to_string())?;
    let base: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(format!("pitch {s:?}: want a note name like D4 or F#3")),
    };
    let rest: String = chars.collect();
    let (acc, oct) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let octave: i32 = oct
        .parse()
        .map_err(|_| format!("pitch {s:?}: missing octave (want e.g. D4)"))?;
    let n = (octave + 1) * 12 + base + acc;
    if !(0..=127).contains(&n) {
        return Err(format!("pitch {s:?} is outside MIDI 0..=127"));
    }
    Ok(n as u8)
}

/// Output of [`Composition::validate_report`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub pending: Vec<String>,
}

const NOT_DEFINED_YET: &str = "(not defined yet)";

fn is_pending(msg: &str) -> bool {
    msg.ends_with(NOT_DEFINED_YET)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('_')
        && !id.ends_with('_')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl Composition {
    pub fn new(name: &str, tempo: f32, beats_per_bar: u32) -> Self {
        Composition {
            schema_version: SCHEMA_VERSION,
            name: name.to_string(),
            tempo,
            beats_per_bar,
            key: String::new(),
            instruments: vec![],
            layers: vec![],
            sections: vec![],
            parts: vec![],
        }
    }

    pub fn section(&self, id: &str) -> Option<&SectionDef> {
        self.sections.iter().find(|s| s.id == id)
    }
    pub fn layer(&self, id: &str) -> Option<&LayerDef> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn instrument(&self, id: &str) -> Option<&Instrument> {
        self.instruments.iter().find(|i| i.id == id)
    }

    pub fn section_beats(&self, s: &SectionDef) -> f32 {
        (s.bars * self.beats_per_bar) as f32
    }

    pub fn secs_per_beat(&self) -> f64 {
        60.0 / self.tempo as f64
    }

    /// Every problem, each naming the field and what is wanted. Empty =
    /// renderable and exportable.
    pub fn validate(&self) -> Vec<String> {
        let r = self.validate_report();
        r.errors.into_iter().chain(r.pending).collect()
    }

    /// [`Self::validate`] split for incremental authoring: `pending` are
    /// references to things not defined *yet* (no loop section, an intro's
    /// `next`, a part's section/layer/instrument), normal while a piece is
    /// being built up; `errors` are wrong values that need fixing now.
    pub fn validate_report(&self) -> ValidationReport {
        let all = self.validate_all();
        let (pending, errors) = all.into_iter().partition(|m| is_pending(m));
        ValidationReport { errors, pending }
    }

    fn validate_all(&self) -> Vec<String> {
        let mut e = Vec::new();
        if self.schema_version != SCHEMA_VERSION {
            e.push(format!(
                "schema_version: want {SCHEMA_VERSION}, got {}",
                self.schema_version
            ));
        }
        if !valid_id(&self.name) {
            e.push(format!("name {:?}: want lowercase [a-z0-9_]+", self.name));
        }
        if !self.tempo.is_finite() || !(40.0..=300.0).contains(&self.tempo) {
            e.push(format!("tempo {}: want 40..=300 bpm", self.tempo));
        }
        if !(1..=16).contains(&self.beats_per_bar) {
            e.push(format!("beats_per_bar {}: want 1..=16", self.beats_per_bar));
        }
        let mut seen = std::collections::HashSet::new();
        for (kind, id) in self
            .instruments
            .iter()
            .map(|i| ("instrument", &i.id))
            .chain(self.layers.iter().map(|l| ("layer", &l.id)))
            .chain(self.sections.iter().map(|s| ("section", &s.id)))
        {
            if !valid_id(id) {
                e.push(format!("{kind} id {id:?}: want lowercase [a-z0-9_]+"));
            }
            if !seen.insert((kind, id.clone())) {
                e.push(format!("{kind} id {id:?} is defined twice"));
            }
        }
        for i in &self.instruments {
            if synth::patch(&i.patch).is_none() {
                e.push(format!(
                    "instrument {:?}: unknown patch {:?} (known: {})",
                    i.id,
                    i.patch,
                    synth::PATCHES
                        .iter()
                        .map(|p| p.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !(0.0..=1.0).contains(&i.reverb) {
                e.push(format!(
                    "instrument {:?}: reverb {} want 0..=1",
                    i.id, i.reverb
                ));
            }
            if !(-60.0..=12.0).contains(&i.gain_db) {
                e.push(format!(
                    "instrument {:?}: gain_db {} want -60..=12",
                    i.id, i.gain_db
                ));
            }
        }
        for l in &self.layers {
            if !(1..=MAX_INTENSITY).contains(&l.min_intensity) {
                e.push(format!(
                    "layer {:?}: min_intensity {} want 1..=5",
                    l.id, l.min_intensity
                ));
            }
            if !(-60.0..=12.0).contains(&l.gain_db) {
                e.push(format!(
                    "layer {:?}: gain_db {} want -60..=12",
                    l.id, l.gain_db
                ));
            }
        }
        if !self.sections.iter().any(|s| s.role == Role::Loop) {
            e.push(format!(
                "sections: need at least one loop section {NOT_DEFINED_YET}"
            ));
        }
        for s in &self.sections {
            if s.bars == 0 || s.bars > 256 {
                e.push(format!("section {:?}: bars {} want 1..=256", s.id, s.bars));
            }
            match (s.role, &s.next) {
                (Role::Intro, None) => {
                    e.push(format!("section {:?}: an intro needs next (the section it hands off to)", s.id))
                }
                (Role::Intro, Some(n)) if self.section(n).is_none() => {
                    e.push(format!("section {:?}: next {n:?} is not a section {NOT_DEFINED_YET}", s.id))
                }
                (Role::Loop | Role::Outro, Some(_)) => e.push(format!(
                    "section {:?}: only an intro has next (loops run until the game switches; outros end in silence)",
                    s.id
                )),
                _ => {}
            }
        }
        for (pi, p) in self.parts.iter().enumerate() {
            let at = format!("parts[{pi}] ({}/{})", p.section, p.layer);
            let sec = self.section(&p.section);
            if sec.is_none() {
                e.push(format!(
                    "{at}: unknown section {:?} {NOT_DEFINED_YET}",
                    p.section
                ));
            }
            if self.layer(&p.layer).is_none() {
                e.push(format!(
                    "{at}: unknown layer {:?} {NOT_DEFINED_YET}",
                    p.layer
                ));
            }
            if self.instrument(&p.instrument).is_none() {
                e.push(format!(
                    "{at}: unknown instrument {:?} {NOT_DEFINED_YET}",
                    p.instrument
                ));
            }
            if let Some(sec) = sec {
                let len = self.section_beats(sec);
                for (ni, n) in p.notes.iter().enumerate() {
                    if !n.start.is_finite() || n.start < 0.0 || n.start >= len {
                        e.push(format!(
                            "{at} notes[{ni}]: start {} outside the section (0..{len} beats)",
                            n.start
                        ));
                    }
                    if !n.length.is_finite() || n.length <= 0.0 || n.length > 64.0 {
                        e.push(format!(
                            "{at} notes[{ni}]: length {} want (0, 64] beats",
                            n.length
                        ));
                    }
                    if !(0.0..=1.0).contains(&n.velocity) {
                        e.push(format!(
                            "{at} notes[{ni}]: velocity {} want 0..=1",
                            n.velocity
                        ));
                    }
                }
            }
        }
        e
    }

    /// The (section, layer) pairs that have notes, in section then layer
    /// order: one stem each.
    pub fn stem_pairs(&self) -> Vec<(&SectionDef, &LayerDef)> {
        let mut out = Vec::new();
        for s in &self.sections {
            for l in &self.layers {
                if self
                    .parts
                    .iter()
                    .any(|p| p.section == s.id && p.layer == l.id && !p.notes.is_empty())
                {
                    out.push((s, l));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_names_parse() {
        assert_eq!(parse_pitch("C4"), Ok(60));
        assert_eq!(parse_pitch("A4"), Ok(69));
        assert_eq!(parse_pitch("F#3"), Ok(54));
        assert_eq!(parse_pitch("Bb2"), Ok(46));
        assert_eq!(parse_pitch("C-1"), Ok(0));
        assert!(parse_pitch("H2").is_err());
        assert!(parse_pitch("D").is_err());
        let n: Note = serde_json::from_str(r#"{"pitch":"D4","start":0,"length":1}"#).unwrap();
        assert_eq!((n.pitch, n.velocity), (62, 0.8));
        let n: Note = serde_json::from_str(r#"{"pitch":62,"start":0,"length":1}"#).unwrap();
        assert_eq!(n.pitch, 62);
        assert!(serde_json::from_str::<Note>(r#"{"pitch":200,"start":0,"length":1}"#).is_err());
    }

    /// The shared fixture audioforge's cross-repo test also plays
    /// (`contracts/fixtures/audioforge-tiny.composition.json`).
    pub(crate) fn tiny() -> Composition {
        serde_json::from_str(include_str!(
            "../../../contracts/fixtures/audioforge-tiny.composition.json"
        ))
        .unwrap()
    }

    #[test]
    fn tiny_composition_validates() {
        assert_eq!(tiny().validate(), Vec::<String>::new());
        assert_eq!(tiny().stem_pairs().len(), 4);
    }

    #[test]
    fn validation_names_the_problem() {
        let mut c = tiny();
        c.instruments[0].patch = "kazoo".into();
        c.sections[0].next = None;
        c.parts[1].notes[0].start = 8.0; // calm is 8 beats: 0..8
        c.layers[1].min_intensity = 9;
        let errs = c.validate().join("\n");
        assert!(errs.contains("unknown patch \"kazoo\""), "{errs}");
        assert!(errs.contains("an intro needs next"), "{errs}");
        assert!(errs.contains("start 8 outside the section"), "{errs}");
        assert!(errs.contains("min_intensity 9"), "{errs}");
    }

    #[test]
    fn forward_references_are_pending_not_errors() {
        let mut c = Composition::new("t", 90.0, 4);
        c.sections.push(SectionDef {
            id: "intro".into(),
            role: Role::Intro,
            bars: 1,
            next: Some("calm".into()),
        });
        let r = c.validate_report();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.pending.len(), 2, "{:?}", r.pending); // no loop yet; next not defined
        c.tempo = 999.0;
        assert!(c.validate_report().errors[0].contains("tempo 999"));
        assert_eq!(
            c.validate().len(),
            3,
            "the export gate still sees everything"
        );
    }
}
