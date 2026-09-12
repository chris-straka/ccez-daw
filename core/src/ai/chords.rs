//! Deterministic chord-transcription baseline.
//!
//! Template-matches one 12-bin chroma vector per bar against the 24
//! major / minor triads and renders the winners as block-chord pads in a
//! [`MidiClip`]. Labels (`Am`, `C`, …) travel in the plan summary; the clip
//! itself is ordinary editable notes.

use super::job::{Job, JobControl};
use crate::midi::{MidiClip, MidiNote};

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// One recognized bar: root pitch class 0..=11 (C = 0) plus quality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChordLabel {
    pub bar: usize,
    pub root: u8,
    pub minor: bool,
}

impl ChordLabel {
    pub fn name(&self) -> String {
        let base = NOTE_NAMES[(self.root % 12) as usize];
        if self.minor {
            format!("{base}m")
        } else {
            base.to_string()
        }
    }
}

/// Chord output: human labels plus an editable pad clip (root position,
/// one octave around MIDI 48, block chord per bar).
#[derive(Debug, Clone, PartialEq)]
pub struct ChordTranscription {
    pub labels: Vec<ChordLabel>,
    pub clip: MidiClip,
}

/// Score one chroma vector against one triad template: energy on chord
/// tones minus half the energy elsewhere. Simple, deterministic, and good
/// enough for a baseline the neural sidecar must beat.
fn triad_score(chroma: &[f64; 12], root: u8, minor: bool) -> f64 {
    let third = if minor { 3 } else { 4 };
    let tones = [0, third, 7];
    let mut score = 0.0;
    for (pc, &e) in chroma.iter().enumerate() {
        let rel = (pc as u8 + 12 - root) % 12;
        if tones.contains(&rel) {
            score += e;
        } else {
            score -= 0.5 * e;
        }
    }
    score
}

/// Recognize the best triad for one bar's chroma vector.
pub fn recognize_bar(chroma: &[f64; 12], bar: usize) -> Result<ChordLabel, String> {
    if chroma.iter().any(|e| !e.is_finite() || *e < 0.0) {
        return Err(format!("bar {bar}: chroma must be finite and >= 0"));
    }
    if chroma.iter().all(|e| *e == 0.0) {
        return Err(format!("bar {bar}: empty chroma (silent bar)"));
    }
    let mut best: Option<(f64, u8, bool)> = None;
    for root in 0..12u8 {
        for minor in [false, true] {
            let s = triad_score(chroma, root, minor);
            // Deterministic ties: strictly-greater keeps the first (major,
            // lowest root) winner, so output never depends on hash order.
            if best.map(|(b, _, _)| s > b).unwrap_or(true) {
                best = Some((s, root, minor));
            }
        }
    }
    let (_, root, minor) = best.expect("24 candidates always score");
    Ok(ChordLabel { bar, root, minor })
}

/// Transcribe one chroma vector per bar into labels + a pad clip.
/// `beats_per_bar` is usually 4.0 (4/4). Silent bars (all-zero chroma)
/// become rests, keeping bar alignment.
pub fn transcribe_chords(
    chromas: &[[f64; 12]],
    beats_per_bar: f64,
) -> Result<ChordTranscription, String> {
    if !beats_per_bar.is_finite() || beats_per_bar <= 0.0 {
        return Err(format!("beats_per_bar {beats_per_bar} must be > 0"));
    }
    if chromas.is_empty() {
        return Err("need at least one bar of chroma".to_string());
    }
    let length_beats = beats_per_bar * chromas.len() as f64;
    let mut clip = MidiClip::new(length_beats);
    let mut labels = Vec::new();
    let mut note_id = 0u32;
    for (bar, chroma) in chromas.iter().enumerate() {
        if chroma.iter().all(|e| *e == 0.0) {
            continue; // rest bar: no label, no notes, alignment kept
        }
        let label = recognize_bar(chroma, bar)?;
        let third = if label.minor { 3 } else { 4 };
        let start = bar as f64 * beats_per_bar;
        for interval in [0, third, 7] {
            let pitch = 48 + label.root + interval;
            let note = MidiNote::new(
                note_id,
                pitch,
                80,
                start,
                beats_per_bar * 0.95,
            );
            note_id += 1;
            clip.add_note(note)?;
        }
        labels.push(label);
    }
    Ok(ChordTranscription { labels, clip })
}

/// Run [`transcribe_chords`] as a background [`Job`] with progress.
pub fn submit_chords(
    chromas: Vec<[f64; 12]>,
    beats_per_bar: f64,
) -> Job<ChordTranscription> {
    Job::submit(move |ctl: &JobControl| {
        let n = chromas.len().max(1);
        let length_beats = beats_per_bar * chromas.len() as f64;
        let mut clip = MidiClip::new(length_beats.max(f64::EPSILON));
        let mut labels = Vec::new();
        let mut note_id = 0u32;
        for (bar, chroma) in chromas.iter().enumerate() {
            if ctl.is_cancelled() {
                break;
            }
            ctl.set_progress(bar as f64 / n as f64);
            if chroma.iter().all(|e| *e == 0.0) {
                continue;
            }
            let Ok(label) = recognize_bar(chroma, bar) else {
                continue;
            };
            let third = if label.minor { 3 } else { 4 };
            let start = bar as f64 * beats_per_bar;
            for interval in [0, third, 7] {
                let note = MidiNote::new(
                    note_id,
                    48 + label.root + interval,
                    80,
                    start,
                    beats_per_bar * 0.95,
                );
                note_id += 1;
                let _ = clip.add_note(note);
            }
            labels.push(label);
        }
        ChordTranscription { labels, clip }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::model::{Clip, ClipKind, OpKind, Project, Track};
    use std::time::Duration;

    fn chroma_for(tones: &[usize]) -> [f64; 12] {
        let mut c = [0.0; 12];
        for &t in tones {
            c[t % 12] = 1.0;
        }
        c
    }

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ccez-ai-chords-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn chords_job_yields_editable_undoable_output() {
        // C major then A minor.
        let chromas = vec![chroma_for(&[0, 4, 7]), chroma_for(&[9, 0, 4])];
        let mut job = submit_chords(chromas, 4.0);
        let tx = job.wait(Duration::from_secs(10)).expect("chords done");
        assert_eq!(
            tx.labels.iter().map(|l| l.name()).collect::<Vec<_>>(),
            vec!["C", "Am"]
        );
        assert_eq!(tx.clip.notes.len(), 6);

        // Editable: re-voice the pad before committing.
        let mut edited = tx.clip.clone();
        for n in &mut edited.notes {
            n.velocity = 64;
        }
        assert!(edited.notes.iter().all(|n| n.velocity == 64));

        // Undoable through the op log with an ai: sidecar actor.
        let mut project = Project::new("proj_ai", "ai-test");
        project.tracks.push(Track {
            id: "trk_harm".to_string(),
            name: "Harmony".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        let dir = tmp_dir("job");
        let mut engine = Engine::create(&dir, project).expect("create");
        edited.save_to_engine(&mut engine, "take:chords-ai").expect("save");
        let draft = Clip {
            id: "clip_chords_ai".to_string(),
            track_id: "trk_harm".to_string(),
            name: "AI chords (C, Am)".to_string(),
            start_beats: 0.0,
            length_beats: 8.0,
            kind: ClipKind::Midi,
            source: "take:chords-ai".to_string(),
        };
        engine
            .apply(
                "ai:transcribe-chords",
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
    fn chords_rejects_bad_inputs() {
        assert!(transcribe_chords(&[], 4.0).is_err());
        assert!(transcribe_chords(&[[1.0; 12]], 0.0).is_err());
        assert!(recognize_bar(&[0.0; 12], 0).is_err());
        let mut bad = [1.0; 12];
        bad[3] = f64::NAN;
        assert!(recognize_bar(&bad, 0).is_err());
    }
}
