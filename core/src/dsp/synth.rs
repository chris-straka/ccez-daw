//! Mono source oscillator with an attack/release envelope.
//!
//! Teaching note: a synth voice is two ideas — a *phase* that walks
//! through a waveform at `freq / sample_rate` cycles per sample, and an
//! *envelope* that scales it (0 = silent, 1 = full). [`Synth::note_on`]
//! starts the attack ramp, [`Synth::note_off`] starts the release ramp.
//! `process` ignores its input (sources *are* the signal, like Track B's
//! `Proc::Impulse`) and writes the voice into `output`.

use crate::model::{Node, NodeKind, Param};

pub const PARAM_FREQ: &str = "freq";
pub const PARAM_GAIN: &str = "gain";
pub const PARAM_WAVE: &str = "wave";
pub const PARAM_ATTACK_MS: &str = "attack_ms";
pub const PARAM_RELEASE_MS: &str = "release_ms";

/// Waveform select: 0 = sine, 1 = saw, 2 = square, 3 = triangle.
pub const WAVE_SINE: f64 = 0.0;
pub const WAVE_SAW: f64 = 1.0;
pub const WAVE_SQUARE: f64 = 2.0;
pub const WAVE_TRIANGLE: f64 = 3.0;

/// Mono synth voice. All state is scalar — `process` allocates nothing.
#[derive(Debug, Clone)]
pub struct Synth {
    pub sample_rate: f32,
    pub freq: f32,
    pub gain: f32,
    pub wave: u8,
    pub attack_ms: f32,
    pub release_ms: f32,
    phase: f32,
    env: f32,
    gate: bool,
}

impl Synth {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            freq: 440.0,
            gain: 0.8,
            wave: 0,
            attack_ms: 5.0,
            release_ms: 50.0,
            phase: 0.0,
            env: 0.0,
            gate: false,
        }
    }

    /// Sources add no latency (no lookahead for compensation to align).
    pub fn latency_samples(&self) -> u64 {
        0
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.env = 0.0;
        self.gate = false;
    }

    pub fn note_on(&mut self) {
        self.gate = true;
    }

    pub fn note_off(&mut self) {
        self.gate = false;
    }

    pub fn is_active(&self) -> bool {
        self.gate || self.env > 1e-6
    }

    fn step(&self) -> f32 {
        if self.gate {
            (1000.0 / (self.attack_ms.max(0.01) * self.sample_rate)).min(1.0)
        } else {
            (1000.0 / (self.release_ms.max(0.01) * self.sample_rate)).min(1.0)
        }
    }

    fn osc(&self) -> f32 {
        let p = self.phase - self.phase.floor();
        match self.wave {
            1 => 2.0 * p - 1.0,
            2 => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => 4.0 * (p - 0.5).abs() - 1.0,
            _ => (p * std::f32::consts::TAU).sin(),
        }
    }

    /// Render the voice into `output`, ignoring `input`.
    /// Lengths may differ — only `min` frames are written.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let _ = input;
        let n = output.len();
        let inc = self.freq / self.sample_rate;
        let step = self.step();
        for o in output.iter_mut().take(n) {
            if self.gate {
                self.env = (self.env + step).min(1.0);
            } else {
                self.env = (self.env - step).max(0.0);
            }
            *o = self.osc() * self.env * self.gain;
            self.phase += inc;
            if self.phase >= 1.0 {
                self.phase -= self.phase.floor();
            }
        }
    }

    /// Render `frames` samples of a held note (test/render helper).
    pub fn render_note(&mut self, frames: usize) -> Vec<f32> {
        self.note_on();
        let mut out = vec![0.0; frames];
        self.process(&[], &mut out);
        out
    }

    /// Clamp + apply one frozen-param value. Unknown ids error, never panic.
    pub fn apply_param(&mut self, id: &str, value: f64) -> Result<(), String> {
        match id {
            PARAM_FREQ => self.freq = value.clamp(20.0, 20_000.0) as f32,
            PARAM_GAIN => self.gain = value.clamp(0.0, 2.0) as f32,
            PARAM_WAVE => {
                if !(0.0..=3.0).contains(&value) {
                    return Err(format!("wave {value} out of [0, 3]"));
                }
                self.wave = value as u8;
            }
            PARAM_ATTACK_MS => self.attack_ms = value.clamp(0.01, 5000.0) as f32,
            PARAM_RELEASE_MS => self.release_ms = value.clamp(0.01, 10_000.0) as f32,
            _ => return Err(format!("unknown synth param `{id}`")),
        }
        Ok(())
    }

    fn param(id: &str, label: &str, value: f64, min: f64, max: f64, default: f64, unit: &str) -> Param {
        Param {
            id: id.to_string(),
            label: label.to_string(),
            value,
            min,
            max,
            default,
            unit: unit.to_string(),
        }
    }

    /// Frozen-schema device node driving this kernel (no schema change).
    pub fn default_node(id: &str, name: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Device,
            name: name.to_string(),
            params: vec![
                Self::param(PARAM_FREQ, "Frequency", 440.0, 20.0, 20_000.0, 440.0, "Hz"),
                Self::param(PARAM_GAIN, "Gain", 0.8, 0.0, 2.0, 0.8, "x"),
                Self::param(PARAM_WAVE, "Wave", WAVE_SINE, 0.0, 3.0, WAVE_SINE, "idx"),
                Self::param(PARAM_ATTACK_MS, "Attack", 5.0, 0.01, 5000.0, 5.0, "ms"),
                Self::param(PARAM_RELEASE_MS, "Release", 50.0, 0.01, 10_000.0, 50.0, "ms"),
            ],
        }
    }

    /// Build a kernel from a frozen device node (missing params = defaults).
    pub fn from_node(node: &Node, sample_rate: f32) -> Self {
        let mut s = Self::new(sample_rate);
        for p in &node.params {
            let _ = s.apply_param(&p.id, p.value);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn sine_renders_near_theory_after_attack() {
        // 440 Hz sine, instant-ish attack, 1 s at 48 kHz: steady-state RMS
        // of gain*g must equal gain/sqrt(2).
        let sr = 48_000.0;
        let mut s = Synth::new(sr);
        s.attack_ms = 0.01;
        s.gain = 0.8;
        let out = s.render_note(sr as usize);
        let tail = &out[(sr as usize) / 2..];
        let got = rms(tail);
        let want = 0.8 / std::f32::consts::SQRT_2;
        assert!((got - want).abs() < 0.01, "rms {got} want {want}");
    }

    #[test]
    fn silence_before_note_on_release_kills_tail() {
        let mut s = Synth::new(48_000.0);
        let mut out = vec![0.0; 256];
        // Never triggered: must be bit-silent.
        s.process(&[1.0; 256], &mut out);
        assert!(out.iter().all(|&v| v == 0.0));
        // Held then released with a fast release: tail must die out.
        s.note_on();
        s.process(&[], &mut out);
        s.note_off();
        s.release_ms = 0.01;
        let mut tail = vec![0.0; 4096];
        s.process(&[], &mut tail);
        assert!(tail[tail.len() - 1].abs() < 1e-4);
        assert!(!s.is_active());
    }

    #[test]
    fn node_round_trip_drives_kernel() {
        let node = Synth::default_node("dev_synth", "Native Synth");
        assert_eq!(node.kind, NodeKind::Device);
        let json = serde_json::to_string(&node).expect("serializes");
        let back: Node = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(node, back);
        let k = Synth::from_node(&back, 44_100.0);
        assert_eq!(k.freq, 440.0);
        assert!(Synth::from_node(&back, 0.0).sample_rate > 0.0);
    }

    #[test]
    fn bad_params_rejected() {
        let mut s = Synth::new(48_000.0);
        assert!(s.apply_param("nope", 1.0).is_err());
        assert!(s.apply_param(PARAM_WAVE, 9.0).is_err());
        s.apply_param(PARAM_FREQ, 1e9).expect("clamps");
        assert_eq!(s.freq, 20_000.0);
    }
}
