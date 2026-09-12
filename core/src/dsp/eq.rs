//! 3-band equalizer: lowshelf + peaking + highshelf biquads.
//!
//! Teaching note: an EQ band is one *biquad* — a 2-pole/2-zero filter
//! whose 5 coefficients come from a closed-form recipe (the RBJ
//! cookbook) mapping musical knobs (frequency, gain, Q) to math. Bands
//! run in series: lowshelf at 200 Hz, peaking at 1 kHz (Q = 1), highshelf
//! at 6 kHz. Flat (all gains 0 dB) is the null-test identity: the
//! kernel bypasses to a copy, so output is bit-identical to input.

use crate::model::{Node, NodeKind, Param};

pub const PARAM_LOW_DB: &str = "low_db";
pub const PARAM_MID_DB: &str = "mid_db";
pub const PARAM_HIGH_DB: &str = "high_db";
pub const LOW_FREQ: f32 = 200.0;
pub const MID_FREQ: f32 = 1000.0;
pub const MID_Q: f32 = 1.0;
pub const HIGH_FREQ: f32 = 6000.0;

/// One Direct-Form-I biquad section. Scalar state — no allocation.
#[derive(Debug, Clone, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn set(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    fn lowshelf(&mut self, f0: f32, gain_db: f32, sr: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = core::f32::consts::TAU * f0 / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / 2.0 * 2.0f32.sqrt();
        let sq = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) - (a - 1.0) * cos + sq),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - sq),
            (a + 1.0) + (a - 1.0) * cos + sq,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - sq,
        );
    }

    fn highshelf(&mut self, f0: f32, gain_db: f32, sr: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = core::f32::consts::TAU * f0 / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / 2.0 * 2.0f32.sqrt();
        let sq = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) + (a - 1.0) * cos + sq),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - sq),
            (a + 1.0) - (a - 1.0) * cos + sq,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - sq,
        );
    }

    fn peaking(&mut self, f0: f32, gain_db: f32, q: f32, sr: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = core::f32::consts::TAU * f0 / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q.max(0.05));
        self.set(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        );
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// 3-band EQ kernel.
#[derive(Debug, Clone)]
pub struct Eq {
    pub sample_rate: f32,
    pub low_db: f32,
    pub mid_db: f32,
    pub high_db: f32,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
}

impl Eq {
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self {
            sample_rate: sample_rate.max(1.0),
            low_db: 0.0,
            mid_db: 0.0,
            high_db: 0.0,
            low: Biquad::default(),
            mid: Biquad::default(),
            high: Biquad::default(),
        };
        e.retune();
        e
    }

    pub fn latency_samples(&self) -> u64 {
        0
    }

    pub fn reset(&mut self) {
        self.low.x1 = 0.0;
        self.low.x2 = 0.0;
        self.low.y1 = 0.0;
        self.low.y2 = 0.0;
        self.mid.x1 = 0.0;
        self.mid.x2 = 0.0;
        self.mid.y1 = 0.0;
        self.mid.y2 = 0.0;
        self.high.x1 = 0.0;
        self.high.x2 = 0.0;
        self.high.y1 = 0.0;
        self.high.y2 = 0.0;
    }

    fn retune(&mut self) {
        let sr = self.sample_rate;
        self.low.lowshelf(LOW_FREQ.min(sr * 0.45), self.low_db, sr);
        self.mid.peaking(MID_FREQ.min(sr * 0.45), self.mid_db, MID_Q, sr);
        self.high.highshelf(HIGH_FREQ.min(sr * 0.45), self.high_db, sr);
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let n = input.len().min(output.len());
        // Flat bypass: all-trivial coefficients still cost float noise per
        // section, so a flat EQ is a wire — bit-exact and cheaper.
        if self.low_db == 0.0 && self.mid_db == 0.0 && self.high_db == 0.0 {
            output[..n].copy_from_slice(&input[..n]);
            return;
        }
        for i in 0..n {
            let y = self.low.run(input[i]);
            let y = self.mid.run(y);
            output[i] = self.high.run(y);
        }
    }

    pub fn apply_param(&mut self, id: &str, value: f64) -> Result<(), String> {
        match id {
            PARAM_LOW_DB => self.low_db = value.clamp(-24.0, 24.0) as f32,
            PARAM_MID_DB => self.mid_db = value.clamp(-24.0, 24.0) as f32,
            PARAM_HIGH_DB => self.high_db = value.clamp(-24.0, 24.0) as f32,
            _ => return Err(format!("unknown eq param `{id}`")),
        }
        self.retune();
        Ok(())
    }

    fn param(id: &str, label: &str, value: f64) -> Param {
        Param {
            id: id.to_string(),
            label: label.to_string(),
            value,
            min: -24.0,
            max: 24.0,
            default: 0.0,
            unit: "dB".to_string(),
        }
    }

    pub fn default_node(id: &str, name: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Device,
            name: name.to_string(),
            params: vec![
                Self::param(PARAM_LOW_DB, "Low", 0.0),
                Self::param(PARAM_MID_DB, "Mid", 0.0),
                Self::param(PARAM_HIGH_DB, "High", 0.0),
            ],
        }
    }

    pub fn from_node(node: &Node, sample_rate: f32) -> Self {
        let mut e = Self::new(sample_rate);
        for p in &node.params {
            let _ = e.apply_param(&p.id, p.value);
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sr: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|t| (t as f32 * core::f32::consts::TAU * freq / sr).sin())
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn flat_is_bit_identical_and_silent_stays_silent() {
        let mut e = Eq::new(48_000.0);
        let input = sine(1000.0, 48_000.0, 1024);
        let mut out = vec![0.0; 1024];
        e.process(&input, &mut out);
        assert_eq!(input, out);
        let mut z = vec![9.9; 64];
        e.process(&[0.0; 64], &mut z);
        assert!(z.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn mid_boost_lifts_center_and_mid_cut_drops_it() {
        let sr = 48_000.0;
        let input = sine(MID_FREQ, sr, 8192);
        let steady = |v: &[f32]| rms(&v[v.len() / 2..]);
        let dry = {
            let mut e = Eq::new(sr);
            let mut o = vec![0.0; input.len()];
            e.process(&input, &mut o);
            steady(&o)
        };
        let mut e = Eq::new(sr);
        e.apply_param(PARAM_MID_DB, 12.0).unwrap();
        let mut o = vec![0.0; input.len()];
        e.process(&input, &mut o);
        let boosted = steady(&o);
        assert!(boosted > dry * 2.5, "boost {boosted} vs dry {dry}");
        let mut e = Eq::new(sr);
        e.apply_param(PARAM_MID_DB, -12.0).unwrap();
        e.process(&input, &mut o);
        let cut = steady(&o);
        assert!(cut < dry * 0.4, "cut {cut} vs dry {dry}");
    }

    #[test]
    fn low_boost_lifts_bass_not_treble() {
        let sr = 48_000.0;
        let mut e = Eq::new(sr);
        e.apply_param(PARAM_LOW_DB, 12.0).unwrap();
        let bass = sine(55.0, sr, 8192);
        let mut o = vec![0.0; bass.len()];
        e.process(&bass, &mut o);
        assert!(rms(&o[o.len() / 2..]) > rms(&bass[bass.len() / 2..]) * 2.0);
        let air = sine(12_000.0, sr, 8192);
        e.process(&air, &mut o);
        let ratio = rms(&o[o.len() / 2..]) / rms(&air[air.len() / 2..]);
        assert!((ratio - 1.0).abs() < 0.15, "treble untouched, ratio {ratio}");
    }

    #[test]
    fn node_round_trip_and_bad_param() {
        let node = Eq::default_node("dev_eq", "Native EQ");
        let back: Node = serde_json::from_str(&serde_json::to_string(&node).unwrap()).unwrap();
        assert_eq!(node, back);
        assert_eq!(Eq::from_node(&back, 44_100.0).mid_db, 0.0);
        let mut e = Eq::new(48_000.0);
        assert!(e.apply_param("nope", 0.0).is_err());
    }
}
