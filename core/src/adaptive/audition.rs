//! State-driven adaptive audition: offline "what does this state sound
//! like?" preview for one [`AdaptiveCue`].
//!
//! Teaching note: the realtime answer lives in [`engine`](super::engine)
//! (`AdaptiveEngine`: stems x state, sample-accurate). This module is the
//! rehearsal-room twin — the [`sfx`](crate::sfx) `audition` philosophy for
//! music: resolve a [`GameStateSnapshot`] to a per-layer mix (audible layers
//! x layer volume), then synthesize a deterministic stand-in tone per layer
//! so the mix is audible without audio hardware or v0 clip decoders.
//!
//! - Audibility mirrors `AdaptiveCue::layers_for_state` (empty `states` =
//!   always-on bed). If the preview and the engine ever disagree, the
//!   engine wins and this preview is the bug.
//! - Tone construction matches [`gaexport`](crate::gaexport) (220 cycles
//!   per beat at the cue tempo, deterministic per-layer 0-4 semitone shift
//!   from `cue:layer`, scaled by layer volume), so auditioned mixes and
//!   exported stems agree in character.
//! - Transition flavors are reported, never faked: `BarWait`/`Stinger`
//!   degrade to a cut, exactly like the engine's `Degraded::Yes`.
//! - Pure function of (cue, snapshot): same state, same samples, every run.
//!
//! Frozen-shape discipline: this module only *evaluates* the v1 contracts —
//! no new serialized types, no `emit` changes.

use crate::game_audio::{AdaptiveCue, CueLayer, GameStateSnapshot, TransitionKind};

/// Audition render settings.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditionConfig {
    pub sample_rate: u32,
    pub seconds: f32,
}

impl Default for AuditionConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            // Two seconds = four beats at 120 BPM: long enough to hear a
            // loop-clean reference tone settle.
            seconds: 2.0,
        }
    }
}

/// Whether the authored transition runs as written or falls back to a cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditionTransition {
    /// `Cut`/`Fade` (or the no-rule cut fallback) render as authored.
    Direct(TransitionKind),
    /// `BarWait`/`Stinger` need the transport slice; audition plays a cut.
    DegradedCut,
}

/// Layers audible under `snapshot` (always-on bed layers included).
pub fn audible_layers<'a>(cue: &'a AdaptiveCue, snapshot: &GameStateSnapshot) -> Vec<&'a CueLayer> {
    cue.layers_for_state(&snapshot.state)
}

/// State-to-layer mix resolution: one `(layer_id, gain)` per cue layer.
/// Audible layers sound at their contract `volume`; anything else is `0.0`.
/// This is the audition twin of the engine's per-sample gain math, settled
/// (no ramps): the mix a state holds once its transition completes.
pub fn layer_mix(cue: &AdaptiveCue, snapshot: &GameStateSnapshot) -> Vec<(String, f64)> {
    let audible: std::collections::HashSet<&str> = cue
        .layers_for_state(&snapshot.state)
        .into_iter()
        .map(|l| l.id.as_str())
        .collect();
    cue.layers
        .iter()
        .map(|l| {
            let gain = if audible.contains(l.id.as_str()) {
                l.volume.max(0.0)
            } else {
                0.0
            };
            (l.id.clone(), gain)
        })
        .collect()
}

/// How a state change would sound in audition: the authored kind, or a
/// labeled degraded cut for transport-owned flavors (and the no-rule cut
/// fallback, which *is* the authored contract behavior).
pub fn effective_transition(
    cue: &AdaptiveCue,
    from_state: &str,
    to_state: &str,
) -> AuditionTransition {
    match cue.transition_for(from_state, to_state) {
        None => AuditionTransition::Direct(TransitionKind::Cut),
        Some(rule) => match rule.kind {
            TransitionKind::BarWait | TransitionKind::Stinger => AuditionTransition::DegradedCut,
            _ => AuditionTransition::Direct(rule.kind.clone()),
        },
    }
}

/// FNV-1a 64-bit: deterministic id -> tone mapping without tables (same
/// construction as the export renderer, kept local so this module needs no
/// new dependencies).
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8482_22_25;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Render one state to mono samples: the settled layer mix as summed
/// reference tones. Deterministic: no RNG, no I/O.
pub fn render_audition(
    cue: &AdaptiveCue,
    snapshot: &GameStateSnapshot,
    config: &AuditionConfig,
) -> Vec<f32> {
    if config.sample_rate == 0 || !(config.seconds > 0.0) {
        return Vec::new();
    }
    let frames = (config.sample_rate as f64 * config.seconds as f64).round() as usize;
    if frames == 0 {
        return Vec::new();
    }
    let mix = layer_mix(cue, snapshot);
    let sr = config.sample_rate as f64;
    let tempo = if cue.tempo.is_finite() && cue.tempo > 0.0 {
        cue.tempo
    } else {
        120.0
    };
    let mut out = vec![0.0f32; frames];
    for (layer_id, gain) in &mix {
        if *gain <= 0.0 {
            continue;
        }
        let semi = (fnv1a64(&format!("{}:{layer_id}", cue.id)) % 5) as f64;
        let hz = 220.0 * (tempo / 60.0) * 2f64.powf(semi / 12.0);
        for (t, s) in out.iter_mut().enumerate() {
            let tone = (0.5 * (2.0 * std::f64::consts::PI * hz * t as f64 / sr).sin()) as f32;
            *s += tone * (*gain as f32);
        }
    }
    out
}

/// Root-mean-square level, the audition "did it sound?" scalar.
pub fn rms(buf: &[f32]) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_audio::{CueLayer, TransitionRule};

    fn demo_cue() -> AdaptiveCue {
        let mut cue = AdaptiveCue::new("cue_fight", "Fight");
        cue.tempo = 120.0;
        cue.default_state = "explore".to_string();
        cue.layers.push(CueLayer {
            id: "bed".to_string(),
            name: "Bed".to_string(),
            clip_ids: vec!["clip_a".to_string()],
            states: vec![],
            volume: 0.8,
        });
        cue.layers.push(CueLayer {
            id: "drums".to_string(),
            name: "Drums".to_string(),
            clip_ids: vec!["clip_b".to_string()],
            states: vec!["combat".to_string()],
            volume: 0.9,
        });
        cue.transitions.push(TransitionRule {
            id: "t1".to_string(),
            from_state: "explore".to_string(),
            to_state: "combat".to_string(),
            kind: TransitionKind::Fade,
            fade_beats: 4.0,
            stinger_cue_id: String::new(),
        });
        cue.transitions.push(TransitionRule {
            id: "t2".to_string(),
            from_state: "combat".to_string(),
            to_state: "explore".to_string(),
            kind: TransitionKind::BarWait,
            fade_beats: 0.0,
            stinger_cue_id: String::new(),
        });
        cue
    }

    fn snap(state: &str) -> GameStateSnapshot {
        GameStateSnapshot {
            state: state.to_string(),
            values: vec![],
        }
    }

    #[test]
    fn state_resolves_to_layer_mix() {
        let cue = demo_cue();
        // Explore: always-on bed only.
        let audible: Vec<String> = audible_layers(&cue, &snap("explore"))
            .into_iter()
            .map(|l| l.id.clone())
            .collect();
        assert_eq!(audible, vec!["bed".to_string()]);
        assert_eq!(
            layer_mix(&cue, &snap("explore")),
            vec![
                ("bed".to_string(), 0.8),
                ("drums".to_string(), 0.0),
            ]
        );
        // Combat: bed holds, drums join at layer volume.
        let audible: Vec<String> = audible_layers(&cue, &snap("combat"))
            .into_iter()
            .map(|l| l.id.clone())
            .collect();
        assert_eq!(audible, vec!["bed".to_string(), "drums".to_string()]);
        assert_eq!(
            layer_mix(&cue, &snap("combat")),
            vec![
                ("bed".to_string(), 0.8),
                ("drums".to_string(), 0.9),
            ]
        );
        // Unknown state: bed only (never silent when a bed exists).
        assert_eq!(layer_mix(&cue, &snap("menu"))[0], ("bed".to_string(), 0.8));
    }

    #[test]
    fn audition_renders_deterministic_audible_state_mixes() {
        let cue = demo_cue();
        let cfg = AuditionConfig {
            sample_rate: 8000,
            seconds: 1.0,
        };
        let explore = render_audition(&cue, &snap("explore"), &cfg);
        let combat = render_audition(&cue, &snap("combat"), &cfg);
        assert_eq!(explore.len(), 8000);
        assert_eq!(render_audition(&cue, &snap("explore"), &cfg), explore);
        assert!(rms(&explore) > 0.01, "bed state must be audible");
        assert!(rms(&combat) > rms(&explore), "combat adds drums");
        assert_ne!(explore, combat);
        // A cue with no layers renders silence, not an error.
        let bare = AdaptiveCue::new("cue_bare", "Bare");
        let silent = render_audition(&bare, &snap("explore"), &cfg);
        assert!(silent.iter().all(|&s| s == 0.0));
        assert_eq!(rms(&silent), 0.0);
    }

    #[test]
    fn transition_flavors_are_reported_never_faked() {
        let cue = demo_cue();
        assert_eq!(
            effective_transition(&cue, "explore", "combat"),
            AuditionTransition::Direct(TransitionKind::Fade)
        );
        assert_eq!(
            effective_transition(&cue, "combat", "explore"),
            AuditionTransition::DegradedCut
        );
        // No rule = authored cut fallback.
        assert_eq!(
            effective_transition(&cue, "explore", "menu"),
            AuditionTransition::Direct(TransitionKind::Cut)
        );
    }

    #[test]
    fn auditioned_cue_passes_export_validation() {
        use crate::gaexport::{build_package, validate_package, ExportOptions, ExportRequest};
        use crate::model::{Clip, ClipKind, Project};

        let cue = demo_cue();
        // The auditioned states must both be non-silent before export ships.
        let cfg = AuditionConfig {
            sample_rate: 8000,
            seconds: 1.0,
        };
        assert!(rms(&render_audition(&cue, &snap("explore"), &cfg)) > 0.01);
        assert!(rms(&render_audition(&cue, &snap("combat"), &cfg)) > 0.01);

        let mut project = Project::new("proj_aud", "Audition export");
        for id in ["clip_a", "clip_b"] {
            project.clips.push(Clip {
                id: id.to_string(),
                track_id: "trk_1".to_string(),
                name: id.to_string(),
                start_beats: 0.0,
                length_beats: 4.0,
                kind: ClipKind::Audio,
                source: format!("samples/{id}.wav"),
            });
        }
        let options = ExportOptions {
            sample_rate: 8000,
            seed: crate::gaexport::DEFAULT_EXPORT_SEED,
        };
        let request = ExportRequest::new("audition-demo", vec![cue.id.clone()], vec![]);
        let built = build_package(&project, &[cue.clone()], &[], &[], &request, options)
            .expect("auditioned cue builds");
        // One stem per layer, loop-clean music loops.
        assert_eq!(built.stems.len(), cue.layers.len());
        for stem in &built.stems {
            assert!(matches!(
                stem.stem.kind,
                crate::game_audio::StemKind::MusicLayer
            ));
            assert!(stem.stem.loop_start_beats < stem.stem.loop_end_beats);
        }
        let dir = std::env::temp_dir().join(format!(
            "ccez-ga-audition-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        crate::gaexport::write_package(&dir, &built).expect("write package");
        let report = validate_package(&dir, &project, &[cue], &[], &[], options);
        assert!(report.is_ok(), "export must validate: {:?}", report.errors);
        std::fs::remove_dir_all(&dir).ok();
    }
}
