//! Written-score model: meter, key, notes, rests, chords, beams.
//!
//! Everything here is symbolic — note values are enum variants plus a dot
//! count, never floats — so two scores built the same way compare equal
//! with `==`, which is what makes the MusicXML round-trip test meaningful.

use serde::{Deserialize, Serialize};

/// Meter (time signature): `num` beats of `1/den` notes per bar.
///
/// `bar_beats()` converts to quarter-note beats the way the rest of the
/// engine counts time: 4/4 -> 4.0, 3/4 -> 3.0, 6/8 -> 3.0, 2/2 -> 4.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meter {
    pub num: u8,
    pub den: u8,
}

impl Meter {
    pub fn new(num: u8, den: u8) -> Self {
        Self { num, den }
    }

    /// Quarter-note beats per bar. `None` for a non-musical meter.
    pub fn bar_beats(&self) -> Option<f64> {
        if self.num == 0 || !matches!(self.den, 1 | 2 | 4 | 8 | 16 | 32) {
            return None;
        }
        Some(self.num as f64 * 4.0 / self.den as f64)
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.bar_beats() {
            Some(b) if b > 0.0 => Ok(()),
            _ => Err(format!("bad meter {}/{}", self.num, self.den)),
        }
    }
}

/// Key signature as fifths on the circle (-7 = Cb major .. 0 = C .. +7 = C#).
/// `minor` records mode; it does not change spelling (relative major/minor
/// share a signature) but round-trips through MusicXML `<mode>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySig {
    pub fifths: i8,
    pub minor: bool,
}

impl KeySig {
    pub fn major(fifths: i8) -> Self {
        Self { fifths, minor: false }
    }

    pub fn minor(fifths: i8) -> Self {
        Self { fifths, minor: true }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(-7..=7).contains(&self.fifths) {
            return Err(format!("key fifths {} out of -7..=7", self.fifths));
        }
        Ok(())
    }
}

/// Diatonic step (letter name), C = 0 in pitch-class terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    C,
    D,
    E,
    F,
    G,
    A,
    B,
}

impl Step {
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'C' => Some(Self::C),
            'D' => Some(Self::D),
            'E' => Some(Self::E),
            'F' => Some(Self::F),
            'G' => Some(Self::G),
            'A' => Some(Self::A),
            'B' => Some(Self::B),
            _ => None,
        }
    }

    pub fn to_char(self) -> char {
        match self {
            Self::C => 'C',
            Self::D => 'D',
            Self::E => 'E',
            Self::F => 'F',
            Self::G => 'G',
            Self::A => 'A',
            Self::B => 'B',
        }
    }

    /// Natural semitone offset of the step (C=0, D=2, E=4, F=5, G=7, A=9).
    pub fn natural_pc(self) -> u8 {
        match self {
            Self::C => 0,
            Self::D => 2,
            Self::E => 4,
            Self::F => 5,
            Self::G => 7,
            Self::A => 9,
            Self::B => 11,
        }
    }
}

/// A spelled pitch: letter + accidental + octave in scientific pitch
/// notation (C4 = middle C = MIDI 60). `alter` is semitones off natural
/// (-1 flat, 0 natural, +1 sharp); double accidentals never occur — the
/// speller below only emits -1/0/+1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpelledPitch {
    pub step: Step,
    pub alter: i8,
    pub octave: i8,
}

impl SpelledPitch {
    /// MIDI note number 0..=127. Errors only when out of range.
    pub fn to_midi(self) -> Result<u8, String> {
        let v = (self.octave as i32 + 1) * 12 + self.step.natural_pc() as i32 + self.alter as i32;
        if !(0..=127).contains(&v) {
            return Err(format!(
                "spelled pitch {}{}{} out of MIDI range",
                self.step.to_char(),
                self.octave,
                self.alter
            ));
        }
        Ok(v as u8)
    }

    /// Spell a MIDI pitch in a key: sharp-side keys (fifths >= 0) name
    /// chromatics as sharps, flat-side keys as flats. Naturals stay natural
    /// in every key (C major and F major both call 60 "C", not "B#").
    pub fn from_midi(pitch: u8, key: KeySig) -> Self {
        debug_assert!(pitch <= 127);
        let pc = pitch % 12;
        let octave = (pitch / 12) as i8 - 1;
        // (step, alter) per pitch class, sharp-side then flat-side.
        const SHARP: [(Step, i8); 12] = [
            (Step::C, 0),
            (Step::C, 1),
            (Step::D, 0),
            (Step::D, 1),
            (Step::E, 0),
            (Step::F, 0),
            (Step::F, 1),
            (Step::G, 0),
            (Step::G, 1),
            (Step::A, 0),
            (Step::A, 1),
            (Step::B, 0),
        ];
        const FLAT: [(Step, i8); 12] = [
            (Step::C, 0),
            (Step::D, -1),
            (Step::D, 0),
            (Step::E, -1),
            (Step::E, 0),
            (Step::F, 0),
            (Step::G, -1),
            (Step::G, 0),
            (Step::A, -1),
            (Step::A, 0),
            (Step::B, -1),
            (Step::B, 0),
        ];
        let (step, alter) = if key.fifths >= 0 {
            SHARP[pc as usize]
        } else {
            FLAT[pc as usize]
        };
        Self { step, alter, octave }
    }
}

/// Written note values. `beats()` counts quarter-note beats (quarter = 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteValue {
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
}

impl NoteValue {
    pub fn beats(self) -> f64 {
        match self {
            Self::Whole => 4.0,
            Self::Half => 2.0,
            Self::Quarter => 1.0,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
        }
    }

    /// MusicXML `<type>` name.
    pub fn xml_type(self) -> &'static str {
        match self {
            Self::Whole => "whole",
            Self::Half => "half",
            Self::Quarter => "quarter",
            Self::Eighth => "eighth",
            Self::Sixteenth => "16th",
            Self::ThirtySecond => "32nd",
        }
    }

    pub fn from_xml_type(name: &str) -> Option<Self> {
        match name {
            "whole" => Some(Self::Whole),
            "half" => Some(Self::Half),
            "quarter" => Some(Self::Quarter),
            "eighth" => Some(Self::Eighth),
            "16th" => Some(Self::Sixteenth),
            "32nd" => Some(Self::ThirtySecond),
            _ => None,
        }
    }

    /// Beamble durations: eighth notes and shorter.
    pub fn beamable(self) -> bool {
        self.beats() <= 0.5
    }
}

/// One written note. `midi` is the sounding pitch (source of truth for
/// playback); `pitch` is its spelling in the score key. `dots` is 0 or 1 —
/// the quantizer only emits single dots. `tie_start`/`tie_stop` chain notes
/// across bar lines (and across over-long durations).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotationNote {
    pub pitch: SpelledPitch,
    pub midi: u8,
    pub value: NoteValue,
    pub dots: u8,
    pub tie_start: bool,
    pub tie_stop: bool,
}

impl NotationNote {
    /// Written length in beats: value x 1.5 when dotted.
    pub fn beats(&self) -> f64 {
        if self.dots > 0 { self.value.beats() * 1.5 } else { self.value.beats() }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.midi > 127 {
            return Err(format!("note midi {} out of range", self.midi));
        }
        if self.dots > 1 {
            return Err(format!("note dots {} > 1", self.dots));
        }
        let spelled = self.pitch.to_midi()?;
        if spelled != self.midi {
            return Err(format!(
                "note spelling {}{} (alter {}) = {spelled} disagrees with midi {}",
                self.pitch.step.to_char(),
                self.pitch.octave,
                self.pitch.alter,
                self.midi
            ));
        }
        Ok(())
    }
}

/// Simultaneous notes sharing one onset and duration (one MusicXML
/// `<chord/>` group). Non-empty by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord {
    pub notes: Vec<NotationNote>,
}

impl Chord {
    pub fn beats(&self) -> f64 {
        self.notes.first().map(|n| n.beats()).unwrap_or(0.0)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.notes.is_empty() {
            return Err("chord must hold at least one note".to_string());
        }
        let beats = self.notes[0].beats();
        for n in &self.notes {
            n.validate()?;
            if (n.beats() - beats).abs() > 1e-9 {
                return Err("chord notes must share one duration".to_string());
            }
        }
        Ok(())
    }
}

/// One written rest. `dots` is 0 or 1, like notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotationRest {
    pub value: NoteValue,
    pub dots: u8,
}

impl NotationRest {
    pub fn beats(&self) -> f64 {
        if self.dots > 0 { self.value.beats() * 1.5 } else { self.value.beats() }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.dots > 1 {
            return Err(format!("rest dots {} > 1", self.dots));
        }
        Ok(())
    }
}

/// One sounding slot in a measure: a note, a chord, or a rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasureEvent {
    Note(NotationNote),
    Chord(Chord),
    Rest(NotationRest),
}

impl MeasureEvent {
    pub fn beats(&self) -> f64 {
        match self {
            Self::Note(n) => n.beats(),
            Self::Chord(c) => c.beats(),
            Self::Rest(r) => r.beats(),
        }
    }

    pub fn is_rest(&self) -> bool {
        matches!(self, Self::Rest(_))
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Note(n) => n.validate(),
            Self::Chord(c) => c.validate(),
            Self::Rest(r) => r.validate(),
        }
    }
}

/// A beam group: indices into the owning measure's `events` that share one
/// beam (consecutive eighth notes and shorter inside one beat group).
/// Empty and single-note groups are rejected by validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeamGroup {
    pub events: Vec<usize>,
}

/// One bar: sounding events plus beam groups over them. `number` is
/// 1-based, matching the MusicXML measure number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub number: usize,
    pub events: Vec<MeasureEvent>,
    pub beams: Vec<BeamGroup>,
}

impl Measure {
    /// Total written beats in the bar.
    pub fn beats(&self) -> f64 {
        self.events.iter().map(|e| e.beats()).sum()
    }

    /// Check every event plus beam-group shape. `bar_beats` is the meter's
    /// bar length: event durations must sum to it within 1e-6, and every
    /// beam index must name a beamable (eighth-or-shorter) sounding event.
    pub fn validate(&self, bar_beats: f64) -> Result<(), String> {
        if self.number == 0 {
            return Err("measure number must be >= 1".to_string());
        }
        if self.events.is_empty() {
            return Err(format!("measure {} holds no events", self.number));
        }
        for e in &self.events {
            e.validate()?;
        }
        if (self.beats() - bar_beats).abs() > 1e-6 {
            return Err(format!(
                "measure {} spans {} beats, want bar length {bar_beats}",
                self.number,
                self.beats()
            ));
        }
        for (gi, group) in self.beams.iter().enumerate() {
            if group.events.len() < 2 {
                return Err(format!("beam group {gi} must span >= 2 events"));
            }
            let mut prev = None;
            for &idx in &group.events {
                let event = self
                    .events
                    .get(idx)
                    .ok_or_else(|| format!("beam group {gi} names missing event {idx}"))?;
                if event.is_rest() {
                    return Err(format!("beam group {gi} covers rest at event {idx}"));
                }
                let value = match event {
                    MeasureEvent::Note(n) => n.value,
                    MeasureEvent::Chord(c) => c.notes[0].value,
                    MeasureEvent::Rest(_) => unreachable!(),
                };
                if !value.beamable() {
                    return Err(format!("beam group {gi} covers unbeamble event {idx}"));
                }
                if let Some(p) = prev {
                    if idx != p + 1 {
                        return Err(format!("beam group {gi} must list consecutive events"));
                    }
                }
                prev = Some(idx);
            }
        }
        Ok(())
    }
}

/// One part, one staff, one voice: the whole written piece.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub title: String,
    pub meter: Meter,
    pub key: KeySig,
    pub measures: Vec<Measure>,
}

impl Score {
    pub fn validate(&self, bar_beats: f64) -> Result<(), String> {
        self.meter.validate()?;
        self.key.validate()?;
        if self.measures.is_empty() {
            return Err("score must hold at least one measure".to_string());
        }
        for (i, m) in self.measures.iter().enumerate() {
            if m.number != i + 1 {
                return Err(format!(
                    "measure {} has number {}, want {}",
                    i,
                    m.number,
                    i + 1
                ));
            }
            m.validate(bar_beats)?;
        }
        Ok(())
    }

    /// Quarter-note beats across all measures (== measures x bar length
    /// for a well-formed score).
    pub fn total_beats(&self) -> f64 {
        self.measures.iter().map(|m| m.beats()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_bar_lengths_match_common_signatures() {
        assert_eq!(Meter::new(4, 4).bar_beats(), Some(4.0));
        assert_eq!(Meter::new(3, 4).bar_beats(), Some(3.0));
        assert_eq!(Meter::new(6, 8).bar_beats(), Some(3.0));
        assert_eq!(Meter::new(2, 2).bar_beats(), Some(4.0));
        assert_eq!(Meter::new(0, 4).bar_beats(), None);
        assert_eq!(Meter::new(4, 3).bar_beats(), None);
    }

    #[test]
    fn spelling_follows_key_side() {
        // F# (pitch 66): sharp in G major, Gb in F major.
        let sharp = SpelledPitch::from_midi(66, KeySig::major(1));
        assert_eq!((sharp.step, sharp.alter), (Step::F, 1));
        let flat = SpelledPitch::from_midi(66, KeySig::major(-1));
        assert_eq!((flat.step, flat.alter), (Step::G, -1));
        // Naturals are spelled natural on both sides.
        let nat = SpelledPitch::from_midi(60, KeySig::major(-1));
        assert_eq!((nat.step, nat.alter, nat.octave), (Step::C, 0, 4));
        // Spelling round-trips through MIDI numbers both sides.
        for key in [KeySig::major(3), KeySig::major(-3)] {
            for midi in 21..=108u8 {
                let spelled = SpelledPitch::from_midi(midi, key);
                assert_eq!(spelled.to_midi().unwrap(), midi);
            }
        }
    }

    #[test]
    fn note_validation_catches_spelling_mismatch() {
        let ok = NotationNote {
            pitch: SpelledPitch { step: Step::C, alter: 0, octave: 4 },
            midi: 60,
            value: NoteValue::Quarter,
            dots: 0,
            tie_start: false,
            tie_stop: false,
        };
        assert!(ok.validate().is_ok());
        let bad = NotationNote { midi: 61, ..ok.clone() };
        assert!(bad.validate().is_err());
        let bad_dots = NotationNote { dots: 2, ..ok };
        assert!(bad_dots.validate().is_err());
    }
}
