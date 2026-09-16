//! Warp + loop auto-fit validation: resample factor, click-threshold
//! conformance, and warp marker-map round-trips.
//!
//! Run: `cargo test --manifest-path core/Cargo.toml --test loop_autofit_warp_roundtrip`.

use ccez_core::loopseam::{self, DEFAULT_CLICK_THRESHOLD, LoopSeamOptions};
use ccez_core::timepitch::{self, WarpMap, WarpMarker};

fn clean_loop(n: usize, cycles: f64) -> Vec<f32> {
    let f = (n / 64).max(1);
    let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
    (0..n)
        .map(|t| {
            let s = (0.5 * (2.0 * std::f64::consts::PI * cycles * t as f64 / n as f64).sin()) as f32;
            let g = if t < f {
                smooth(t as f32 / f as f32)
            } else if t >= n - f {
                smooth((n - 1 - t) as f32 / f as f32)
            } else {
                1.0
            };
            s * g
        })
        .collect()
}

#[test]
fn midi_and_audio_paths_agree_on_the_fit_factor() {
    // One ratio drives both paths: MIDI scales beats, audio scales frames.
    let ratio = timepitch::loop_fit_ratio(120.0, 140.0).unwrap();
    let clip = timepitch::sample_phrase();
    let fitted = timepitch::fit_midi_loop_to_tempo(&clip, 120.0, 140.0).unwrap();
    assert!((fitted.length_beats - clip.length_beats / ratio).abs() < 1e-9);
    let audio = clean_loop(8000, 220.0);
    let (out, fit) =
        loopseam::auto_fit_loop_to_tempo(&audio, 120.0, 140.0, DEFAULT_CLICK_THRESHOLD).unwrap();
    assert!((fit.ratio - ratio).abs() < 1e-12);
    assert_eq!(out.len(), timepitch::resampled_len(audio.len(), ratio).unwrap());
}

#[test]
fn auto_fit_output_conforms_to_the_click_threshold() {
    // Clean loop at unity gain: no conform needed, seam quiet.
    let clean = clean_loop(8000, 220.0);
    let (out, fit) =
        loopseam::auto_fit_loop_to_tempo(&clean, 100.0, 100.0, DEFAULT_CLICK_THRESHOLD).unwrap();
    assert!(!fit.conformed);
    assert!(loopseam::check_stem("stems/a.wav", &out, &LoopSeamOptions::default()).is_none());

    // Clicked loop fitted across tempos: conform closes the seam so the
    // ship-gate sees no click.
    let mut clicked = clean.clone();
    clicked[8000 - 1] = clicked[0] + 0.3;
    let (out, fit) =
        loopseam::auto_fit_loop_to_tempo(&clicked, 120.0, 96.0, DEFAULT_CLICK_THRESHOLD).unwrap();
    assert!(fit.conformed);
    assert!(!fit.seam.is_click());
    assert!(loopseam::check_stem("stems/a.wav", &out, &LoopSeamOptions::default()).is_none());
}

#[test]
fn warp_marker_map_round_trips_through_json_and_negation() {
    let clip = timepitch::sample_phrase();
    let map = WarpMap::new(vec![
        WarpMarker { at_beats: 0.0, shift_beats: 0.1 },
        WarpMarker { at_beats: 2.0, shift_beats: -0.2 },
    ])
    .unwrap();
    // JSON round-trip is exact (sorted order preserved).
    let back = WarpMap::from_json(&map.to_json()).unwrap();
    assert_eq!(back, map);
    assert_eq!(
        timepitch::apply_warp_to_midi(&clip, &back),
        timepitch::apply_warp_to_midi(&clip, &map)
    );
    // Warp then negate restores onsets for a small uniform shift (the
    // documented exact case: shifts small relative to marker spacing);
    // lengths never change either way.
    let small = WarpMap::new(vec![WarpMarker { at_beats: 0.0, shift_beats: 0.1 }]).unwrap();
    let warped = timepitch::apply_warp_to_midi(&clip, &small);
    let restored = timepitch::apply_warp_to_midi(&warped, &small.negated());
    for (a, b) in restored.notes.iter().zip(clip.notes.iter()) {
        assert!((a.start_beats - b.start_beats).abs() < 1e-9, "{a:?} vs {b:?}");
        assert!((a.len_beats - b.len_beats).abs() < 1e-12);
    }
    assert_eq!(
        warped.notes.iter().map(|n| n.len_beats).collect::<Vec<_>>(),
        clip.notes.iter().map(|n| n.len_beats).collect::<Vec<_>>()
    );
    // Empty map is the identity, including through JSON.
    let id = WarpMap::new(vec![]).unwrap();
    assert!(id.is_identity());
    let id_back = WarpMap::from_json(&id.to_json()).unwrap();
    assert!(id_back.is_identity());
    assert_eq!(timepitch::apply_warp_to_midi(&clip, &id_back), clip);
}
