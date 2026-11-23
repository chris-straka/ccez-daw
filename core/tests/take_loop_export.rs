//! End-to-end ACE-Step take loop/stem cutting, gameaudio export contract,
//! loop-seam verification, and malformed-bounds rejection tests (`ccez-daw-3`).

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

use ccez_core::compose::render::PlanStep;
use ccez_core::compose::takes::{
    default_take_plan, export_takes, verify_exported_package, verify_wav_loop_seam, CutRange,
    FadeMargin, TakeAudio, TakeFileStemSpec, TakePackFileSpec, TakePackSpec, TakeRole,
    TakeSectionSpec, TakeStemSpec,
};
use ccez_core::compose::wavread::{self, Audio};
use ccez_core::compose::{LayerDef, SAMPLE_RATE};
use ccez_core::loopseam::DEFAULT_CLICK_THRESHOLD;
use serde_json::Value;

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ccez-take-export-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Synthesize a continuous stereo take with inharmonic frequency ratios so
/// arbitrary frame cuts do NOT naturally fall on zero crossings (proving
/// crossfade/edge-fade closure is genuine).
fn synth_stereo_take(sample_rate: u32, frames: usize, f_l: f64, f_r: f64, amp: f32) -> Audio {
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / sample_rate as f64;
        let l = (TAU * f_l * t + 0.37).sin() * 0.7 + (TAU * (f_l * 1.5) * t + 0.19).cos() * 0.3;
        let r = (TAU * f_r * t + 0.81).sin() * 0.7 + (TAU * (f_r * 1.25) * t + 0.43).cos() * 0.3;
        left.push(l as f32 * amp);
        right.push(r as f32 * amp);
    }
    Audio {
        sample_rate,
        channels: vec![left, right],
        sample_loop: None,
    }
}

/// Synthesize a mono WAV take at any sample rate (e.g. 44.1 kHz).
fn synth_mono_wav(sample_rate: u32, frames: usize, freq: f64, amp: f32) -> Vec<u8> {
    let samples: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            ((TAU * freq * t + 0.51).sin() * amp as f64) as f32
        })
        .collect();
    wavread::encode_pcm16(sample_rate, &[samples])
}

fn multi_stem_fixture() -> TakePackSpec {
    // 120 bpm: 1 beat = 0.5 s = 24_000 frames @ 48_000 Hz.
    // Raw 12-beat stereo takes (with pre-roll and post-roll margin).
    let take_frames_48k = 12 * 24_000;
    let intro_take = synth_stereo_take(SAMPLE_RATE, 6 * 24_000, 196.0, 293.66, 0.55);
    let calm_bed = synth_stereo_take(SAMPLE_RATE, take_frames_48k, 146.83, 220.0, 0.50);
    // 44.1 kHz mono intensity layer WAV bytes (converted to 48 kHz stereo on export).
    let calm_pulse_wav = synth_mono_wav(44_100, 12 * 22_050, 440.0, 0.45);
    // Variation loop with a 1-beat pickup before the loop point (beats 1..5).
    let tense_bed = synth_stereo_take(SAMPLE_RATE, take_frames_48k, 164.81, 246.94, 0.55);
    let tense_lead = synth_stereo_take(SAMPLE_RATE, take_frames_48k, 329.63, 493.88, 0.40);
    let outro_take = synth_stereo_take(SAMPLE_RATE, 6 * 24_000, 146.83, 196.0, 0.50);

    TakePackSpec {
        name: "dungeon_theme".to_string(),
        tempo: 120.0,
        beats_per_bar: 4,
        key: "D minor".to_string(),
        xfade_beats: 1.0,
        layers: vec![
            LayerDef {
                id: "bed".to_string(),
                min_intensity: 1,
                gain_db: 0.0,
            },
            LayerDef {
                id: "pulse".to_string(),
                min_intensity: 3,
                gain_db: -2.0,
            },
            LayerDef {
                id: "lead".to_string(),
                min_intensity: 4,
                gain_db: -1.5,
            },
        ],
        sections: vec![
            TakeSectionSpec {
                id: "intro".to_string(),
                role: TakeRole::Intro,
                next: Some("calm".to_string()),
                length_beats: Some(4.0),
            },
            TakeSectionSpec {
                id: "calm".to_string(),
                role: TakeRole::Loop,
                next: None,
                length_beats: None,
            },
            TakeSectionSpec {
                id: "tense".to_string(),
                role: TakeRole::Loop,
                next: None,
                length_beats: None,
            },
            TakeSectionSpec {
                id: "outro".to_string(),
                role: TakeRole::Outro,
                next: None,
                length_beats: Some(4.0),
            },
        ],
        stems: vec![
            // Intro with 1-beat ring-out tail past its 4-beat handoff.
            TakeStemSpec {
                section: "intro".to_string(),
                layer: "bed".to_string(),
                audio: TakeAudio::Decoded(intro_take),
                cut: CutRange::frames(0, 5 * 24_000),
                fades: FadeMargin::jingle_edges_ms(5.0, 40.0),
                gain_db: 0.0,
                source_label: Some("ACE-Step take 1 intro".to_string()),
            },
            // Calm bed cut from interior [1..9 beats) with 6 ms post-loop crossfade.
            TakeStemSpec {
                section: "calm".to_string(),
                layer: "bed".to_string(),
                audio: TakeAudio::Decoded(calm_bed),
                cut: CutRange::frames(24_000, 9 * 24_000),
                fades: FadeMargin::crossfade_ms(6.0),
                gain_db: 0.0,
                source_label: Some("ACE-Step take 1 calm bed".to_string()),
            },
            // Calm intensity-3 pulse layer imported from 44.1 kHz mono WAV via beat bounds.
            TakeStemSpec {
                section: "calm".to_string(),
                layer: "pulse".to_string(),
                audio: TakeAudio::WavBytes(calm_pulse_wav),
                cut: CutRange::beats(1.0, 9.0),
                fades: FadeMargin::crossfade_ms(6.0),
                gain_db: 0.0,
                source_label: Some("ACE-Step take 1 calm pulse".to_string()),
            },
            // Tense variation with 1-beat pickup [1..2) + 4-beat loop [2..6).
            TakeStemSpec {
                section: "tense".to_string(),
                layer: "bed".to_string(),
                audio: TakeAudio::Decoded(tense_bed),
                cut: CutRange::frames_with_pickup(24_000, 2 * 24_000, 6 * 24_000),
                fades: FadeMargin::crossfade_ms(6.0),
                gain_db: 0.0,
                source_label: Some("ACE-Step take 2 tense bed".to_string()),
            },
            TakeStemSpec {
                section: "tense".to_string(),
                layer: "lead".to_string(),
                audio: TakeAudio::Decoded(tense_lead),
                cut: CutRange::beats_with_pickup(1.0, 2.0, 6.0),
                fades: FadeMargin::crossfade_ms(6.0),
                gain_db: 0.0,
                source_label: Some("ACE-Step take 2 tense lead".to_string()),
            },
            // Outro cue with 50 ms tail fade.
            TakeStemSpec {
                section: "outro".to_string(),
                layer: "bed".to_string(),
                audio: TakeAudio::Decoded(outro_take),
                cut: CutRange::beats(0.0, 5.0),
                fades: FadeMargin::jingle_edges_ms(5.0, 50.0),
                gain_db: 0.0,
                source_label: Some("ACE-Step take 3 outro".to_string()),
            },
        ],
        normalize_peak_dbfs: Some(-1.0),
    }
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn multi_stem_take_export_matches_gameaudio_contract_and_passes_seam_checks() {
    let spec = multi_stem_fixture();
    let plan = default_take_plan(&spec);
    assert!(!plan.is_empty());

    let dir_a = tmp_dir("pkg_a");
    let dir_b = tmp_dir("pkg_b");
    let report = export_takes(&spec, &plan, &dir_a).expect("take package exports cleanly");
    let _report_b = export_takes(&spec, &plan, &dir_b).expect("deterministic re-export");

    assert_eq!(report.stems, 6);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.loop_seams.len(), 4);
    assert!(
        report.loop_seams.iter().all(|s| s.seamless),
        "{:?}",
        report.loop_seams
    );

    // Contract: manifest.json matches gameaudio's ccez_daw_export expectations.
    let manifest = read_json(&dir_a.join("manifest.json"));
    assert_eq!(manifest["format"], "audioforge-music");
    assert_eq!(manifest["format_version"], 1);
    assert_eq!(manifest["score"], "score.json");
    assert_eq!(manifest["mix"], "mix.json");
    assert_eq!(manifest["sample_rate"], SAMPLE_RATE);
    assert_eq!(manifest["channels"], 2);
    assert_eq!(manifest["stems"].as_array().unwrap().len(), 6);

    // Contract: score.json matches af_music::Score schema & validation rules.
    let score = read_json(&dir_a.join("score.json"));
    assert_eq!(score["initial"], "intro");
    assert_eq!(score["bpm"], 120.0);
    assert_eq!(score["beats_per_bar"], 4);
    assert_eq!(score["initial_intensity"], 1);
    assert_eq!(score["sections"]["intro"]["once"], true);
    assert_eq!(score["sections"]["intro"]["next"], "calm");
    assert_eq!(score["sections"]["intro"]["length_beats"], 4.0);
    assert!(score["sections"]["intro"].get("loop_start_beats").is_none());
    assert!(score["sections"]["intro"].get("loop_end_beats").is_none());

    assert_eq!(score["sections"]["calm"]["loop_start_beats"], 0.0);
    assert_eq!(score["sections"]["calm"]["loop_end_beats"], 8.0);
    let calm_layers = score["sections"]["calm"]["layers"].as_array().unwrap();
    assert_eq!(calm_layers.len(), 2);
    assert_eq!(calm_layers[0]["min_intensity"], 1);
    assert_eq!(calm_layers[1]["min_intensity"], 3);

    // Pickup variation: trim_start=1 beat, loop_start=2 beats, loop_end=6 beats
    // => inside trimmed WAV, loop region is [1.0, 5.0) beats!
    assert_eq!(score["sections"]["tense"]["loop_start_beats"], 1.0);
    assert_eq!(score["sections"]["tense"]["loop_end_beats"], 5.0);
    let tense_layers = score["sections"]["tense"]["layers"].as_array().unwrap();
    assert_eq!(tense_layers[1]["min_intensity"], 4);

    assert_eq!(score["sections"]["outro"]["once"], true);
    assert!(score["sections"]["outro"].get("next").is_none());
    assert!(score["sections"]["outro"].get("loop_start_beats").is_none());

    // Contract: mix.json + events/*.json + scenes/preview.json + WAV smpl chunks.
    let mix = read_json(&dir_a.join("mix.json"));
    assert_eq!(mix["sample_rate"], SAMPLE_RATE);
    assert_eq!(mix["score"], "score.json");

    for stem in manifest["stems"].as_array().unwrap() {
        let ev = stem["event"].as_str().unwrap();
        let rel = stem["path"].as_str().unwrap();
        let ev_json = read_json(&dir_a.join(format!("events/{ev}.json")));
        assert_eq!(ev_json["kind"], "loop");
        assert_eq!(ev_json["bus"], "music");
        assert_eq!(ev_json["sources"][0]["wav"], format!("../{rel}"));

        let decoded = wavread::read(&dir_a.join(rel)).expect("exported stem decodes");
        assert_eq!(decoded.sample_rate, SAMPLE_RATE);
        assert_eq!(decoded.channels.len(), 2);
    }

    let calm_pulse = wavread::read(&dir_a.join("stems/calm_pulse.wav")).unwrap();
    assert_eq!(calm_pulse.frames(), 8 * 24_000);
    assert_eq!(calm_pulse.sample_loop, Some((0, 8 * 24_000)));

    let tense_bed = wavread::read(&dir_a.join("stems/tense_bed.wav")).unwrap();
    assert_eq!(tense_bed.frames(), 5 * 24_000);
    assert_eq!(tense_bed.sample_loop, Some((24_000, 5 * 24_000)));

    let outro_bed = wavread::read(&dir_a.join("stems/outro_bed.wav")).unwrap();
    assert_eq!(outro_bed.sample_loop, None);

    // Post-export disk verification checks actual PCM16 WAV loop seams.
    let verified =
        verify_exported_package(&dir_a, DEFAULT_CLICK_THRESHOLD).expect("exported package passes");
    assert_eq!(verified.len(), 4);
    for check in &verified {
        assert!(check.seamless, "{check:?}");
        let step_limit = check.p999_step.max(DEFAULT_CLICK_THRESHOLD);
        assert!(
            check.max_boundary_step <= step_limit,
            "boundary step exceeds signal slew/click threshold: {check:?}"
        );
        assert!(
            check.boundary_rms_step <= step_limit,
            "boundary rms step exceeds signal slew/click threshold: {check:?}"
        );
        assert!(
            check.window_rms_delta <= DEFAULT_CLICK_THRESHOLD,
            "cross-seam window rms delta too large: {check:?}"
        );
    }

    // Determinism: every generated file is byte-identical across runs.
    for rel in &report.files {
        let a = std::fs::read(dir_a.join(rel)).unwrap();
        let b = std::fs::read(dir_b.join(rel)).unwrap();
        assert_eq!(a, b, "non-deterministic export in {rel}");
    }

    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

#[test]
fn exact_length_jingle_cue_exports_sample_exact_without_loop_points() {
    // 120 bpm: 2 beats = 48_000 frames exact.
    let take = synth_stereo_take(SAMPLE_RATE, 96_000, 330.0, 495.0, 0.6);
    let spec = TakePackSpec {
        name: "victory_jingle".to_string(),
        tempo: 120.0,
        beats_per_bar: 4,
        key: "C major".to_string(),
        xfade_beats: 1.0,
        layers: vec![LayerDef {
            id: "fanfare".to_string(),
            min_intensity: 1,
            gain_db: 0.0,
        }],
        sections: vec![TakeSectionSpec {
            id: "jingle".to_string(),
            role: TakeRole::Jingle,
            next: None,
            length_beats: None,
        }],
        stems: vec![TakeStemSpec {
            section: "jingle".to_string(),
            layer: "fanfare".to_string(),
            audio: TakeAudio::Decoded(take),
            cut: CutRange::frames(12_000, 60_000),
            fades: FadeMargin::jingle_edges_ms(5.0, 15.0),
            gain_db: 0.0,
            source_label: Some("ACE-Step jingle".to_string()),
        }],
        normalize_peak_dbfs: Some(-1.0),
    };
    let dir = tmp_dir("jingle");
    let plan = vec![PlanStep {
        section: "jingle".to_string(),
        bars: None,
        intensity: 1,
    }];
    let report = export_takes(&spec, &plan, &dir).expect("jingle exports");
    assert_eq!(report.stems, 1);
    assert!(report.loop_seams.is_empty());

    let score = read_json(&dir.join("score.json"));
    assert_eq!(score["initial"], "jingle");
    assert_eq!(score["sections"]["jingle"]["once"], true);
    assert_eq!(score["sections"]["jingle"]["length_beats"], 2.0);
    assert!(score["sections"]["jingle"].get("next").is_none());
    assert!(score["sections"]["jingle"]
        .get("loop_start_beats")
        .is_none());
    assert!(score["sections"]["jingle"].get("loop_end_beats").is_none());

    let wav = wavread::read(&dir.join("stems/jingle_fanfare.wav")).unwrap();
    assert_eq!(wav.frames(), 48_000);
    assert_eq!(wav.sample_loop, None);
    verify_exported_package(&dir, DEFAULT_CLICK_THRESHOLD).expect("jingle package validates");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn edge_fade_closes_loop_without_margins_and_verifier_catches_raw_click() {
    // Cut full buffer [0..48_000) with no pre/post margin: edge fade closes seam.
    let take = synth_stereo_take(SAMPLE_RATE, 48_000, 227.0, 311.0, 0.6);
    assert!(
        verify_wav_loop_seam("calm", "bed", &take, 0, 48_000, DEFAULT_CLICK_THRESHOLD)
            .expect("valid range")
            .max_boundary_step
            > DEFAULT_CLICK_THRESHOLD,
        "raw inharmonic cut must click before fading"
    );

    let spec = TakePackSpec {
        name: "edge_fade_loop".to_string(),
        tempo: 120.0,
        beats_per_bar: 4,
        key: String::new(),
        xfade_beats: 1.0,
        layers: vec![LayerDef {
            id: "bed".to_string(),
            min_intensity: 1,
            gain_db: 0.0,
        }],
        sections: vec![TakeSectionSpec {
            id: "calm".to_string(),
            role: TakeRole::Loop,
            next: None,
            length_beats: None,
        }],
        stems: vec![TakeStemSpec {
            section: "calm".to_string(),
            layer: "bed".to_string(),
            audio: TakeAudio::Decoded(take),
            cut: CutRange::frames(0, 48_000),
            fades: FadeMargin::crossfade(256),
            gain_db: 0.0,
            source_label: None,
        }],
        normalize_peak_dbfs: Some(-1.0),
    };
    let dir = tmp_dir("edge_fade");
    let report = export_takes(&spec, &default_take_plan(&spec), &dir).expect("exports");
    assert!(report.loop_seams[0].seamless);
    assert!(report.loop_seams[0].max_boundary_step <= DEFAULT_CLICK_THRESHOLD);
    verify_exported_package(&dir, DEFAULT_CLICK_THRESHOLD).expect("package verifies");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_loop_bounds_and_audio_return_clean_errors() {
    let valid_audio = synth_stereo_take(SAMPLE_RATE, 48_000, 220.0, 220.0, 0.5);

    let run_case = |audio: Audio, cut: CutRange, fades: FadeMargin| {
        let spec = TakePackSpec {
            name: "bad_case".to_string(),
            tempo: 120.0,
            beats_per_bar: 4,
            key: String::new(),
            xfade_beats: 1.0,
            layers: vec![LayerDef {
                id: "bed".to_string(),
                min_intensity: 1,
                gain_db: 0.0,
            }],
            sections: vec![TakeSectionSpec {
                id: "calm".to_string(),
                role: TakeRole::Loop,
                next: None,
                length_beats: None,
            }],
            stems: vec![TakeStemSpec {
                section: "calm".to_string(),
                layer: "bed".to_string(),
                audio: TakeAudio::Decoded(audio),
                cut,
                fades,
                gain_db: 0.0,
                source_label: None,
            }],
            normalize_peak_dbfs: None,
        };
        let out = tmp_dir("never");
        let res = export_takes(&spec, &[], &out);
        let _ = std::fs::remove_dir_all(&out);
        res
    };

    // loop_end <= loop_start (equal and reversed, frames and beats)
    assert!(run_case(
        valid_audio.clone(),
        CutRange::frames(1000, 1000),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("loop_end"));
    assert!(run_case(
        valid_audio.clone(),
        CutRange::frames(2000, 1000),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("loop_end"));
    assert!(run_case(
        valid_audio.clone(),
        CutRange::beats(2.0, 2.0),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("loop_end"));
    assert!(run_case(
        valid_audio.clone(),
        CutRange::beats(3.0, 1.0),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("loop_end"));

    // Out-of-range frames and beats
    assert!(run_case(
        valid_audio.clone(),
        CutRange::frames(0, 48_001),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("out of range"));
    assert!(run_case(
        valid_audio.clone(),
        CutRange::frames(48_000, 49_000),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("out of range"));
    assert!(run_case(
        valid_audio.clone(),
        CutRange::beats(0.0, 5.0),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("out of range"));

    // trim_start > loop_start
    assert!(run_case(
        valid_audio.clone(),
        CutRange::frames_with_pickup(2000, 1000, 4000),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("trim_start"));

    // Crossfade exceeding half the loop length
    assert!(run_case(
        valid_audio.clone(),
        CutRange::frames(0, 1000),
        FadeMargin::crossfade(600)
    )
    .unwrap_err()
    .contains("crossfade"));

    // Empty and zero-channel audio
    let zero_ch = Audio {
        sample_rate: SAMPLE_RATE,
        channels: vec![],
        sample_loop: None,
    };
    assert!(run_case(
        zero_ch.clone(),
        CutRange::frames(0, 100),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("zero channels"));
    let empty_frames = Audio {
        sample_rate: SAMPLE_RATE,
        channels: vec![vec![], vec![]],
        sample_loop: None,
    };
    assert!(run_case(
        empty_frames.clone(),
        CutRange::frames(0, 100),
        FadeMargin::default()
    )
    .unwrap_err()
    .contains("empty"));

    // Non-finite audio and zero sample rate
    let nan_audio = Audio {
        sample_rate: SAMPLE_RATE,
        channels: vec![vec![0.0, f32::NAN, 0.0, 0.0]],
        sample_loop: None,
    };
    assert!(
        run_case(nan_audio, CutRange::frames(0, 4), FadeMargin::default())
            .unwrap_err()
            .contains("non-finite")
    );
    let zero_sr = Audio {
        sample_rate: 0,
        channels: vec![vec![0.0; 100]],
        sample_loop: None,
    };
    assert!(
        run_case(zero_sr, CutRange::frames(0, 100), FadeMargin::default())
            .unwrap_err()
            .contains("sample rate")
    );

    // Seam verifier also rejects malformed loop bounds / empty / zero-channel audio
    assert!(
        verify_wav_loop_seam("calm", "bed", &valid_audio, 100, 100, 0.02)
            .unwrap_err()
            .contains("loop_end")
    );
    assert!(
        verify_wav_loop_seam("calm", "bed", &valid_audio, 200, 100, 0.02)
            .unwrap_err()
            .contains("loop_end")
    );
    assert!(
        verify_wav_loop_seam("calm", "bed", &valid_audio, 0, 48_001, 0.02)
            .unwrap_err()
            .contains("out of range")
    );
    assert!(verify_wav_loop_seam("calm", "bed", &zero_ch, 0, 10, 0.02)
        .unwrap_err()
        .contains("zero channels"));
    assert!(
        verify_wav_loop_seam("calm", "bed", &empty_frames, 0, 10, 0.02)
            .unwrap_err()
            .contains("empty")
    );
}

#[test]
fn file_spec_loads_wav_takes_from_disk_and_exports_deterministically() {
    let dir = tmp_dir("file_spec");
    let src_dir = dir.join("takes");
    let out_dir = dir.join("pkg");
    std::fs::create_dir_all(&src_dir).unwrap();

    let take = synth_stereo_take(SAMPLE_RATE, 6 * 24_000, 196.0, 294.0, 0.5);
    let wav_bytes = wavread::encode_pcm16(SAMPLE_RATE, &take.channels);
    std::fs::write(src_dir.join("calm_bed.wav"), &wav_bytes).unwrap();

    let file_spec = TakePackFileSpec {
        name: "disk_take".to_string(),
        tempo: 120.0,
        beats_per_bar: 4,
        key: "G minor".to_string(),
        xfade_beats: 1.0,
        layers: vec![LayerDef {
            id: "bed".to_string(),
            min_intensity: 1,
            gain_db: 0.0,
        }],
        sections: vec![TakeSectionSpec {
            id: "calm".to_string(),
            role: TakeRole::Loop,
            next: None,
            length_beats: None,
        }],
        stems: vec![TakeFileStemSpec {
            section: "calm".to_string(),
            layer: "bed".to_string(),
            wav_path: "calm_bed.wav".to_string(),
            cut: CutRange::beats(0.5, 4.5),
            fades: FadeMargin::crossfade_ms(25.0),
            gain_db: 0.0,
            source_label: Some("ACE-Step disk take".to_string()),
        }],
        normalize_peak_dbfs: Some(-1.0),
    };
    let spec = file_spec.load_from(&src_dir).expect("loads WAV from disk");
    let report = export_takes(&spec, &default_take_plan(&spec), &out_dir).expect("exports");
    assert_eq!(report.stems, 1);
    assert!(report.loop_seams[0].seamless);
    verify_exported_package(&out_dir, DEFAULT_CLICK_THRESHOLD).expect("verifies");
    let _ = std::fs::remove_dir_all(&dir);
}
