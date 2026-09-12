//! Feedforward peak compressor.
//!
//! Teaching note: a compressor is an automatic hand on a fader. A
//! *detector* tracks the input level (fast attack, slow release); when
//! that level passes the *threshold*, the kernel turns the gain down so
//! the output only rises `1/ratio` dB per input dB above it. Below the
//! threshold gain is 1 — quiet passes untouched. *Makeup* is a static
//! output trim applied after the gain computer.

use crate::model::{Node, NodeKind, Param};

pub const PARAM_THRESHOLD_DB: &str = "threshold_db";
pub const PARAM_RATIO: &str = "ratio";
pub const PARAM_ATTACK_MS: &str = "attack_ms";
pub const PARAM_RELEASE_MS: &str = "release_ms";
pub const PARAM_MAKEUP_DB: &str = "makeup_db";

/// Feedforward peak compressor. Scalar state — no allocation in `process`.
#[derive(Debug, Clone)]
pub struct Compressor {
    pub sample_rate: f32,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    env: f32,
}

impl Compressor {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            env: 0.0,
        }
    }

    pub fn latency_samples(&self) -> u64 {
        0
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }

    /// Current detector level in dBFS (test probe).
    pub fn envelope_db(&self) -> f32 {
        if self.env <= 0.0 {
            f32::NEG_INFINITY
        } else {
            20.0 * self.env.log10()
        }
    }

    /// Static gain-computer output (linear) for one detector level.
    fn static_gain(&self, env: f32) -> f32 {
        if env <= 0.0 {
            return 1.0;
        }
        let env_db = 20.0 * env.log10();
        if env_db <= self.threshold_db {
            return 1.0;
        }
        let target_db = self.threshold_db + (env_db - self.threshold_db) / self.ratio.max(1.0);
        10.0f32.powf((target_db - env_db) / 20.0)
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let n = input.len().min(output.len());
        let atk = (-1.0 / (self.attack_ms.max(0.01) * self.sample_rate / 1000.0)).exp();
        let rel = (-1.0 / (self.release_ms.max(0.01) * self.sample_rate / 1000.0)).exp();
        let makeup = 10.0f32.powf(self.makeup_db / 20.0);
        for i in 0..n {
            let peak = input[i].abs();
            let coeff = if peak > self.env { atk } else { rel };
            self.env = coeff * self.env + (1.0 - coeff) * peak;
            output[i] = input[i] * self.static_gain(self.env) * makeup;
        }
    }

    pub fn apply_param(&mut self, id: &str, value: f64) -> Result<(), String> {
        match id {
            PARAM_THRESHOLD_DB => self.threshold_db = value.clamp(-60.0, 0.0) as f32,
            PARAM_RATIO => {
                if !(1.0..=100.0).contains(&value) {
                    return Err(format!("ratio {value} out of [1, 100]"));
                }
                self.ratio = value as f32;
            }
            PARAM_ATTACK_MS => self.attack_ms = value.clamp(0.01, 1000.0) as f32,
            PARAM_RELEASE_MS => self.release_ms = value.clamp(1.0, 5000.0) as f32,
            PARAM_MAKEUP_DB => self.makeup_db = value.clamp(0.0, 24.0) as f32,
            _ => return Err(format!("unknown comp param `{id}`")),
        }
        Ok(())
    }

    fn param(id: &str, label: &str, value: f64, min: f64, max: f64, unit: &str) -> Param {
        Param {
            id: id.to_string(),
            label: label.to_string(),
            value,
            min,
            max,
            default: value,
            unit: unit.to_string(),
        }
    }

    pub fn default_node(id: &str, name: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Device,
            name: name.to_string(),
            params: vec![
                Self::param(PARAM_THRESHOLD_DB, "Threshold", -18.0, -60.0, 0.0, "dB"),
                Self::param(PARAM_RATIO, "Ratio", 4.0, 1.0, 100.0, "x"),
                Self::param(PARAM_ATTACK_MS, "Attack", 10.0, 0.01, 1000.0, "ms"),
                Self::param(PARAM_RELEASE_MS, "Release", 100.0, 1.0, 5000.0, "ms"),
                Self::param(PARAM_MAKEUP_DB, "Makeup", 0.0, 0.0, 24.0, "dB"),
            ],
        }
    }

    pub fn from_node(node: &Node, sample_rate: f32) -> Self {
        let mut c = Self::new(sample_rate);
        for p in &node.params {
            let _ = c.apply_param(&p.id, p.value);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(level: f32, freq: f32, sr: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|t| level * (t as f32 * core::f32::consts::TAU * freq / sr).sin())
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn quiet_passes_untouched_loud_gets_pulled_down() {
        let sr = 48_000.0;
        // -30 dBFS peak sine sits below the -18 dB threshold: unity.
        let quiet = sine(10.0f32.powf(-30.0 / 20.0), 220.0, sr, 8192);
        let mut c = Compressor::new(sr);
        let mut o = vec![0.0; quiet.len()];
        c.process(&quiet, &mut o);
        let tail = o.len() / 2..;
        assert!((rms(&o[tail.clone()]) / rms(&quiet[tail])).abs() - 1.0 < 0.02);
        // 0 dBFS peak sine, 18 dB over: output must settle +4.5 dB over
        // (18 / ratio 4), i.e. ~13.5 dB of gain reduction.
        let loud = sine(1.0, 220.0, sr, 48000);
        c.reset();
        let mut o = vec![0.0; loud.len()];
        c.process(&loud, &mut o);
        let got_db = 20.0 * rms(&o[o.len() / 2..]).log10();
        let want_db = 20.0 * (1.0 / std::f32::consts::SQRT_2).log10() - 13.5;
        assert!((got_db - want_db).abs() < 1.5, "got {got_db} want {want_db}");
    }

    #[test]
    fn silence_stays_silent_and_reset_clears_detector() {
        let mut c = Compressor::new(48_000.0);
        let mut o = vec![9.9; 64];
        c.process(&[0.0; 64], &mut o);
        assert!(o.iter().all(|&v| v == 0.0));
        let loud = vec![1.0; 4800];
        let mut big = vec![0.0; 4800];
        c.process(&loud, &mut big);
        assert!(c.envelope_db() > -1.0);
        c.reset();
        assert_eq!(c.envelope_db(), f32::NEG_INFINITY);
    }

    #[test]
    fn makeup_restores_level_and_node_round_trips() {
        let sr = 48_000.0;
        let loud = sine(1.0, 220.0, sr, 48000);
        let mut c = Compressor::new(sr);
        c.apply_param(PARAM_MAKEUP_DB, 13.5).unwrap();
        let mut o = vec![0.0; loud.len()];
        c.process(&loud, &mut o);
        let got_db = 20.0 * rms(&o[o.len() / 2..]).log10();
        let dry_db = 20.0 * (1.0 / std::f32::consts::SQRT_2).log10();
        assert!((got_db - dry_db).abs() < 1.5, "makeup {got_db} vs dry {dry_db}");
        let node = Compressor::default_node("dev_comp", "Native Comp");
        let back: Node = serde_json::from_str(&serde_json::to_string(&node).unwrap()).unwrap();
        assert_eq!(node, back);
        assert_eq!(Compressor::from_node(&back, sr).ratio, 4.0);
        assert!(c.apply_param("nope", 0.0).is_err());
        assert!(c.apply_param(PARAM_RATIO, 0.5).is_err());
    }
}
