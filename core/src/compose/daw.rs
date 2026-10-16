//! Open a composition in the DAW: a project bundle with the sections laid
//! out on the timeline.
//!
//! Teaching note: the DAW's MIDI clips have no sampled instruments behind
//! them (the engine renders MIDI procedurally), so a composition becomes
//! two kinds of track:
//!
//! - one **audio** track per layer carrying the composer's rendered stems
//!   (`asset:` clips, mono 48 kHz), so pressing play sounds the real
//!   orchestration, exactly what export ships;
//! - one **MIDI** track per (layer, instrument) holding the notes as
//!   piano-roll clips, muted so they don't add a procedural tone. They
//!   are there to read and edit; `composition.json` (also stored in the
//!   bundle) stays the source the composer renders.
//!
//! Sections play in document order (intro, loops, outro), one pass each.

use std::path::Path;

use serde::Serialize;

use super::render::{render_stems, Rendered};
use super::{Composition, SAMPLE_RATE};
use crate::bounce::{encode_wav, Stem};
use crate::engine::Engine;
use crate::midi::{MidiClip, MidiNote};
use crate::model::{Clip, ClipKind, Project, Track};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DawReport {
    pub dir: String,
    pub tracks: usize,
    pub clips: usize,
    pub audio_tracks: usize,
    pub midi_tracks: usize,
    pub beats: f32,
    /// Plain-language problems (e.g. the render failed: MIDI only).
    pub warnings: Vec<String>,
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Section start beats along the timeline, in document order.
pub fn timeline(c: &Composition) -> Vec<(String, f32, f32)> {
    let mut at = 0.0f32;
    c.sections
        .iter()
        .map(|s| {
            let len = c.section_beats(s);
            let row = (s.id.clone(), at, len);
            at += len;
            row
        })
        .collect()
}

/// Build the project (and its assets) for `c`. `rendered` = stems to put
/// on the audio tracks (None: MIDI only).
pub fn build(c: &Composition, rendered: Option<&Rendered>) -> (Project, Vec<(String, &'static str, Vec<u8>)>) {
    let mut p = Project::new(&format!("compose_{}", c.name), &c.name);
    p.tempo = c.tempo as f64;
    p.time_sig_num = c.beats_per_bar.min(255) as u8;
    p.time_sig_den = 4;
    let mut assets: Vec<(String, &'static str, Vec<u8>)> = Vec::new();
    let places = timeline(c);
    let at = |sec: &str| places.iter().find(|x| x.0 == sec).map(|x| (x.1, x.2));
    if let Some(r) = rendered {
        for l in &c.layers {
            let stems: Vec<_> = r.stems.iter().filter(|s| s.layer == l.id).collect();
            if stems.is_empty() {
                continue;
            }
            let tid = format!("trk_audio_{}", slug(&l.id));
            let mut track = Track {
                id: tid.clone(),
                name: format!("{} (rendered, intensity {}+)", l.id, l.min_intensity),
                volume: 0.8,
                pan: 0.0,
                muted: false,
                solo: false,
                clip_ids: vec![],
                device_ids: vec![],
            };
            for st in stems {
                let Some((start, len)) = at(&st.section) else { continue };
                let key = format!("stem_{}_{}.wav", slug(&st.section), slug(&l.id));
                let n = st.length_frames.min(st.frames());
                let mono: Vec<f32> = (0..n).map(|i| 0.5 * (st.left[i] + st.right[i])).collect();
                let stem = Stem { name: key.clone(), sample_rate: SAMPLE_RATE, samples: mono, loop_start: 0, loop_end: n as u32 };
                assets.push((key.clone(), crate::bounce::FROZEN_STEM_KIND, encode_wav(&stem)));
                let cid = format!("clip_{}_{}_audio", slug(&st.section), slug(&l.id));
                track.clip_ids.push(cid.clone());
                p.clips.push(Clip {
                    id: cid,
                    track_id: tid.clone(),
                    name: format!("{} · {}", st.section, l.id),
                    start_beats: start as f64,
                    length_beats: len as f64,
                    kind: ClipKind::Audio,
                    source: format!("asset:{key}"),
                });
            }
            p.tracks.push(track);
        }
    }
    for l in &c.layers {
        for inst in &c.instruments {
            let parts: Vec<_> = c.parts.iter().filter(|x| x.layer == l.id && x.instrument == inst.id && !x.notes.is_empty()).collect();
            if parts.is_empty() {
                continue;
            }
            let tid = format!("trk_midi_{}_{}", slug(&l.id), slug(&inst.id));
            let mut track = Track {
                id: tid.clone(),
                name: format!("{} · {} ({})", l.id, inst.id, inst.patch),
                volume: 0.8,
                pan: inst.pan.unwrap_or(0.0) as f64,
                muted: true,
                solo: false,
                clip_ids: vec![],
                device_ids: vec![],
            };
            for part in parts {
                let Some((start, len)) = at(&part.section) else { continue };
                let mut clip = MidiClip::new(len as f64);
                for (i, n) in part.notes.iter().enumerate() {
                    let v = (n.velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                    let _ = clip.add_note(MidiNote::new(i as u32 + 1, n.pitch, v, n.start as f64, n.length.max(1.0 / 64.0) as f64));
                }
                let key = format!("midi_{}_{}_{}.json", slug(&part.section), slug(&l.id), slug(&inst.id));
                assets.push((key.clone(), "midi", clip.encode().unwrap_or_default()));
                let cid = format!("clip_{}_{}_{}", slug(&part.section), slug(&l.id), slug(&inst.id));
                track.clip_ids.push(cid.clone());
                p.clips.push(Clip {
                    id: cid,
                    track_id: tid.clone(),
                    name: format!("{} · {}", part.section, inst.id),
                    start_beats: start as f64,
                    length_beats: len as f64,
                    kind: ClipKind::Midi,
                    source: key,
                });
            }
            p.tracks.push(track);
        }
    }
    let doc = serde_json::to_vec_pretty(c).unwrap_or_default();
    assets.push(("composition.json".into(), "composition", doc));
    (p, assets)
}

/// Write `c` as a DAW project bundle at `dir` (must not exist), rendering
/// its stems for the audio tracks. A failed render (e.g. samples can't be
/// downloaded) still writes the MIDI tracks and says why there is no audio.
pub fn write_project(c: &Composition, dir: &Path) -> Result<DawReport, String> {
    if dir.exists() {
        return Err(format!("{}: already exists (pick a new folder)", dir.display()));
    }
    let errors = c.validate();
    if !errors.is_empty() {
        return Err(format!("composition is not renderable:\n- {}", errors.join("\n- ")));
    }
    let mut warnings = Vec::new();
    let rendered = match render_stems(c) {
        Ok(r) => Some(r),
        Err(e) => {
            warnings.push(format!("no rendered audio ({e}); the MIDI tracks are there to edit, unmute them to hear the DAW's own tone"));
            None
        }
    };
    let (project, assets) = build(c, rendered.as_ref());
    let mut engine = Engine::create(dir, project.clone()).map_err(|e| e.to_string())?;
    for (key, kind, bytes) in &assets {
        engine.store_asset(key, kind, bytes).map_err(|e| format!("{key}: {e}"))?;
    }
    let audio_tracks = project.tracks.iter().filter(|t| t.id.starts_with("trk_audio_")).count();
    Ok(DawReport {
        dir: dir.display().to_string(),
        tracks: project.tracks.len(),
        clips: project.clips.len(),
        audio_tracks,
        midi_tracks: project.tracks.len() - audio_tracks,
        beats: timeline(c).last().map_or(0.0, |x| x.1 + x.2),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::tiny;
    use super::*;

    #[test]
    fn a_composition_opens_as_a_project_that_sounds_and_edits() {
        let dir = std::env::temp_dir().join(format!("ccez-daw-proj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let c = tiny();
        let rep = write_project(&c, &dir).unwrap();
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        // bed + pulse audio; bed/keys + pulse/tick MIDI.
        assert_eq!((rep.audio_tracks, rep.midi_tracks), (2, 2));
        assert_eq!(rep.beats, 16.0);
        let engine = Engine::open(&dir).unwrap();
        let p = engine.project();
        assert_eq!(p.tempo, 120.0);
        // Sections laid end to end: intro 0, calm 4, end 12.
        let calm = p.clips.iter().find(|x| x.id == "clip_calm_bed_audio").unwrap();
        assert_eq!((calm.start_beats, calm.length_beats), (4.0, 8.0));
        // The piano roll's clip decodes with the notes.
        let midi = p.clips.iter().find(|x| x.id == "clip_calm_bed_keys").unwrap();
        let mc = MidiClip::decode(&engine.load_asset(&midi.source).unwrap()).unwrap();
        assert_eq!(mc.notes.len(), 2);
        assert_eq!(mc.notes[0].pitch, 62);
        assert!(p.tracks.iter().filter(|t| t.id.starts_with("trk_midi")).all(|t| t.muted));
        // Play renders the stems: a pumped live loop is not silent.
        let bank = engine.sample_bank();
        let (graph, _) = crate::audio::live_graph(p, 48_000, &bank).expect("audible");
        assert!(matches!(graph.proc_of("mix"), crate::audio::render::Proc::Loop { buf } if buf.iter().any(|s| s.abs() > 1e-3)));
        assert!(write_project(&c, &dir).unwrap_err().contains("already exists"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
