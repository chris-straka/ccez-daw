//! Deterministic drum-transcription baseline.
//!
//! The neural sidecar (unpinned model, HTTP) is the ceiling; this module is
//! the floor that always works offline: thresholded onset detection over an
//! energy envelope plus accent-based voice mapping (kick / snare / hat).
//! Output is a [`MidiClip`]: fully editable notes, never frozen audio.

use super::job::{Job, JobControl};
use crate::midi::{MidiClip, MidiNote};

/// General-MIDI drum voices this baseline emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrumVoice {
    Kick,
    Snare,
    Hat,
}

impl DrumVoice {
    /// GM pitch on channel 10 (0-indexed channel 9): 36 / 38 / 42.
    pub fn pitch(self) -> u8 {
        match self {
            Self::Kick => 36,
            Self::Snare => 38,
            Self::Hat => 42,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::Hat => "hat",
        }
    }
}

/// One detected onset: position in beats plus normalized accent 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumOnset {
    pub beat: f64,
    pub energy: f64,
}

impl DrumOnset {
    pub fn new(beat: f64, energy: f64) -> Result<Self, String> {
        if !beat.is_finite() || beat < 0.0 {
            return Err(format!("onset beat {beat} must be finite and >= 0"));
        }
        if !energy.is_finite() || !(0.0..=1.0).contains(&energy) {
            return Err(format!("onset energy {energy} must be in 0..=1"));
        }
        Ok(Self { beat, energy })
    }
}

/// Accent-to-voice rule: strong onsets are kicks, mid are snares, weak are
/// hats. Downbeats (integer multiples of 4) bias one step stronger so a
/// four-on-the-floor pulse reads as kicks even at mid energy.
pub fn voice_for(onset: &DrumOnset) -> DrumVoice {
    let downbeat = (onset.beat % 4.0).abs() < 1e-9;
    let e = if downbeat {
        (onset.energy + 0.34).min(1.0)
    } else {
        onset.energy
    };
    if e >= 0.66 {
        DrumVoice::Kick
    } else if e >= 0.33 {
        DrumVoice::Snare
    } else {
        DrumVoice::Hat
    }
}

/// Quantize to the 16th-note grid (0.25 beats).
pub fn quantize_16th(beat: f64) -> f64 {
    (beat * 4.0).round() / 4.0
}

/// Build an editable [`MidiClip`] from onsets. Notes are 16th-note hits
/// (length 0.25) on the drum channel with velocity from accent.
pub fn drums_from_onsets(
    onsets: &[DrumOnset],
    length_beats: f64,
) -> Result<MidiClip, String> {
    if !length_beats.is_finite() || length_beats <= 0.0 {
        return Err(format!("length_beats {length_beats} must be > 0"));
    }
    let mut clip = MidiClip::new(length_beats);
    for (i, onset) in onsets.iter().enumerate() {
        if onset.beat > length_beats {
            return Err(format!(
                "onset beat {} past clip length {length_beats}",
                onset.beat
            ));
        }
        let voice = voice_for(onset);
        let velocity = (onset.energy * 126.0).round() as u8 + 1;
        let mut note = MidiNote::new(
            i as u32,
            voice.pitch(),
            velocity,
            quantize_16th(onset.beat),
            0.25,
        );
        note.channel = 9;
        note.velocity = velocity.clamp(1, 127);
        clip.add_note(note)?;
    }
    Ok(clip)
}

/// Peak-pick onsets from an energy envelope sampled at `frames_per_beat`.
/// A frame starts an onset when it crosses `threshold` upward (one-frame
/// refractory, so a plateau yields one onset, not one per frame).
pub fn detect_drums(
    frames: &[f64],
    frames_per_beat: f64,
    threshold: f64,
    length_beats: f64,
) -> Result<MidiClip, String> {
    if !frames_per_beat.is_finite() || frames_per_beat <= 0.0 {
        return Err(format!("frames_per_beat {frames_per_beat} must be > 0"));
    }
    if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
        return Err(format!("threshold {threshold} must be in 0..=1"));
    }
    let mut onsets = Vec::new();
    let mut armed = true;
    for (i, &energy) in frames.iter().enumerate() {
        if !energy.is_finite() || !(0.0..=1.0).contains(&energy) {
            return Err(format!("frame {i} energy {energy} must be in 0..=1"));
        }
        if armed && energy >= threshold {
            onsets.push(DrumOnset {
                beat: i as f64 / frames_per_beat,
                energy,
            });
            armed = false;
        } else if energy < threshold {
            armed = true;
        }
    }
    drums_from_onsets(&onsets, length_beats)
}

/// Run [`detect_drums`] as a background [`Job`], reporting progress per
/// chunk of frames. Cancel-aware: returns the partial-envelope clip.
pub fn submit_drums(
    frames: Vec<f64>,
    frames_per_beat: f64,
    threshold: f64,
    length_beats: f64,
) -> Job<MidiClip> {
    Job::submit(move |ctl: &JobControl| {
        let n = frames.len().max(1);
        let mut onsets = Vec::new();
        let mut armed = true;
        for (i, &energy) in frames.iter().enumerate() {
            if ctl.is_cancelled() {
                break;
            }
            if !(0.0..=1.0).contains(&energy) || !energy.is_finite() {
                continue;
            }
            if armed && energy >= threshold {
                onsets.push(DrumOnset {
                    beat: i as f64 / frames_per_beat,
                    energy,
                });
                armed = false;
            } else if energy < threshold {
                armed = true;
            }
            if i % 64 == 0 {
                ctl.set_progress(i as f64 / n as f64);
            }
        }
        drums_from_onsets(&onsets, length_beats).unwrap_or_else(|_| MidiClip::new(length_beats))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::model::{Clip, ClipKind, OpKind, Project, Track};
    use std::time::Duration;

    fn test_project() -> Project {
        let mut p = Project::new("proj_ai", "ai-test");
        p.tracks.push(Track {
            id: "trk_drums".to_string(),
            name: "Drums".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        p
    }

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ccez-ai-drums-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn drums_job_yields_editable_undoable_output() {
        // Four-on-the-floor energy pulse: strong frames on beats 0..3.
        let frames = vec![
            0.9, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1,
        ];
        let mut job = submit_drums(frames, 4.0, 0.5, 4.0);
        let clip = job.wait(Duration::from_secs(10)).expect("drums done");
        assert_eq!(clip.notes.len(), 4);
        assert!(clip.notes.iter().all(|n| n.pitch == 36 && n.channel == 9));

        // Editable: the caller can reshape notes before committing.
        let mut edited = clip.clone();
        edited.notes[0].velocity = 100;
        assert_eq!(edited.notes[0].velocity, 100);

        // Undoable: store the MIDI asset, apply as an ai: sidecar op, undo.
        let dir = tmp_dir("job");
        let mut engine = Engine::create(&dir, test_project()).expect("create");
        edited.save_to_engine(&mut engine, "take:drums-ai").expect("save");
        let draft = Clip {
            id: "clip_drums_ai".to_string(),
            track_id: "trk_drums".to_string(),
            name: "AI drums".to_string(),
            start_beats: 0.0,
            length_beats: 4.0,
            kind: ClipKind::Midi,
            source: "take:drums-ai".to_string(),
        };
        let seq = engine
            .apply(
                "ai:transcribe-drums",
                OpKind::ClipAdded,
                &draft.id,
                &serde_json::to_string(&draft).unwrap(),
            )
            .expect("apply");
        assert_eq!(seq, 1);
        assert!(engine.project().clips.iter().any(|c| c.id == "clip_drums_ai"));
        engine.undo().expect("undo");
        assert!(engine.project().clips.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drums_rejects_bad_inputs() {
        assert!(DrumOnset::new(-1.0, 0.5).is_err());
        assert!(DrumOnset::new(0.0, 2.0).is_err());
        assert!(drums_from_onsets(&[], 0.0).is_err());
        assert!(detect_drums(&[0.5], 0.0, 0.5, 4.0).is_err());
        assert!(detect_drums(&[2.0], 1.0, 0.5, 4.0).is_err());
    }
}
