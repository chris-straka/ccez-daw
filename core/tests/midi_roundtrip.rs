//! Track F validation: MIDI record/playback + asset round-trips.
//!
//! Run: `cargo test --manifest-path core/Cargo.toml --test midi_roundtrip`.

use ccez_core::engine::Engine;
use ccez_core::midi::{GrooveTemplate, MidiClip, MidiNote, Recorder, expand};
use ccez_core::midi::{groove, transport};
use ccez_core::model::Project;

fn sample_clip() -> MidiClip {
    let mut clip = MidiClip::new(4.0);
    let mut n1 = MidiNote::new(1, 60, 100, 0.0, 0.5);
    n1.channel = 1;
    n1.pressure = 0.4;
    n1.timbre = 0.7;
    n1.pitch_bend = 0.5;
    clip.add_note(n1).unwrap();
    let mut n2 = MidiNote::new(2, 64, 90, 0.5, 0.5);
    n2.channel = 2;
    n2.probability = 1.0;
    n2.ratchets = 2; // two 16th hits across the 8th span
    clip.add_note(n2).unwrap();
    let mut n3 = MidiNote::new(3, 67, 110, 1.0, 1.0);
    n3.timing_offset_beats = 0.02; // slightly late: human feel
    clip.add_note(n3).unwrap();
    clip
}

#[test]
fn midi_json_round_trip_is_byte_stable() {
    let clip = sample_clip();
    let bytes = clip.encode().expect("encode");
    let back = MidiClip::decode(&bytes).expect("decode");
    assert_eq!(clip, back);
    // Second encode is byte-identical (sorted order is canonical).
    assert_eq!(bytes, back.encode().expect("re-encode"));
}

#[test]
fn midi_asset_round_trip_through_engine() {
    let dir = std::env::temp_dir().join(format!("ccez-midi-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut engine = Engine::create(&dir, Project::new("p", "Midi RT")).expect("create");
    let clip = sample_clip();
    clip.save_to_engine(&mut engine, "take1.mid.json").expect("save");
    // Manifest carries the blob as midi-kind without reading bytes.
    assert!(engine.asset_keys().iter().any(|e| e.key == "take1.mid.json" && e.kind == "midi"));
    let back = MidiClip::load_from_engine(&engine, "take1.mid.json").expect("load");
    assert_eq!(clip, back);
    // Reopen: the asset survives restart (lazy load, not project.json).
    drop(engine);
    let engine2 = Engine::open(&dir).expect("reopen");
    assert_eq!(MidiClip::load_from_engine(&engine2, "take1.mid.json").expect("load2"), clip);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn midi_record_playback_round_trip() {
    // Playback a plain clip (prob=1, ratchet=1, no offset) through a
    // Recorder: the recorded clip matches note-for-note (pitch, velocity,
    // channel, timing, MPE expression).
    let mut clip = MidiClip::new(4.0);
    let mut a = MidiNote::new(1, 60, 100, 0.0, 0.5);
    a.channel = 3;
    a.pressure = 0.5;
    a.timbre = 0.25;
    a.pitch_bend = -1.0;
    clip.add_note(a).unwrap();
    let mut b = MidiNote::new(2, 64, 80, 1.0, 0.5);
    b.channel = 4;
    clip.add_note(b).unwrap();

    let events = expand(&clip, 0);
    assert_eq!(events.len(), 2);
    // Events are sorted by onset: feed them to the recorder in order.
    let mut rec = Recorder::new(4.0);
    for e in &events {
        rec.note_on(e.pitch, e.velocity, e.channel, e.on_beats, e.pitch_bend, e.pressure, e.timbre);
    }
    for e in &events {
        rec.note_off(e.pitch, e.channel, e.off_beats);
    }
    let back = rec.finish(4.0);
    assert_eq!(back.notes.len(), 2);
    for (want, got) in clip.notes.iter().zip(back.notes.iter()) {
        assert_eq!(want.pitch, got.pitch);
        assert_eq!(want.velocity, got.velocity);
        assert_eq!(want.channel, got.channel);
        assert!((want.start_beats - got.start_beats).abs() < 1e-9);
        assert!((want.len_beats - got.len_beats).abs() < 1e-9);
        assert!((want.pressure - got.pressure).abs() < 1e-9);
        assert!((want.timbre - got.timbre).abs() < 1e-9);
        assert!((want.pitch_bend - got.pitch_bend).abs() < 1e-9);
    }
}

#[test]
fn ratchets_subdivide_and_muted_notes_vanish() {
    let clip = sample_clip();
    let events = expand(&clip, 1);
    // n1 (1) + n2 ratcheted (2) + n3 (1) = 4 sounding notes.
    assert_eq!(events.len(), 4);
    let n2_hits: Vec<_> = events.iter().filter(|e| e.note_id == 2).collect();
    assert_eq!(n2_hits.len(), 2);
    assert!((n2_hits[1].on_beats - n2_hits[0].on_beats - 0.25).abs() < 1e-9);

    let mut muted = sample_clip();
    muted.notes[0].muted = true;
    assert_eq!(expand(&muted, 1).len(), 3);
}

#[test]
fn groove_extract_transfer_round_trip() {
    // A performance with a late off-beat teaches a template; transferring
    // it at full amount onto a straight clip reproduces the offset.
    let mut perf = MidiClip::new(4.0);
    perf.add_note(MidiNote::new(1, 60, 100, 0.0, 0.25)).unwrap();
    let mut late = MidiNote::new(2, 60, 100, 0.0, 0.25);
    late.start_beats = 0.5 + 0.03; // off-beat played 0.03 late
    perf.add_note(late).unwrap();

    let template = groove::extract(&perf, 2).expect("extract");
    // Step 1 (the off-beat) learned +0.03; step 0 stayed ~0.
    assert!((template.offsets[1] - 0.03).abs() < 1e-9, "offsets={:?}", template.offsets);
    assert!(template.offsets[0].abs() < 1e-9);

    let mut straight = MidiClip::new(4.0);
    straight.add_note(MidiNote::new(9, 62, 100, 0.0, 0.25)).unwrap();
    straight.add_note(MidiNote::new(10, 62, 100, 0.5, 0.25)).unwrap();
    groove::apply(&mut straight, &template, 1.0).expect("apply");
    let off: f64 = straight.notes.iter().find(|n| n.note_id == 10).unwrap().timing_offset_beats;
    assert!((off - 0.03).abs() < 1e-9);
    // Zero amount is a no-op.
    let mut untouched = MidiClip::new(4.0);
    untouched.add_note(MidiNote::new(11, 62, 100, 0.5, 0.25)).unwrap();
    groove::apply(&mut untouched, &template, 0.0).expect("apply0");
    assert_eq!(untouched.notes[0].timing_offset_beats, 0.0);

    let _ = GrooveTemplate::flat(2); // identity template exists
    let _ = transport::expand(&straight, 0);
}
