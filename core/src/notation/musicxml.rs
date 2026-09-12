//! MusicXML export (plus a minimal read-back for round-trip tests).
//!
//! [`export_xml`] renders a [`Score`] as a score-partwise MusicXML 4.0
//! document: one part, one staff, one voice. Every measure carries its own
//! `<attributes>` (divisions, key, time, treble clef) so measures stay
//! self-describing; notes carry pitch/rest, duration in divisions, type,
//! dots, ties, and beam begin/continue/end markers.
//!
//! [`parse_xml`] is intentionally *not* a general importer — it reads back
//! exactly what the exporter emits (plus harmless whitespace/attribute-order
//! variation) so `parse(export(score)) == score` is checkable in tests.
//! Real-world MusicXML import (multi-part, multi-voice, backup/forward,
//! grace notes) is out of scope for this track.

use super::model::{
    BeamGroup, Chord, KeySig, Measure, MeasureEvent, Meter, NotationNote, NotationRest, NoteValue,
    Score, SpelledPitch, Step,
};

/// Divisions (ticks per quarter note) used by the exporter.
pub const DIVISIONS: u32 = 480;

/// Beats -> MusicXML duration ticks.
fn ticks(beats: f64) -> u32 {
    (beats * DIVISIONS as f64).round() as u32
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

/// Export a score as a MusicXML 4.0 score-partwise document.
pub fn export_xml(score: &Score) -> Result<String, String> {
    let bar = score.meter.bar_beats().ok_or_else(|| {
        format!("bad meter {}/{}", score.meter.num, score.meter.den)
    })?;
    score.validate(bar)?;

    // Beam lookup: event index -> (position in group, group length).
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n",
    );
    out.push_str("<score-partwise version=\"4.0\">\n");
    out.push_str(&format!("  <work><work-title>{}</work-title></work>\n", escape(&score.title)));
    out.push_str("  <part-list><score-part id=\"P1\"><part-name>Music</part-name></score-part></part-list>\n");
    out.push_str("  <part id=\"P1\">\n");

    for m in &score.measures {
        out.push_str(&format!("    <measure number=\"{}\">\n", m.number));
        out.push_str("      <attributes>\n");
        out.push_str(&format!("        <divisions>{DIVISIONS}</divisions>\n"));
        out.push_str(&format!(
            "        <key><fifths>{}</fifths><mode>{}</mode></key>\n",
            score.key.fifths,
            if score.key.minor { "minor" } else { "major" }
        ));
        out.push_str(&format!(
            "        <time><beats>{}</beats><beat-type>{}</beat-type></time>\n",
            score.meter.num, score.meter.den
        ));
        out.push_str("        <clef><sign>G</sign><line>2</line></clef>\n");
        out.push_str("      </attributes>\n");

        let beams = beam_tags(m);
        for (idx, event) in m.events.iter().enumerate() {
            match event {
                MeasureEvent::Note(n) => {
                    push_note(&mut out, n, false, beams.get(&idx).copied());
                }
                MeasureEvent::Chord(c) => {
                    for (i, n) in c.notes.iter().enumerate() {
                        push_note(&mut out, n, i > 0, beams.get(&idx).copied());
                    }
                }
                MeasureEvent::Rest(r) => push_rest(&mut out, r),
            }
        }
        out.push_str("    </measure>\n");
    }
    out.push_str("  </part>\n</score-partwise>\n");
    Ok(out)
}

/// Beam marker per event index: "begin" | "continue" | "end".
fn beam_tags(m: &Measure) -> std::collections::HashMap<usize, &'static str> {
    let mut tags = std::collections::HashMap::new();
    for g in &m.beams {
        for (i, &idx) in g.events.iter().enumerate() {
            let tag = if i == 0 {
                "begin"
            } else if i + 1 == g.events.len() {
                "end"
            } else {
                "continue"
            };
            tags.insert(idx, tag);
        }
    }
    tags
}

fn push_note(
    out: &mut String,
    n: &NotationNote,
    chord_tail: bool,
    beam: Option<&'static str>,
) {
    out.push_str("      <note>\n");
    if chord_tail {
        out.push_str("        <chord/>\n");
    }
    out.push_str(&format!(
        "        <pitch><step>{}</step>{}<octave>{}</octave></pitch>\n",
        n.pitch.step.to_char(),
        if n.pitch.alter != 0 {
            format!("<alter>{}</alter>", n.pitch.alter)
        } else {
            String::new()
        },
        n.pitch.octave
    ));
    out.push_str(&format!("        <duration>{}</duration>\n", ticks(n.beats())));
    out.push_str("        <voice>1</voice>\n");
    out.push_str(&format!("        <type>{}</type>\n", n.value.xml_type()));
    for _ in 0..n.dots {
        out.push_str("        <dot/>\n");
    }
    if n.tie_stop {
        out.push_str("        <tie type=\"stop\"/>\n");
    }
    if n.tie_start {
        out.push_str("        <tie type=\"start\"/>\n");
    }
    if let Some(tag) = beam {
        out.push_str(&format!("        <beam number=\"1\">{tag}</beam>\n"));
    }
    out.push_str("      </note>\n");
}

fn push_rest(out: &mut String, r: &NotationRest) {
    out.push_str("      <note>\n");
    out.push_str("        <rest/>\n");
    out.push_str(&format!("        <duration>{}</duration>\n", ticks(r.beats())));
    out.push_str("        <voice>1</voice>\n");
    out.push_str(&format!("        <type>{}</type>\n", r.value.xml_type()));
    for _ in 0..r.dots {
        out.push_str("        <dot/>\n");
    }
    out.push_str("      </note>\n");
}

// ---------------------------------------------------------------------------
// Minimal read-back (test support, not a general importer).
// ---------------------------------------------------------------------------

/// Parsed note element (one `<note>...</note>` block).
#[derive(Debug, Clone)]
struct RawNote {
    chord_tail: bool,
    step: Option<char>,
    alter: i8,
    octave: i8,
    is_rest: bool,
    value: NoteValue,
    dots: u8,
    tie_start: bool,
    tie_stop: bool,
    beam: Option<String>,
}

/// Take the text between `<tag...>` and `</tag>` (first occurrence at or
/// after `from`). Returns (content, position after the close tag).
fn take_block<'a>(xml: &'a str, from: usize, tag: &str) -> Option<(&'a str, usize)> {
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}>");
    let open = xml[from..].find(&open_pat)? + from;
    let body_start = xml[open..].find('>')? + open + 1;
    let body_end_rel = xml[body_start..].find(&close_pat)?;
    let body_end = body_start + body_end_rel;
    Some((&xml[body_start..body_end], body_end + close_pat.len()))
}

/// Text of the first `<tag>...</tag>` inside `block`, trimmed.
fn child_text(block: &str, tag: &str) -> Option<String> {
    let (body, _) = take_block(block, 0, tag)?;
    // Reject nested same-name tags leaking in: our blocks are flat.
    Some(body.trim().to_string())
}

fn parse_raw_note(block: &str) -> Result<RawNote, String> {
    let chord_tail = block.contains("<chord");
    let is_rest = block.contains("<rest");
    let type_name = child_text(block, "type")
        .ok_or_else(|| "note element missing <type>".to_string())?;
    let value = NoteValue::from_xml_type(&type_name)
        .ok_or_else(|| format!("unknown note type `{type_name}`"))?;
    let dots = block.matches("<dot").count() as u8;
    if dots > 1 {
        return Err(format!("note has {dots} dots, want <= 1"));
    }
    let tie_start = block.contains("<tie type=\"start\"");
    let tie_stop = block.contains("<tie type=\"stop\"");
    let beam = child_text(block, "beam");
    let (step, alter, octave) = if is_rest {
        (None, 0, 0)
    } else {
        let pitch_block = take_block(block, 0, "pitch")
            .ok_or_else(|| "note element missing <pitch>".to_string())?
            .0;
        let step = child_text(pitch_block, "step")
            .ok_or_else(|| "pitch missing <step>".to_string())?;
        let step_ch = step
            .chars()
            .next()
            .filter(|c| Step::from_char(*c).is_some())
            .ok_or_else(|| format!("bad step `{step}`"))?;
        let alter = match child_text(pitch_block, "alter") {
            Some(a) => a.parse::<i8>().map_err(|_| format!("bad alter `{a}`"))?,
            None => 0,
        };
        let octave = child_text(pitch_block, "octave")
            .ok_or_else(|| "pitch missing <octave>".to_string())?
            .parse::<i8>()
            .map_err(|e| format!("bad octave: {e}"))?;
        (Some(step_ch), alter, octave)
    };
    Ok(RawNote { chord_tail, step, alter, octave, is_rest, value, dots, tie_start, tie_stop, beam })
}

/// Parse a document produced by [`export_xml`] back into a [`Score`].
/// Rejects unknown note types, missing types, dots > 1, and measures whose
/// events do not sum to the bar length — the same invariants `export`
/// guarantees, so any failure here means the XML did not come from us.
pub fn parse_xml(xml: &str) -> Result<Score, String> {
    let title = take_block(xml, 0, "work-title").map(|(b, _)| unescape(b.trim())).unwrap_or_default();

    // First measure's attributes give meter + key (exporter repeats them
    // per measure; all copies must agree — checked below).
    let mut meter: Option<Meter> = None;
    let mut key: Option<KeySig> = None;
    let mut measures: Vec<Measure> = Vec::new();

    let mut pos = 0;
    while let Some((body, next)) = take_block(xml, pos, "measure") {
        pos = next;
        let number: usize = {
            // Re-find the opening tag to read its number attribute.
            let open = xml[..next].rfind("<measure").expect("measure open");
            let tag_end = xml[open..].find('>').expect("measure tag") + open;
            let tag = &xml[open..tag_end];
            let num_start = tag
                .find("number=\"")
                .ok_or_else(|| "measure missing number".to_string())?
                + 8;
            let num_end = tag[num_start..]
                .find('"')
                .ok_or_else(|| "bad measure number".to_string())?
                + num_start;
            tag[num_start..num_end]
                .parse()
                .map_err(|_| "bad measure number".to_string())?
        };
        // Attributes (required on every measure we emit).
        let attr = take_block(body, 0, "attributes")
            .ok_or_else(|| format!("measure {number} missing <attributes>"))?
            .0;
        let key_block = take_block(attr, 0, "key")
            .ok_or_else(|| format!("measure {number} missing <key>"))?
            .0;
        let fifths: i8 = child_text(key_block, "fifths")
            .ok_or_else(|| format!("measure {number} key missing <fifths>"))?
            .parse()
            .map_err(|_| format!("measure {number} has bad <fifths>"))?;
        let minor = child_text(key_block, "mode").as_deref() == Some("minor");
        let time_block = take_block(attr, 0, "time")
            .ok_or_else(|| format!("measure {number} missing <time>"))?
            .0;
        let num: u8 = child_text(time_block, "beats")
            .ok_or_else(|| format!("measure {number} time missing <beats>"))?
            .parse()
            .map_err(|_| format!("measure {number} has bad <beats>"))?;
        let den: u8 = child_text(time_block, "beat-type")
            .ok_or_else(|| format!("measure {number} time missing <beat-type>"))?
            .parse()
            .map_err(|_| format!("measure {number} has bad <beat-type>"))?;
        let m = Meter::new(num, den);
        let k = KeySig { fifths, minor };
        match (&meter, &key) {
            (None, None) => {
                meter = Some(m);
                key = Some(k);
            }
            _ => {
                if meter != Some(m) || key != Some(k) {
                    return Err(format!("measure {number} changes meter or key"));
                }
            }
        }

        // Notes: <chord/> tails join the previous note into a chord group.
        let mut events: Vec<MeasureEvent> = Vec::new();
        let mut beam_marks: Vec<Option<String>> = Vec::new();
        let mut npos = 0;
        while let Some((nbody, nnext)) = take_block(body, npos, "note") {
            npos = nnext;
            let raw = parse_raw_note(nbody)?;
            if raw.is_rest {
                if raw.chord_tail {
                    return Err(format!("measure {number}: rest cannot be a chord tail"));
                }
                events.push(MeasureEvent::Rest(NotationRest {
                    value: raw.value,
                    dots: raw.dots,
                }));
                beam_marks.push(None);
            } else {
                let step = Step::from_char(raw.step.expect("pitch checked")).expect("pitch checked");
                let pitch = SpelledPitch { step, alter: raw.alter, octave: raw.octave };
                let midi = pitch.to_midi()?;
                let note = NotationNote {
                    pitch,
                    midi,
                    value: raw.value,
                    dots: raw.dots,
                    tie_start: raw.tie_start,
                    tie_stop: raw.tie_stop,
                };
                if raw.chord_tail {
                    // Join the previous event (note or chord); rests and
                    // duration mismatches are errors. The tail shares the
                    // head's beam slot: its mark must agree, and no second
                    // slot is pushed (marks stay parallel to events).
                    let prev = events.pop().ok_or_else(|| {
                        format!("measure {number}: chord tail with no head")
                    })?;
                    let head_mark = beam_marks.pop().ok_or_else(|| {
                        format!("measure {number}: chord tail with no head")
                    })?;
                    if head_mark != raw.beam {
                        return Err(format!("measure {number}: chord beam marks disagree"));
                    }
                    let mut notes = match prev {
                        MeasureEvent::Note(n) => vec![n],
                        MeasureEvent::Chord(c) => c.notes,
                        MeasureEvent::Rest(_) => {
                            return Err(format!("measure {number}: chord tail after rest"));
                        }
                    };
                    if (notes[0].beats() - note.beats()).abs() > 1e-9 {
                        return Err(format!("measure {number}: chord notes differ in length"));
                    }
                    notes.push(note);
                    notes.sort_by_key(|n| n.midi);
                    events.push(MeasureEvent::Chord(Chord { notes }));
                    beam_marks.push(head_mark);
                } else {
                    events.push(MeasureEvent::Note(note));
                    beam_marks.push(raw.beam);
                }
            }
        }

        // Rebuild beam groups from begin/continue/end marks (one mark
        // per event slot; chord tails share their head's slot).
        let mut beams: Vec<BeamGroup> = Vec::new();
        let mut run: Vec<usize> = Vec::new();
        for (idx, mark) in beam_marks.iter().enumerate() {
            match mark.as_deref() {
                None => {
                    if !run.is_empty() {
                        return Err(format!("measure {number}: beam run left open"));
                    }
                }
                Some("begin") => {
                    if !run.is_empty() {
                        return Err(format!("measure {number}: nested beam begin"));
                    }
                    run.push(idx);
                }
                Some("continue") => {
                    if run.is_empty() {
                        return Err(format!("measure {number}: beam continue with no begin"));
                    }
                    run.push(idx);
                }
                Some("end") => {
                    if run.is_empty() {
                        return Err(format!("measure {number}: beam end with no begin"));
                    }
                    run.push(idx);
                    beams.push(BeamGroup { events: std::mem::take(&mut run) });
                }
                Some(other) => return Err(format!("measure {number}: bad beam `{other}`")),
            }
        }
        if !run.is_empty() {
            return Err(format!("measure {number}: beam run left open"));
        }
        measures.push(Measure { number, events, beams });
    }

    let (meter, key) = match (meter, key) {
        (Some(m), Some(k)) => (m, k),
        _ => return Err("document holds no measures".to_string()),
    };
    let bar = meter.bar_beats().ok_or_else(|| {
        format!("bad meter {}/{}", meter.num, meter.den)
    })?;
    let score = Score { title, meter, key, measures };
    score.validate(bar)?;
    Ok(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{MidiClip, MidiNote};
    use crate::notation::quantize::{QuantizeOptions, quantize};

    fn demo_score() -> Score {
        // G major, 4/4: triad + beamed eighths + tied cross-bar whole feel.
        let mut clip = MidiClip::new(8.0);
        for (id, pitch, start, len) in [
            (1u32, 60u8, 0.0f64, 1.0f64),
            (2, 64, 0.0, 1.0),
            (3, 67, 0.0, 1.0),
            (4, 66, 1.0, 0.5),
            (5, 69, 1.5, 0.5),
            (6, 71, 2.0, 2.0),
            (7, 72, 3.0, 2.0),
        ] {
            clip.add_note(MidiNote::new(id, pitch, 90, start, len)).unwrap();
        }
        let opts = QuantizeOptions {
            key: KeySig::major(1),
            ..QuantizeOptions::default()
        };
        quantize(&clip, &opts).unwrap()
    }

    #[test]
    fn export_contains_expected_musicxml_shapes() {
        let score = demo_score();
        let xml = export_xml(&score).unwrap();
        assert!(xml.contains("<score-partwise version=\"4.0\">"));
        assert!(xml.contains("<fifths>1</fifths>"));
        assert!(xml.contains("<beats>4</beats><beat-type>4</beat-type>"));
        assert!(xml.contains("<chord/>"), "triad must emit chord joins");
        assert!(xml.contains("<beam number=\"1\">begin</beam>"), "eighths must beam");
        assert!(xml.contains("<tie type=\"start\""), "bar 1->2 whole note must tie");
        assert!(xml.contains("<step>F</step><alter>1</alter>"), "F# spelling must survive");
        // Two bars: the 4-beat note at beat 4 fills bar 2.
        assert_eq!(xml.matches("<measure number=").count(), 2);
    }

    #[test]
    fn musicxml_round_trip_is_exact() {
        let score = demo_score();
        let xml = export_xml(&score).unwrap();
        let back = parse_xml(&xml).unwrap();
        assert_eq!(score, back);
    }

    #[test]
    fn musicxml_round_trip_covers_minor_key_and_triple_meter() {
        let mut clip = MidiClip::new(6.0);
        clip.add_note(MidiNote::new(1, 58, 90, 0.0, 1.0)).unwrap(); // Bb in F major
        clip.add_note(MidiNote::new(2, 62, 90, 1.0, 0.5)).unwrap();
        clip.add_note(MidiNote::new(3, 60, 90, 1.5, 0.5)).unwrap();
        let opts = QuantizeOptions {
            meter: Meter::new(3, 4),
            key: KeySig::major(-1),
            ..QuantizeOptions::default()
        };
        let score = quantize(&clip, &opts).unwrap();
        let back = parse_xml(&export_xml(&score).unwrap()).unwrap();
        assert_eq!(score, back);
        assert!(export_xml(&score).unwrap().contains("<mode>major</mode>"));
        // Minor mode flag round-trips too.
        let mut minor_score = score.clone();
        minor_score.key = KeySig::minor(-1);
        let minor_back = parse_xml(&export_xml(&minor_score).unwrap()).unwrap();
        assert_eq!(minor_score, minor_back);
    }

    #[test]
    fn parse_rejects_non_exporter_shapes() {
        assert!(parse_xml("<score-partwise></score-partwise>").is_err());
        let mut bad = export_xml(&demo_score()).unwrap();
        bad = bad.replacen("<type>quarter</type>", "<type>breve</type>", 1);
        assert!(parse_xml(&bad).is_err());
    }

    #[test]
    fn title_escapes_and_returns() {
        let mut score = demo_score();
        score.title = "Fish & Chips <live>".to_string();
        let back = parse_xml(&export_xml(&score).unwrap()).unwrap();
        assert_eq!(back.title, "Fish & Chips <live>");
    }
}
