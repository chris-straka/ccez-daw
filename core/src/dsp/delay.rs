//! Feedback delay line with millisecond time, feedback, and wet/dry mix.
//!
//! Teaching note: a delay is a *circular buffer* — a tape loop. Each
//! sample is written at the head and read `delay_samples` behind it; a
//! fraction (`feedback`) of what is read is written back, so one tap
//! becomes a decaying train. `mix` crossfades the dry input with the tap.
//! Feed it an impulse: echoes must land exactly on multiples of the delay
//! time, each quieter than the last — the render test below.

use crate::model::{Node, NodeKind, Param};

pub const PARAM_TIME_MS: &str = "time_ms";
pub const PARAM_FEEDBACK: &str = "feedback";
pub const PARAM_MIX: &str = "mix";
const MAX_DELAY_S: f32 = 2.0;

/// Feedback delay kernel. The line is preallocated; `process` borrows only.
#[derive(Debug, Clone)]
pub struct Delay {
    pub sample_rate: f32,
    pub time_ms: f32,
    pub feedback: f32,
    pub mix: f32,
    line: Vec<f32>,
    pos: usize,
}

impl Delay {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        Self {
            sample_rate: sr,
            time_ms: 375.0,
            feedback: 0.35,
            mix: 0.3,
            line: vec![0.0; (sr * MAX_DELAY_S) as usize + 1],
            pos: 0,
        }
    }

    pub fn latency_samples(&self) -> u64 {
        0
    }

    pub fn reset(&mut self) {
        self.line.fill(0.0);
        self.pos = 0;
    }

    /// Current delay in samples (test probe).
    pub fn delay_samples(&self) -> usize {
        ((self.time_ms / 1000.0 * self.sample_rate) as usize).clamp(1, self.line.len() - 1)
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let n = input.len().min(output.len());
        let d = self.delay_samples();
        let len = self.line.len();
        let wet = self.mix.clamp(0.0, 1.0);
        let fb = self.feedback.clamp(0.0, 0.95);
        for i in 0..n {
            let tap = (self.pos + len - d) % len;
            let echo = self.line[tap];
            self.line[self.pos] = input[i] + echo * fb;
            output[i] = input[i] * (1.0 - wet) + echo * wet;
            self.pos += 1;
            if self.pos >= len {
                self.pos = 0;
            }
        }
    }

    pub fn apply_param(&mut self, id: &str, value: f64) -> Result<(), String> {
        match id {
            PARAM_TIME_MS => self.time_ms = value.clamp(1.0, MAX_DELAY_S as f64 * 1000.0) as f32,
            PARAM_FEEDBACK => self.feedback = value.clamp(0.0, 0.95) as f32,
            PARAM_MIX => self.mix = value.clamp(0.0, 1.0) as f32,
            _ => return Err(format!("unknown delay param `{id}`")),
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
                Self::param(PARAM_TIME_MS, "Time", 375.0, 1.0, 2000.0, "ms"),
                Self::param(PARAM_FEEDBACK, "Feedback", 0.35, 0.0, 0.95, "x"),
                Self::param(PARAM_MIX, "Mix", 0.3, 0.0, 1.0, "x"),
            ],
        }
    }

    pub fn from_node(node: &Node, sample_rate: f32) -> Self {
        let mut d = Self::new(sample_rate);
        for p in &node.params {
            let _ = d.apply_param(&p.id, p.value);
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_into(frames: usize, d: &mut Delay) -> Vec<f32> {
        let mut input = vec![0.0; frames];
        input[0] = 1.0;
        let mut out = vec![0.0; frames];
        d.process(&input, &mut out);
        out
    }

    #[test]
    fn echoes_land_on_the_grid_and_decay() {
        let sr = 48_000.0;
        let mut d = Delay::new(sr);
        d.apply_param(PARAM_TIME_MS, 100.0).unwrap();
        d.apply_param(PARAM_FEEDBACK, 0.5).unwrap();
        d.apply_param(PARAM_MIX, 1.0).unwrap();
        let taps = d.delay_samples();
        assert_eq!(taps, 4800);
        let out = impulse_into(taps * 4 + 64, &mut d);
        // Fully wet: dry impulse itself is cancelled — frame 0 is silent.
        assert_eq!(out[0], 0.0);
        // Echoes at exactly 1x/2x/3x the delay, geometrically decaying.
        let (e1, e2, e3) = (out[taps], out[2 * taps], out[3 * taps]);
        assert!((e1 - 1.0).abs() < 1e-5, "first echo {e1}");
        assert!((e2 - 0.5).abs() < 1e-4, "second echo {e2}");
        assert!((e3 - 0.25).abs() < 1e-4, "third echo {e3}");
        // Off-grid frames stay silent.
        assert!(out[taps / 2].abs() < 1e-6);
    }

    #[test]
    fn dry_mix_is_a_wire_and_reset_clears_repeats() {
        let mut d = Delay::new(48_000.0);
        d.apply_param(PARAM_MIX, 0.0).unwrap();
        let out = impulse_into(512, &mut d);
        assert_eq!(out[0], 1.0);
        assert!(out[1..].iter().all(|&v| v == 0.0));
        // Reset mid-train: pending echoes vanish.
        d.apply_param(PARAM_MIX, 1.0).unwrap();
        d.reset();
        let mut input = vec![0.0; 256];
        input[0] = 1.0;
        let mut o = vec![0.0; 256];
        d.process(&input, &mut o);
        d.reset();
        let mut o2 = vec![0.0; 8192];
        d.process(&[0.0; 8192], &mut o2);
        assert!(o2.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn node_round_trip_and_bad_param() {
        let node = Delay::default_node("dev_dly", "Native Delay");
        let back: Node = serde_json::from_str(&serde_json::to_string(&node).unwrap()).unwrap();
        assert_eq!(node, back);
        assert_eq!(Delay::from_node(&back, 44_100.0).time_ms, 375.0);
        let mut d = Delay::new(48_000.0);
        assert!(d.apply_param("nope", 0.0).is_err());
        d.apply_param(PARAM_FEEDBACK, 99.0).unwrap();
        assert_eq!(d.feedback, 0.95);
    }
}
