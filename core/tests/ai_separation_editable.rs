//! Track M (agent 2) validation: separation + cleanup + groove jobs run
//! in the background and yield editable, undoable output.
//!
//! Run: `cargo test --manifest-path core/Cargo.toml --test ai_separation_editable`.

use std::time::Duration;

use ccez_core::ai::{
    ai_actor, SIDECAR_CLEANUP, SIDECAR_GROOVE, SIDECAR_SEPARATION, SeparationDest, STEM_NAMES,
    apply_cleanup_to_engine, apply_groove_to_engine, apply_separation_to_engine,
    submit_cleanup, submit_groove_transfer, submit_separation,
};
use ccez_core::ai::JobStatus;
use ccez_core::bounce::decode_wav;
use ccez_core::engine::{Engine, valid_actor};
use ccez_core::midi::{MidiClip, MidiNote};
use ccez_core::model::{Project, Track};

fn test_project() -> Project {
    let mut p = Project::new("proj_ai_sep", "ai-separation-test");
    for (id, name) in [("trk_stems", "Stems"), ("trk_take", "Take"), ("trk_midi", "Midi")] {
        p.tracks.push(Track {
            id: id.to_string(),
            name: name.to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
    }
    p
}

fn tmp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ccez-ai-sep-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn sine_mix(rate: u32, n: usize) -> Vec<f32> {
    // Bass sine + transient clicks + mid sustained tone: every stem feeder.
    let mut mix = vec![0.0f32; n];
    for i in 0..n {
        let t = i as f64 / rate as f64;
        mix[i] += 0.5 * (2.0 * std::f64::consts::PI * 55.0 * t).sin() as f32;
        mix[i] += 0.3 * (2.0 * std::f64::consts::PI * 440.0 * t).sin() as f32;
    }
    for k in 0..8 {
        let at = k * n / 8;
        mix[at] += 0.9;
    }
    mix
}

#[test]
fn sidecar_actors_satisfy_engine_actor_rule() {
    for sidecar in [SIDECAR_SEPARATION, SIDECAR_CLEANUP, SIDECAR_GROOVE] {
        let actor = ai_actor(sidecar).expect("actor");
        assert!(valid_actor(&actor), "engine rejects {actor}");
    }
}

#[test]
fn separation_job_yields_editable_stems() {
    let rate = 22050;
    let mix = sine_mix(rate, rate as usize);
    let mut job = submit_separation(mix.clone(), rate);
    let sep = job
        .wait(Duration::from_secs(30))
        .flatten()
        .expect("separation done");
    assert_eq!(sep.len(), mix.len());
    assert!(sep.conservation_error(&mix) < 1e-4, "stems must sum to the mix");
    assert_eq!(job.poll(), (JobStatus::Done, 1.0));

    // Editable + undoable: four WAV-backed clips via ordinary ai: ops.
    let dir = tmp_dir("stems");
    let mut engine = Engine::create(&dir, test_project()).expect("create");
    let actor = ai_actor(SIDECAR_SEPARATION).unwrap();
    let dest = SeparationDest {
        track_id: "trk_stems",
        clip_id_prefix: "sep1",
        name_prefix: "Sep",
        start_beats: 0.0,
        length_beats: 4.0,
    };
    let seqs = apply_separation_to_engine(&mut engine, &actor, &sep, &dest).expect("apply");
    assert_eq!(seqs.len(), STEM_NAMES.len());
    assert_eq!(engine.project().clips.len(), 4);
    // Every committed stem decodes behind its key and the mix reconstructs.
    let mut recon = vec![0.0f32; mix.len()];
    for slot in STEM_NAMES {
        let clip = engine
            .project()
            .clips
            .iter()
            .find(|c| c.id == format!("sep1-{slot}"))
            .unwrap_or_else(|| panic!("missing stem clip {slot}"));
        assert_eq!(clip.track_id, "trk_stems");
        let stem = decode_wav(&engine.load_asset(&clip.source).expect("load")).expect("wav");
        assert_eq!(stem.samples.len(), mix.len());
        for (r, s) in recon.iter_mut().zip(&stem.samples) {
            *r += *s;
        }
    }
    // WAV is 16-bit: reconstruction holds to quantization, not float epsilon.
    let err = recon
        .iter()
        .zip(&mix)
        .map(|(r, m)| (r - m).abs())
        .fold(0.0f32, f32::max);
    assert!(err < 1e-4, "wav round-trip reconstruction {err}");
    // Undo removes all four stems (each op undoes independently).
    for _ in 0..4 {
        engine.undo().expect("undo");
    }
    assert!(engine.project().clips.is_empty());
    engine.redo().expect("redo");
    assert_eq!(engine.project().clips.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cleanup_job_yields_editable_take() {
    let rate = 22050;
    let n = rate as usize;
    let mut take = vec![0.02f32; n];
    for i in (n / 4)..(n / 2) {
        let t = i as f64 / rate as f64;
        take[i] = 0.7 * (2.0 * std::f64::consts::PI * 440.0 * t).sin() as f32;
    }
    let mut job = submit_cleanup(take.clone(), rate, 0.1);
    let clean = job
        .wait(Duration::from_secs(30))
        .flatten()
        .expect("cleanup done");
    assert_eq!(clean.samples.len(), n);
    assert!(!clean.gated_regions.is_empty(), "floor must gate somewhere");
    let floor: f32 =
        clean.samples[..n / 8].iter().map(|s| s.abs()).sum::<f32>() / (n / 8) as f32;
    assert!(floor < 0.01, "floor {floor}");

    let dir = tmp_dir("cleanup");
    let mut engine = Engine::create(&dir, test_project()).expect("create");
    let actor = ai_actor(SIDECAR_CLEANUP).unwrap();
    let seq = apply_cleanup_to_engine(
        &mut engine,
        &actor,
        &clean,
        "trk_take",
        "clean1",
        "Clean take",
        0.0,
        4.0,
    )
    .expect("apply");
    assert_eq!(seq, 1);
    assert_eq!(engine.project().clips.len(), 1);
    // The noisy original is a separate concern: cleanup added, not replaced.
    assert_eq!(engine.project().tracks.len(), 3);
    engine.undo().expect("undo");
    assert!(engine.project().clips.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

fn swung_source() -> MidiClip {
    let mut clip = MidiClip::new(2.0);
    for (i, start) in [0.0, 0.5, 1.0, 1.5].iter().enumerate() {
        let mut n = MidiNote::new(i as u32, 60, 100, *start, 0.25);
        if i % 2 == 1 {
            n.timing_offset_beats = 0.06;
        }
        clip.add_note(n).unwrap();
    }
    clip
}

fn straight_target() -> MidiClip {
    let mut clip = MidiClip::new(2.0);
    for (i, start) in [0.0, 0.5, 1.0, 1.5].iter().enumerate() {
        clip.add_note(MidiNote::new(i as u32, 64, 90, *start, 0.25)).unwrap();
    }
    clip
}

#[test]
fn groove_job_yields_editable_midi_clip() {
    let mut job = submit_groove_transfer(swung_source(), straight_target(), 2, 1.0);
    let transferred = job
        .wait(Duration::from_secs(30))
        .flatten()
        .expect("groove done");
    assert!(transferred.notes.iter().any(|n| n.timing_offset_beats.abs() > 0.01));
    for n in &transferred.notes {
        n.validate().expect("transferred note validates");
    }
    // Still editable MIDI: the caller can reshape notes before committing.
    let mut edited = transferred.clone();
    edited.notes[0].velocity = 100;
    assert_eq!(edited.notes[0].velocity, 100);

    let dir = tmp_dir("groove");
    let mut engine = Engine::create(&dir, test_project()).expect("create");
    let actor = ai_actor(SIDECAR_GROOVE).unwrap();
    let seq = apply_groove_to_engine(
        &mut engine,
        &actor,
        &edited,
        "trk_midi",
        "groove1",
        "Groove take",
        0.0,
    )
    .expect("apply");
    assert_eq!(seq, 1);
    // The MIDI asset round-trips behind the clip source; notes stay live.
    let clip = &engine.project().clips[0];
    let back = MidiClip::load_from_engine(&engine, &clip.source).expect("load midi");
    assert_eq!(back, edited);
    engine.undo().expect("undo");
    assert!(engine.project().clips.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn long_separation_job_cancels() {
    // Seconds of audio keep the worker busy long enough to cancel.
    let rate = 44100;
    let mix = sine_mix(rate, (rate * 8) as usize);
    let mut job = submit_separation(mix, rate);
    job.cancel();
    job.join();
    assert_eq!(job.poll().0, JobStatus::Cancelled);
    assert!(job.try_take().is_none());
}
