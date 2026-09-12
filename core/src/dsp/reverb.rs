//! Schroeder reverb: 4 parallel damped combs into 2 series allpasses.
//!
//! Teaching note: a room is thousands of echoes, but the ear accepts a
//! sketch — a handful of *comb* filters (delay lines feeding back into
//! themselves: the metallic ringing) in parallel, then *allpass* filters
//! (they smear phase without coloring loudness: the diffusion). `size`
//! scales the feedback (bigger room = longer tail), `damping` is a
//! one-pole lowpass inside each comb loop (darker room = faster-murdering
//! highs), `mix` crossfades dry/wet. Feed it an impulse and the tail
//! must decay smoothly to near-silence — the render test below.

use crate::model::{Node, NodeKind, Param};

pub const PARAM_SIZE: &str = "size";
pub const PARAM_DAMPING: &str = "damping";
pub const PARAM_MIX: &str = "mix";

const COMB_BASE: [usize; 4] = [1116, 1188, 1277, 1356];
const ALLPASS_BASE: [usize; 2] = [556, 441];

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    feedback: f32,
    damp: f32,
    lp: f32,
}

impl Comb {
    fn new(len: usize, feedback: f32, damp: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(2)],
            pos: 0,
            feedback,
            damp: damp.clamp(0.0, 1.0),
            lp: 0.0,
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let out = self.buf[self.pos];
        self.lp = out.mul_add(1.0 - self.damp, self.lp * self.damp);
        self.buf[self.pos] = x + self.lp * self.feedback;
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
        out
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
    feedback: f32,
}

impl Allpass {
    fn new(len: usize, feedback: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(2)],
            pos: 0,
            feedback,
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let bufout = self.buf[self.pos];
        let y = -x + bufout;
        self.buf[self.pos] = x + bufout * self.feedback;
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

/// Schroeder reverb kernel. Heap holds the lines; `process` borrows only.
#[derive(Debug, Clone)]
pub struct Reverb {
    pub sample_rate: f32,
    /// 0 = small closet, 1 = cathedral.
    pub size: f32,
    /// 0 = bright, 1 = dark.
    pub damping: f32,
    /// 0 = dry, 1 = fully wet.
    pub mix: f32,
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let mut r = Self {
            sample_rate: sample_rate.max(1.0),
            size: 0.5,
            damping: 0.4,
            mix: 0.3,
            combs: Vec::new(),
            allpasses: Vec::new(),
        };
        r.rebuild();
        r
    }

    pub fn latency_samples(&self) -> u64 {
        0
    }

    pub fn reset(&mut self) {
        for c in &mut self.combs {
            c.buf.fill(0.0);
            c.lp = 0.0;
            c.pos = 0;
        }
        for a in &mut self.allpasses {
            a.buf.fill(0.0);
            a.pos = 0;
        }
    }

    fn scale(&self, base: usize) -> usize {
        ((base as f32 * self.sample_rate / 44_100.0) as usize).max(2)
    }

    fn rebuild(&mut self) {
        let fb = 0.70 + 0.25 * self.size.clamp(0.0, 1.0);
        self.combs = COMB_BASE
            .iter()
            .map(|&b| Comb::new(self.scale(b), fb, self.damping))
            .collect();
        self.allpasses = ALLPASS_BASE
            .iter()
            .map(|&b| Allpass::new(self.scale(b), 0.5))
            .collect();
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let n = input.len().min(output.len());
        let wet = self.mix.clamp(0.0, 1.0);
        for i in 0..n {
            let mut v = 0.0;
            for c in self.combs.iter_mut() {
                v += c.run(input[i]);
            }
            v *= 0.25;
            for a in self.allpasses.iter_mut() {
                v = a.run(v);
            }
            output[i] = input[i] * (1.0 - wet) + v * wet;
        }
    }

    pub fn apply_param(&mut self, id: &str, value: f64) -> Result<(), String> {
        match id {
            PARAM_SIZE => self.size = value.clamp(0.0, 1.0) as f32,
            PARAM_DAMPING => {
                self.damping = value.clamp(0.0, 1.0) as f32;
                for c in &mut self.combs {
                    c.damp = self.damping;
                }
                return Ok(());
            }
            PARAM_MIX => self.mix = value.clamp(0.0, 1.0) as f32,
            _ => return Err(format!("unknown reverb param `{id}`")),
        }
        self.rebuild();
        Ok(())
    }

    fn param(id: &str, label: &str, value: f64) -> Param {
        Param {
            id: id.to_string(),
            label: label.to_string(),
            value,
            min: 0.0,
            max: 1.0,
            default: value,
            unit: "x".to_string(),
        }
    }

    pub fn default_node(id: &str, name: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Device,
            name: name.to_string(),
            params: vec![
                Self::param(PARAM_SIZE, "Size", 0.5),
                Self::param(PARAM_DAMPING, "Damping", 0.4),
                Self::param(PARAM_MIX, "Mix", 0.3),
            ],
        }
    }

    pub fn from_node(node: &Node, sample_rate: f32) -> Self {
        let mut r = Self::new(sample_rate);
        for p in &node.params {
            let _ = r.apply_param(&p.id, p.value);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_response(frames: usize, mix: f32) -> Vec<f32> {
        let mut r = Reverb::new(44_100.0);
        r.apply_param(PARAM_MIX, mix as f64).unwrap();
        let mut input = vec![0.0; frames];
        input[0] = 1.0;
        let mut out = vec![0.0; frames];
        r.process(&input, &mut out);
        out
    }

    fn energy(v: &[f32]) -> f32 {
        v.iter().map(|s| s * s).sum()
    }

    #[test]
    fn impulse_produces_decaying_tail() {
        let out = impulse_response(44_100, 1.0);
        // Wet tail exists well past the longest line (~1356 samples).
        assert!(energy(&out[2000..8000]) > 1e-6);
        // ...and has effectively died after 1 s in a mid-size room.
        assert!(energy(&out[40_000..]) < 1e-4);
        // Decay is monotonic-ish across quarters (no second bang).
        let q = |a: usize, b: usize| energy(&out[a..b]);
        assert!(q(0, 11_025) > q(11_025, 22_050));
        assert!(q(11_025, 22_050) > q(22_050, 33_075));
    }

    #[test]
    fn dry_mix_passes_through_and_size_grows_tail() {
        // mix = 0: the device is a wire.
        let out = impulse_response(4096, 0.0);
        assert_eq!(out[0], 1.0);
        assert!(out[1..].iter().all(|&v| v == 0.0));
        // Bigger room holds more late energy than a closet.
        let mut small = Reverb::new(44_100.0);
        small.apply_param(PARAM_SIZE, 0.0).unwrap();
        small.apply_param(PARAM_MIX, 1.0).unwrap();
        let mut big = Reverb::new(44_100.0);
        big.apply_param(PARAM_SIZE, 1.0).unwrap();
        big.apply_param(PARAM_MIX, 1.0).unwrap();
        let frames = 44_100;
        let mut input = vec![0.0; frames];
        input[0] = 1.0;
        let (mut os, mut ob) = (vec![0.0; frames], vec![0.0; frames]);
        small.process(&input, &mut os);
        big.process(&input, &mut ob);
        assert!(energy(&ob[20_000..]) > energy(&os[20_000..]));
    }

    #[test]
    fn reset_silences_tail_and_node_round_trips() {
        let mut r = Reverb::new(44_100.0);
        let mut input = vec![0.0; 512];
        input[0] = 1.0;
        let mut o = vec![0.0; 512];
        r.process(&input, &mut o);
        r.reset();
        let mut o2 = vec![0.0; 512];
        r.process(&[0.0; 512], &mut o2);
        assert!(o2.iter().all(|&v| v == 0.0));
        let node = Reverb::default_node("dev_verb", "Native Reverb");
        let back: Node = serde_json::from_str(&serde_json::to_string(&node).unwrap()).unwrap();
        assert_eq!(node, back);
        assert_eq!(Reverb::from_node(&back, 48_000.0).mix, 0.3);
        assert!(r.apply_param("nope", 0.0).is_err());
    }
}
