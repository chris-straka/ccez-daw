//! Live loop playback: what the transport actually sounds.
//!
//! The streaming backends render [`RenderGraph`]s whose nodes all default
//! to [`Proc::Null`] — pushing bare topology plays digital silence. This
//! module closes that gap with the smallest honest sound: it pre-renders
//! the project's mix loop once (the same [`render_mix`] the bounce/export
//! path uses, honoring mute/solo/volume/device gain) and parks it as a
//! [`Proc::Loop`] on the `mix` sink. Edits re-push (the shell calls
//! `push_graph` on every applied op), so the loop always matches the
//! project; playback position comes from the backends' running frame
//! counters, never from this buffer.
//!
//! Limits (v1, not hidden): the loop is a fixed render at push-time tempo
//! (a tempo change re-renders from the loop start), it spans the longest
//! clip end clamped to [`MAX_LIVE_LOOP_BEATS`], and note-level MIDI detail
//! is whatever `render_mix` renders procedurally — exactly what export
//! hears, which is the point: live and bounce agree by construction.

use std::collections::BTreeMap;

use crate::bounce::{render_mix_with_bank, BounceConfig, SampleBank};
use crate::model::Project;

use super::graph::AudioGraph;
use super::render::{Proc, RenderGraph};

/// Longest live loop, in beats (bounds the push-time render).
pub const MAX_LIVE_LOOP_BEATS: f64 = 256.0;

/// Shortest live loop, in beats (a click-less loop needs a whole beat).
pub const MIN_LIVE_LOOP_BEATS: f64 = 1.0;

/// Loop length for `project`: longest clip end, clamped. `None` when the
/// project holds no clips (nothing to sound — the shell pushes bare
/// topology and stays silent, as before).
pub fn loop_beats(project: &Project) -> Option<f64> {
    let end = project
        .clips
        .iter()
        .map(|c| c.start_beats + c.length_beats)
        .fold(0.0f64, f64::max);
    if !(end > 0.0) || !end.is_finite() {
        return None;
    }
    Some(end.clamp(MIN_LIVE_LOOP_BEATS, MAX_LIVE_LOOP_BEATS))
}

/// Build the live graph for `project` at `sample_rate`: topology plus the
/// pre-rendered mix loop on `mix`. Returns `None` when there is nothing to
/// sound (no clips) or the render fails — the caller falls back to the
/// bare-topology push.
pub fn live_graph(
    project: &Project,
    sample_rate: u32,
    bank: &SampleBank,
) -> Option<(RenderGraph, BTreeMap<(String, String), u64>)> {
    let beats = loop_beats(project)?;
    let config = BounceConfig::new(sample_rate, 0.0, beats).ok()?;
    let stem = render_mix_with_bank(project, &config, bank).ok()?;
    if stem.samples.iter().all(|&s| s == 0.0) {
        return None;
    }
    let mut topo = AudioGraph::from_project(project);
    topo.ensure_node("mix");
    let delays = topo.all_edge_delays().ok()?;
    let mut graph = RenderGraph::from_audio_graph(topo);
    graph.set_proc("mix", Proc::Loop { buf: stem.samples });
    Some((graph, delays))
}

#[cfg(test)]
mod tests {
    use super::super::device::{AudioBackend, AudioCommand, AudioEngine, NullBackend};
    use super::*;
    use crate::model::{Clip, ClipKind, Project, Track};

    fn sounding_project() -> Project {
        let mut p = Project::new("p", "Live");
        p.tracks.push(Track {
            id: "trk".to_string(),
            name: "Trk".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        p.clips.push(Clip {
            id: "clip".to_string(),
            name: "Clip".to_string(),
            track_id: "trk".to_string(),
            start_beats: 0.0,
            length_beats: 4.0,
            kind: ClipKind::Midi,
            source: "take:clip".to_string(),
        });
        p
    }

    #[test]
    fn live_graph_loops_a_sounding_mix() {
        let p = sounding_project();
        let (graph, _) = live_graph(&p, 44100, &SampleBank::empty()).expect("MIDI clip must sound");
        assert!(
            matches!(graph.proc_of("mix"), Proc::Loop { buf } if buf.iter().any(|&s| s != 0.0)),
            "mix must carry a non-silent loop"
        );
    }

    #[test]
    fn live_graph_stays_silent_without_clips() {
        let p = Project::new("p", "Empty");
        assert_eq!(loop_beats(&p), None);
        assert!(live_graph(&p, 44100, &SampleBank::empty()).is_none());
    }

    #[test]
    fn live_loop_hears_banked_samples() {
        // An `asset:` clip + its banked WAV must reach the pushed loop.
        let src = crate::bounce::Stem {
            name: "hit".to_string(),
            sample_rate: 44100,
            samples: vec![0.5; 4410],
            loop_start: 0,
            loop_end: 4410,
        };
        let mut bank = SampleBank::empty();
        bank.insert_wav("hit.wav", &crate::bounce::encode_wav(&src)).expect("decode");
        let mut p = Project::new("p", "Sampled");
        p.tracks.push(crate::model::Track {
            id: "trk".to_string(),
            name: "Trk".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        p.clips.push(crate::model::Clip {
            id: "clip".to_string(),
            name: "Hit".to_string(),
            track_id: "trk".to_string(),
            start_beats: 0.0,
            length_beats: 0.2,
            kind: crate::model::ClipKind::Audio,
            source: "asset:hit.wav".to_string(),
        });
        let (graph, _) = live_graph(&p, 44100, &bank).expect("sample must sound");
        assert!(
            matches!(graph.proc_of("mix"), Proc::Loop { buf } if buf.iter().any(|&s| s != 0.0)),
            "mix loop must carry the decoded sample"
        );
    }

    #[test]
    fn loop_length_clamps_to_whole_beats() {
        let mut p = sounding_project();
        p.clips[0].length_beats = 10_000.0;
        assert_eq!(loop_beats(&p), Some(MAX_LIVE_LOOP_BEATS));
    }

    #[test]
    fn pumped_blocks_sound_and_advance_through_the_loop() {
        // The transport played silent before: bare topology renders Nulls.
        // Pump the live graph through the real null backend and prove two
        // consecutive blocks are non-silent and contiguous.
        let p = sounding_project();
        let (graph, delays) = live_graph(&p, 44100, &SampleBank::empty()).expect("must sound");
        let (engine, rx, params) = AudioEngine::channel();
        let mut backend = NullBackend::new(rx, params);
        engine.send(AudioCommand::SwapGraph {
            graph,
            delays,
            out_node: "mix".to_string(),
        });
        let frames = 512;
        assert!(backend.pump(frames));
        assert!(backend.pump(frames));
        assert!(
            backend.rendered.iter().any(|b| b.iter().any(|&s| s != 0.0)),
            "loop must sound"
        );
        let joined: Vec<f32> = backend.rendered.concat();
        let (g2, d2) = live_graph(&p, 44100, &SampleBank::empty()).expect("must sound");
        let expect = g2.render(joined.len(), 1, &d2, 0).expect("renders")["mix"].clone();
        assert_eq!(joined, expect, "blocks must be contiguous loop reads");
    }
}
