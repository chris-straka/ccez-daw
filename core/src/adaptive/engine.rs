//! Sample-accurate vertical-layer mixer for one [`AdaptiveCue`].
//!
//! Model: every layer owns a mono stem buffer (caller-rendered, e.g. via the
//! Track B path). Each output sample is
//!
//! ```text
//! out[t] = sum over layers of stem_l[t] * volume_l * state_gain_l[t] * override_gain_l
//! ```
//!
//! - `stem_l[t]` wraps around the stem (stems loop; one-shots are just stems
//!   the author keeps short — the engine never stops the clock).
//! - `volume_l` is the contract's linear layer volume.
//! - `state_gain_l[t]` is 1.0 for audible layers, 0.0 otherwise, linearly
//!   ramped per sample across a `Fade` window. `Cut` flips on one sample.
//! - `override_gain_l` is the mute/solo audition mask (constant per render).
//!
//! State changes take effect at an exact sample (`post_snapshot_at`), so a
//! transition landing mid-block is bit-exact, not block-quantized. Missing
//! stems render as silence: dangling `clip_ids` are an *export-validator*
//! error (contracts/export-package.md), never a realtime failure.
//!
//! `BarWait` / `Stinger` degrade to `Cut` here (returned as [`Degraded::Yes`]);
//! bar grids and stinger one-shots are the transport slice's job.

use std::collections::{BTreeMap, BTreeSet};

use crate::game_audio::{AdaptiveCue, GameStateSnapshot, TransitionKind};

/// Whether the chosen transition ran as authored or fell back to a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Degraded {
    /// `Cut`/`Fade` ran exactly as authored (including no-rule cut fallback,
    /// which *is* the authored contract behavior).
    No,
    /// `BarWait`/`Stinger` ran as a cut; the transport slice owns the full
    /// behavior.
    Yes,
}

/// Per-layer audition override (orthogonal to game-state audibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerOverride {
    /// No override: audibility comes from the game state alone.
    #[default]
    None,
    /// Layer never sounds, even when its state is active.
    Muted,
    /// Solo forces the layer audible regardless of game state (audition
    /// isolation). When any layer is soloed, non-soloed layers go quiet.
    Soloed,
}

/// Linear gain ramp for one layer across a sample window.
#[derive(Debug, Clone, PartialEq)]
struct Ramp {
    from: f64,
    to: f64,
    /// First sample the ramp applies to (inclusive).
    start: u64,
    /// First sample at the final gain (ramp covers `[start, end)`).
    end: u64,
}

impl Ramp {
    fn gain_at(&self, t: u64) -> f64 {
        if t < self.start {
            return self.from;
        }
        if t >= self.end {
            return self.to;
        }
        let len = (self.end - self.start) as f64;
        if len <= 0.0 {
            return self.to;
        }
        let k = (t - self.start) as f64 / len;
        self.from + (self.to - self.from) * k
    }
}

#[derive(Debug, Clone)]
struct LayerMix {
    gain: f64,
    ramp: Option<Ramp>,
    override_state: LayerOverride,
}

impl LayerMix {
    fn gain_at(&self, t: u64) -> f64 {
        match &self.ramp {
            Some(r) => r.gain_at(t),
            None => self.gain,
        }
    }

    /// Settle any ramp fully covered by sample `t`: bake the gain in.
    fn settle(&mut self, t: u64) {
        if let Some(r) = &self.ramp {
            if t >= r.end {
                self.gain = r.to;
                self.ramp = None;
            }
        }
    }
}

/// Vertical-layer mixer for one cue.
///
/// All positions are absolute sample indices on one clock; construct with the
/// render sample rate. The engine starts in the cue's `default_state` with
/// settled gains (no opening fade).
#[derive(Debug, Clone)]
pub struct AdaptiveEngine {
    cue: AdaptiveCue,
    sample_rate: f64,
    state: String,
    layers: BTreeMap<String, LayerMix>,
}

impl AdaptiveEngine {
    /// New engine in `cue.default_state`. `sample_rate` must be finite > 0.
    ///
    /// # Panics
    ///
    /// Panics on a non-finite or non-positive `sample_rate`.
    pub fn new(cue: AdaptiveCue, sample_rate: f64) -> Self {
        assert!(
            sample_rate.is_finite() && sample_rate > 0.0,
            "sample_rate {sample_rate} must be finite and > 0"
        );
        let state = cue.default_state.clone();
        let mut engine = Self {
            cue,
            sample_rate,
            state: state.clone(),
            layers: BTreeMap::new(),
        };
        for layer in &engine.cue.layers.clone() {
            let audible = layer.states.is_empty() || layer.states.iter().any(|s| s == &state);
            engine.layers.insert(
                layer.id.clone(),
                LayerMix {
                    gain: if audible { 1.0 } else { 0.0 },
                    ramp: None,
                    override_state: LayerOverride::None,
                },
            );
        }
        engine
    }

    /// Current game state driving the mix.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Layer ids audible in the current state, before mute/solo overrides.
    pub fn audible_layers(&self) -> Vec<String> {
        self.cue
            .layers_for_state(&self.state)
            .into_iter()
            .map(|l| l.id.clone())
            .collect()
    }

    /// Set a mute/solo audition override for one layer.
    /// Unknown ids are ignored (audition UI may race cue edits).
    pub fn set_override(&mut self, layer_id: &str, override_state: LayerOverride) {
        if let Some(mix) = self.layers.get_mut(layer_id) {
            mix.override_state = override_state;
        }
    }

    /// Post a snapshot effective at sample `at` (sample-accurate boundary).
    ///
    /// Resolves the contract's transition rule (`transition_for`, no rule =
    /// cut), retargets every layer, and returns whether the transition was
    /// degraded (see [`Degraded`]).
    ///
    /// Calls must run in non-decreasing sample order (render up to `at`,
    /// then post, then render from `at`): posting a boundary and rendering
    /// *earlier* samples afterwards yields the new mix, not the old one.
    pub fn post_snapshot_at(&mut self, snapshot: &GameStateSnapshot, at: u64) -> Degraded {
        let from = std::mem::replace(&mut self.state, snapshot.state.clone());
        let to = self.state.clone();
        let (kind, fade_beats) = match self.cue.transition_for(&from, &to) {
            Some(rule) => (rule.kind.clone(), rule.fade_beats),
            None => (TransitionKind::Cut, 0.0),
        };
        let degraded = matches!(kind, TransitionKind::BarWait | TransitionKind::Stinger);
        let fade_samples = if matches!(kind, TransitionKind::Fade) && fade_beats > 0.0 {
            beats_to_samples(fade_beats, self.cue.tempo, self.sample_rate)
        } else {
            0
        };
        let target: BTreeSet<String> = self.cue.layers_for_state(&to).iter().map(|l| l.id.clone()).collect();
        let layer_ids: Vec<String> = self.layers.keys().cloned().collect();
        for id in layer_ids {
            let to_gain = if target.contains(&id) { 1.0 } else { 0.0 };
            let mix = self.layers.get_mut(&id).expect("layer mix");
            let from_gain = mix.gain_at(at);
            mix.settle(at);
            if fade_samples == 0 {
                mix.gain = to_gain;
                mix.ramp = None;
            } else if (from_gain - to_gain).abs() < f64::EPSILON {
                mix.gain = to_gain;
                mix.ramp = None;
            } else {
                mix.ramp = Some(Ramp { from: from_gain, to: to_gain, start: at, end: at + fade_samples });
            }
        }
        if degraded { Degraded::Yes } else { Degraded::No }
    }

    /// Post a snapshot effective immediately at the engine clock origin.
    /// Convenience for tests and non-realtime audition callers.
    pub fn post_snapshot(&mut self, snapshot: &GameStateSnapshot) -> Degraded {
        self.post_snapshot_at(snapshot, 0)
    }

    /// Render `frames` samples starting at absolute sample `start`.
    ///
    /// `stems` maps layer id → mono stem (looped by wrapping). Missing layers
    /// contribute silence. Output is silence when `frames == 0`.
    pub fn render(&mut self, stems: &BTreeMap<String, Vec<f32>>, start: u64, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames];
        if frames == 0 {
            return out;
        }
        let solo_any = self.layers.values().any(|m| m.override_state == LayerOverride::Soloed);
        // Settle ramps fully behind us so state stays bounded on long renders.
        for mix in self.layers.values_mut() {
            mix.settle(start);
        }
        for layer in &self.cue.layers.clone() {
            let mix = self.layers.get(&layer.id).expect("layer mix");
            // Solo forces the layer audible even when its state is inactive
            // (audition use: isolate a stem without posting snapshots).
            let force_audible = mix.override_state == LayerOverride::Soloed;
            let audible_mask = match mix.override_state {
                LayerOverride::Muted => 0.0,
                LayerOverride::Soloed => 1.0,
                LayerOverride::None => {
                    if solo_any {
                        0.0
                    } else {
                        1.0
                    }
                }
            };
            if audible_mask == 0.0 {
                continue;
            }
            let stem_gain = (layer.volume as f32) * (audible_mask as f32);
            if stem_gain == 0.0 {
                continue;
            }
            let stem = match stems.get(&layer.id) {
                Some(s) if !s.is_empty() => s,
                _ => continue,
            };
            let ramp = mix.ramp.clone();
            let settled = mix.gain;
            let len = stem.len() as u64;
            for i in 0..frames {
                let t = start + i as u64;
                let state_gain = if force_audible {
                    1.0
                } else {
                    match &ramp {
                        Some(r) => r.gain_at(t) as f32,
                        None => settled as f32,
                    }
                };
                if state_gain == 0.0 {
                    continue;
                }
                out[i] += stem[(t % len) as usize] * stem_gain * state_gain;
            }
        }
        // Bake in ramps fully covered by this block.
        let end = start + frames as u64;
        for mix in self.layers.values_mut() {
            mix.settle(end);
        }
        out
    }

    /// Fade window, in samples, for `fade_beats` at this cue's tempo.
    pub fn fade_samples(&self, fade_beats: f64) -> u64 {
        beats_to_samples(fade_beats, self.cue.tempo, self.sample_rate)
    }
}

/// Whole beats → whole samples at `tempo` BPM. Non-positive beats → 0.
fn beats_to_samples(fade_beats: f64, tempo: f64, sample_rate: f64) -> u64 {
    if !(fade_beats > 0.0) || !(tempo > 0.0) || !sample_rate.is_finite() {
        return 0;
    }
    ((fade_beats * 60.0 / tempo * sample_rate).round() as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_audio::{CueLayer, TransitionRule};

    const SR: f64 = 48_000.0;

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
        cue
    }

    fn snap(state: &str) -> GameStateSnapshot {
        GameStateSnapshot { state: state.to_string(), values: vec![] }
    }

    fn stems() -> BTreeMap<String, Vec<f32>> {
        // Constant-valued stems: bed = 1.0, drums = 1.0, so the mix level
        // directly reads out the gain math (bed 0.8 / drums 0.9 volumes).
        BTreeMap::from([
            ("bed".to_string(), vec![1.0f32; 48_000]),
            ("drums".to_string(), vec![1.0f32; 48_000]),
        ])
    }

    #[test]
    fn state_change_remix_renders_audible_sets() {
        let mut engine = AdaptiveEngine::new(demo_cue(), SR);
        assert_eq!(engine.state(), "explore");
        assert_eq!(engine.audible_layers(), vec!["bed".to_string()]);

        // Explore: bed only at layer volume 0.8.
        let out = engine.render(&stems(), 0, 8);
        assert!(out.iter().all(|&s| (s - 0.8).abs() < 1e-6));

        // Explore -> combat: authored Fade rule, 4 beats @120bpm = 96_000 samples.
        let at = 1_000u64;
        let degraded = engine.post_snapshot_at(&snap("combat"), at);
        assert_eq!(degraded, Degraded::No);
        assert_eq!(engine.fade_samples(4.0), 96_000);

        // Sample before the boundary still the old mix...
        let before = engine.render(&stems(), at - 4, 4);
        assert!(before.iter().all(|&s| (s - 0.8).abs() < 1e-6));
        // ...and the boundary sample starts the crossfade exactly: drums gain
        // ramps 0 -> 1 while the always-on bed holds at 1.
        let edge = engine.render(&stems(), at, 2);
        assert!((edge[0] - 0.8).abs() < 1e-6, "fade starts at 0, got {}", edge[0]);
        // Mid-fade drums gain ~= 0.5 -> 0.8 + 0.9 * 0.5 = 1.25.
        let mid = engine.render(&stems(), at + 48_000, 1);
        assert!((mid[0] - 1.25).abs() < 1e-4, "mid-fade mix, got {}", mid[0]);
        // Fade settled: bed + drums.
        let settled = engine.render(&stems(), at + 96_000, 4);
        assert!(settled.iter().all(|&s| (s - 1.7).abs() < 1e-4));

        // Combat -> explore has NO rule: contract cut fallback, exact sample.
        // (Render the pre-boundary sample first: the engine runs in sample
        // order, so posting `back` retires the old mix.)
        let back = 200_000u64;
        let pre = engine.render(&stems(), back - 1, 1);
        assert!((pre[0] - 1.7).abs() < 1e-4);
        let degraded = engine.post_snapshot_at(&snap("explore"), back);
        assert_eq!(degraded, Degraded::No);
        let post = engine.render(&stems(), back, 4);
        assert!(post.iter().all(|&s| (s - 0.8).abs() < 1e-6));
    }

    #[test]
    fn fade_ramp_is_linear_and_sample_accurate() {
        let mut engine = AdaptiveEngine::new(demo_cue(), SR);
        let at = 500u64;
        engine.post_snapshot_at(&snap("combat"), at);
        // Quarter points of the 96k-sample fade: drums gain 0.25 / 0.75.
        for (offset, gain) in [(24_000u64, 0.25f32), (72_000, 0.75)] {
            let out = engine.render(&stems(), at + offset, 1);
            let want = 0.8 + 0.9 * gain;
            assert!((out[0] - want).abs() < 1e-4, "offset {offset}: got {}, want {want}", out[0]);
        }
    }

    #[test]
    fn mute_solo_override_state_audibility() {
        let mut engine = AdaptiveEngine::new(demo_cue(), SR);
        // Mute the always-on bed: explore goes silent.
        engine.set_override("bed", LayerOverride::Muted);
        let out = engine.render(&stems(), 0, 4);
        assert!(out.iter().all(|&s| s == 0.0));
        // Solo drums while still in explore: drums sound despite the state.
        engine.set_override("bed", LayerOverride::None);
        engine.set_override("drums", LayerOverride::Soloed);
        let out = engine.render(&stems(), 0, 4);
        assert!(out.iter().all(|&s| (s - 0.9).abs() < 1e-6));
        // Unknown layer ids never error (UI may race cue edits).
        engine.set_override("nope", LayerOverride::Muted);
    }

    #[test]
    fn barwait_and_stinger_degrade_to_audible_cut() {
        let mut cue = demo_cue();
        cue.transitions.push(TransitionRule {
            id: "t2".to_string(),
            from_state: "combat".to_string(),
            to_state: "explore".to_string(),
            kind: TransitionKind::BarWait,
            fade_beats: 0.0,
            stinger_cue_id: String::new(),
        });
        let mut engine = AdaptiveEngine::new(cue, SR);
        engine.post_snapshot_at(&snap("combat"), 0);
        let degraded = engine.post_snapshot_at(&snap("explore"), 10);
        assert_eq!(degraded, Degraded::Yes);
        let out = engine.render(&stems(), 10, 4);
        assert!(out.iter().all(|&s| (s - 0.8).abs() < 1e-6));
    }

    #[test]
    fn missing_stem_is_silence_not_an_error() {
        let mut engine = AdaptiveEngine::new(demo_cue(), SR);
        let thin: BTreeMap<String, Vec<f32>> =
            BTreeMap::from([("bed".to_string(), vec![1.0f32; 8])]);
        let out = engine.render(&thin, 0, 4);
        assert!(out.iter().all(|&s| (s - 0.8).abs() < 1e-6));
        assert_eq!(engine.render(&thin, 0, 0).len(), 0);
    }
}
