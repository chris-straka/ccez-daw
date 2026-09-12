//! Deterministic melody-transcription baseline.
//!
//! Segments a monophonic pitch track (one optional MIDI pitch per frame)
//! into sustained notes: a run of equal pitches becomes one note, silence
//! splits phrases. Output is a [`MidiClip`] with plain editable notes.

use super::job::{Job, JobControl};
use crate::midi::{MidiClip, MidiNote};

/// One pitch-track frame: the estimated MIDI pitch at `beat`, or `None`
/// for unvoiced / silent frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchFrame {
    pub beat: f64,
    pub midi: Option<u8>,
}

impl PitchFrame {
    pub fn voiced(beat: f64, midi: u8) -> Result<Self, String> {
        if !beat.is_finite() || beat < 0.0 {
            return Err(format!("pitch frame beat {beat} must be finite and >= 0"));
        }
        if midi > 127 {
            return Err(format!("pitch {midi} out of range 0..=127"));
        }
        Ok(Self {
            beat,
            midi: Some(midi),
        })
    }

    pub fn silent(beat: f64) -> Result<Self, String> {
        if !beat.is_finite() || beat < 0.0 {
            return Err(format!("pitch frame beat {beat} must be finite and >= 0"));
        }
        Ok(Self { beat, midi: None })
    }
}

/// Segment `frames` (ascending beats) into sustained notes. A run of the
/// same pitch becomes one note spanning the run; `None` frames split runs.
/// Notes shorter than `min_len_beats` are dropped as jitter. Velocity is a
/// flat 96 — the sidecar estimates expression, the baseline stays honest.
pub fn transcribe_melody(
    frames: &[PitchFrame],
    min_len_beats: f64,
    length_beats: f64,
) -> Result<MidiClip, String> {
    if !length_beats.is_finite() || length_beats <= 0.0 {
        return Err(format!("length_beats {length_beats} must be > 0"));
    }
    if !min_len_beats.is_finite() || min_len_beats < 0.0 {
        return Err(format!("min_len_beats {min_len_beats} must be >= 0"));
    }
    let mut clip = MidiClip::new(length_beats);
    let mut run: Option<(u8, f64, usize)> = None; // (pitch, start, index)
    let mut note_id = 0u32;
    let mut flush = |pitch: u8, start: f64, end: f64, clip: &mut MidiClip| {
        if end - start >= min_len_beats && end > start {
            let note = MidiNote::new(note_id, pitch, 96, start, end - start);
            note_id += 1;
            let _ = clip.add_note(note);
        }
    };
    for (i, f) in frames.iter().enumerate() {
        if !f.beat.is_finite() || f.beat < 0.0 {
            return Err(format!("frame {i} beat {} invalid", f.beat));
        }
        if i > 0 && f.beat < frames[i - 1].beat {
            return Err(format!("frame {i} beat {} out of order", f.beat));
        }
        if f.beat > length_beats {
            return Err(format!(
                "frame {i} beat {} past clip length {length_beats}",
                f.beat
            ));
        }
        match (run, f.midi) {
            (Some((p, s, _)), Some(q)) if p == q => {
                run = Some((p, s, i));
            }
            (Some((p, s, _)), _) => {
                flush(p, s, f.beat, &mut clip);
                run = f.midi.map(|q| (q, f.beat, i));
            }
            (None, Some(q)) => run = Some((q, f.beat, i)),
            (None, None) => {}
        }
    }
    if let Some((p, s, _)) = run {
        let end = frames.last().map(|f| f.beat).unwrap_or(0.0);
        flush(p, s, end.max(s + f64::EPSILON), &mut clip);
    }
    Ok(clip)
}

/// Run [`transcribe_melody`] as a background [`Job`] with progress.
pub fn submit_melody(
    frames: Vec<PitchFrame>,
    min_len_beats: f64,
    length_beats: f64,
) -> Job<MidiClip> {
    Job::submit(move |ctl: &JobControl| {
        let n = frames.len().max(1);
        let mut clip = MidiClip::new(length_beats);
        let mut run: Option<(u8, f64)> = None;
        let mut note_id = 0u32;
        for (i, f) in frames.iter().enumerate() {
            if ctl.is_cancelled() {
                break;
            }
            if i % 64 == 0 {
                ctl.set_progress(i as f64 / n as f64);
            }
            let Some(q) = f.midi else {
                if let Some((p, s)) = run.take() {
                    if f.beat - s >= min_len_beats && f.beat > s {
                        let note = MidiNote::new(note_id, p, 96, s, f.beat - s);
                        note_id += 1;
                        let _ = clip.add_note(note);
                    }
                }
                continue;
            };
            match run {
                Some((p, _)) if p == q => {}
                Some((p, s)) => {
                    if f.beat - s >= min_len_beats && f.beat > s {
                        let note = MidiNote::new(note_id, p, 96, s, f.beat - s);
                        note_id += 1;
                        let _ = clip.add_note(note);
                    }
                    run = Some((q, f.beat));
                }
                None => run = Some((q, f.beat)),
            }
        }
        if let Some((p, s)) = run {
            if !ctl.is_cancelled() {
                let end = frames.last().map(|f| f.beat).unwrap_or(s);
                if end - s >= min_len_beats && end > s {
                    let note = MidiNote::new(note_id, p, 96, s, end - s);
                    let _ = clip.add_note(note);
                }
            }
        }
        clip
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::model::{Clip, ClipKind, OpKind, Project, Track};
    use std::time::Duration;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ccez-ai-melody-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn melody_job_yields_editable_undoable_output() {
        // C4 held 0..2, rest, E4 held 2.5..4.
        let frames = vec![
            PitchFrame::voiced(0.0, 60).unwrap(),
            PitchFrame::voiced(0.5, 60).unwrap(),
            PitchFrame::voiced(1.0, 60).unwrap(),
            PitchFrame::voiced(1.5, 60).unwrap(),
            PitchFrame::silent(2.0).unwrap(),
            PitchFrame::voiced(2.5, 64).unwrap(),
            PitchFrame::voiced(3.0, 64).unwrap(),
            PitchFrame::voiced(3.5, 64).unwrap(),
        ];
        let mut job = submit_melody(frames, 0.25, 4.0);
        let clip = job.wait(Duration::from_secs(10)).expect("melody done");
        assert_eq!(clip.notes.len(), 2);
        assert_eq!(clip.notes[0].pitch, 60);
        assert_eq!(clip.notes[1].pitch, 64);

        // Editable: transpose before committing.
        let mut edited = clip.clone();
        for n in &mut edited.notes {
            n.pitch += 2;
        }
        assert_eq!(edited.notes[0].pitch, 62);

        // Undoable through the op log with an ai: sidecar actor.
        let mut project = Project::new("proj_ai", "ai-test");
        project.tracks.push(Track {
            id: "trk_mel".to_string(),
            name: "Melody".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        let dir = tmp_dir("job");
        let mut engine = Engine::create(&dir, project).expect("create");
        edited.save_to_engine(&mut engine, "take:melody-ai").expect("save");
        let draft = Clip {
            id: "clip_mel_ai".to_string(),
            track_id: "trk_mel".to_string(),
            name: "AI melody".to_string(),
            start_beats: 0.0,
            length_beats: 4.0,
            kind: ClipKind::Midi,
            source: "take:melody-ai".to_string(),
        };
        engine
            .apply(
                "ai:transcribe-melody",
                OpKind::ClipAdded,
                &draft.id,
                &serde_json::to_string(&draft).unwrap(),
            )
            .expect("apply");
        assert_eq!(engine.project().clips.len(), 1);
        engine.undo().expect("undo");
        assert!(engine.project().clips.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn melody_rejects_bad_inputs() {
        assert!(transcribe_melody(&[], 0.25, 0.0).is_err());
        assert!(PitchFrame::voiced(0.0, 200).is_err());
        let bad = vec![
            PitchFrame::voiced(1.0, 60).unwrap(),
            PitchFrame::voiced(0.0, 60).unwrap(),
        ];
        assert!(transcribe_melody(&bad, 0.25, 4.0).is_err());
    }
}
