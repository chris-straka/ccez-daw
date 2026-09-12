//! Track I validation: pitch-edit round-trips + warp/repair invariants.
//!
//! Run: `cargo test --manifest-path core/Cargo.toml --test timepitch_repair_roundtrip`.

use ccez_core::midi::{MidiClip, MidiNote};
use ccez_core::repair;
use ccez_core::timepitch::{self, WarpMap, WarpMarker};

fn phrase() -> MidiClip {
    timepitch::sample_phrase()
}

#[test]
fn round_trip_pitch_edit_restores_every_note() {
    // THE validation: a per-clip pitch edit (+7 st, the Track I demo edit)
    // undone (-7 st) returns the clip bit-for-bit — pitch edits are lossless
    // views, and the engine undo path restores bytes, not approximations.
    let clip = phrase();
    let bytes = clip.encode().unwrap();
    let up = timepitch::transpose_clip(&clip, 7.0).unwrap();
    assert_eq!(up.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![67, 71, 74]);
    let back = timepitch::transpose_clip(&up, -7.0).unwrap();
    assert_eq!(back, clip);
    assert_eq!(back.encode().unwrap(), bytes, "round-trip must be byte-stable");
}

#[test]
fn round_trip_props_edit_restores_clip() {
    // Full per-clip edit (pitch + time) then its exact inverse.
    let clip = phrase();
    let edited = timepitch::apply_props_to_midi(&clip, 12.0, 2.0).unwrap();
    assert_eq!(edited.length_beats, 2.0);
    let back = timepitch::apply_props_to_midi(&edited, -12.0, 0.5).unwrap();
    for (a, b) in back.notes.iter().zip(clip.notes.iter()) {
        assert_eq!(a.pitch, b.pitch);
        assert!((a.start_beats - b.start_beats).abs() < 1e-9);
        assert!((a.len_beats - b.len_beats).abs() < 1e-9);
    }
    assert!((back.length_beats - clip.length_beats).abs() < 1e-9);
}

#[test]
fn fractional_pitch_rides_bend_not_rounding() {
    let clip = phrase();
    let edited = timepitch::apply_props_to_midi(&clip, 0.3, 1.0).unwrap();
    // Pitches untouched, bend carries the detune.
    assert_eq!(
        edited.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![60, 64, 67]
    );
    assert!(edited.notes.iter().all(|n| (n.pitch_bend - 0.3).abs() < 1e-12));
    // Undo the detune: bend returns to flat.
    let back = timepitch::apply_props_to_midi(&edited, -0.3, 1.0).unwrap();
    assert!(back.notes.iter().all(|n| n.pitch_bend.abs() < 1e-12));
}

#[test]
fn warp_identity_and_small_shift_round_trip() {
    let clip = phrase();
    let id = WarpMap::new(vec![]).unwrap();
    assert!(id.is_identity());
    assert_eq!(timepitch::apply_warp_to_midi(&clip, &id), clip);

    // One +0.1-beat pin over the whole phrase, then its negation: onsets
    // return within a tick (warp moves onsets only; lengths never change).
    let map = WarpMap::new(vec![WarpMarker { at_beats: 0.0, shift_beats: 0.1 }]).unwrap();
    let warped = timepitch::apply_warp_to_midi(&clip, &map);
    assert!(warped.notes.iter().all(|n| (n.start_beats
        - (clip.notes.iter().find(|m| m.note_id == n.note_id).unwrap().start_beats + 0.1))
        .abs()
        < 1e-9));
    assert_eq!(
        warped.notes.iter().map(|n| n.len_beats).collect::<Vec<_>>(),
        clip.notes.iter().map(|n| n.len_beats).collect::<Vec<_>>()
    );
    let back = timepitch::apply_warp_to_midi(&warped, &map.negated());
    for (a, b) in back.notes.iter().zip(clip.notes.iter()) {
        assert!((a.start_beats - b.start_beats).abs() < 1e-9, "{a:?} vs {b:?}");
    }
}

#[test]
fn repair_cleanup_is_stable_and_convergent() {
    // declick + dc-remove on a dirty buffer converges: a second pass finds
    // nothing and the tone underneath survives.
    let mut buf: Vec<f32> = (0..128).map(|i| (i as f32 * 0.3).sin() * 0.2 + 0.05).collect();
    buf[64] = 0.95;
    let first = repair::declick(&mut buf, 0.5, 2, 4.0);
    assert_eq!(first, 1);
    assert_eq!(repair::declick(&mut buf, 0.5, 2, 4.0), 0);
    repair::remove_dc(&mut buf);
    let mean = buf.iter().sum::<f32>() / buf.len() as f32;
    assert!(mean.abs() < 1e-5, "mean={mean}");

    // MIDI tidy: quantize -> dedupe converges, muted notes drop.
    let mut clip = MidiClip::new(4.0);
    let mut a = MidiNote::new(1, 60, 100, 0.02, 2.0);
    a.timing_offset_beats = 0.0;
    clip.add_note(a).unwrap();
    clip.add_note(MidiNote::new(2, 60, 100, 1.0, 1.0)).unwrap();
    let q = repair::quantize_clip(&clip, 0.5).unwrap();
    assert_eq!(q.notes.iter().find(|n| n.note_id == 1).unwrap().start_beats, 0.0);
    let mut q = q;
    assert_eq!(repair::fix_same_pitch_overlaps(&mut q), 1);
    assert_eq!(repair::fix_same_pitch_overlaps(&mut q), 0, "second pass is a no-op");
}
