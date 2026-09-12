//! Notation model: MIDI <-> written-score quantization plus MusicXML export.
//!
//! Teaching note: MIDI records *performance* (note X started at beat 3.97
//! for 0.48 beats). Notation records *intent* ("an eighth note on D5 on the
//! off-beat of 3"). This module converts between the two and serializes the
//! written side as MusicXML:
//!
//! - [`model`]: the written-score types. A [`Score`](crate::notation::Score)
//!   is one part (one staff, one voice): a [`Meter`], a [`KeySig`], and a
//!   list of [`Measure`]s. Each measure holds [`MeasureEvent`]s —
//!   [`NotationNote`], [`Chord`] (simultaneous notes), or [`NotationRest`] —
//!   plus [`BeamGroup`]s naming which event indices are beamed together.
//!   Everything is symbolic (no floats), so scores compare with `==`.
//! - [`quantize`]: [`quantize`](crate::notation::quantize) snaps a
//!   [`MidiClip`](crate::midi::MidiClip) onto a grid inside the bar lines
//!   implied by the meter, spells pitch names from the key signature, fills
//!   gaps with rests, splits cross-bar notes with ties, and auto-beams
//!   eighth notes and shorter. [`to_midi`](crate::notation::to_midi) renders
//!   a score back to a clip (the written part, not the original feel).
//! - [`musicxml`]: [`export_xml`](crate::notation::export_xml) writes a
//!   score-partwise MusicXML 4.0 document (one part, one staff, voice 1).
//!   [`parse_xml`](crate::notation::parse_xml) reads back exactly what the
//!   exporter emits so round-trips are testable; it is *not* a general
//!   MusicXML importer (no multi-part, multi-voice, or backup/forward
//!   support) — import is out of scope for this track.
//!
//! The module reads the frozen v0 [`Clip`](crate::model::Clip) shape only
//! through the opaque MIDI-asset convention (like `midi/`): it adds no IPC,
//! no project-schema fields, and no generated-TS surface, so the typegen
//! drift gate (`bun run check`) is unaffected.

pub mod model;
pub mod musicxml;
pub mod quantize;

pub use model::{
    BeamGroup, Chord, KeySig, Measure, MeasureEvent, Meter, NotationNote, NotationRest, NoteValue,
    Score, SpelledPitch, Step,
};
pub use musicxml::{DIVISIONS, export_xml, parse_xml};
pub use quantize::{QuantizeOptions, quantize, to_midi};
