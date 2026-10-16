//! The composer's shared stereo hall: early reflections, then an 8-line
//! feedback delay network (FDN).
//!
//! Teaching note: a hall is a few distinct early echoes off the nearest
//! walls, then a dense tail that decays smoothly. The tail here is eight
//! delay lines of unrelated lengths whose outputs are mixed back into
//! every input through a Hadamard matrix (energy-preserving: nothing
//! rings louder than it went in), each line scaled so it loses 60 dB in
//! `decay_secs`, with a low-pass inside the loop so highs die first (real
//! air and walls do that). Left feeds half the lines and right the other
//! half, and the outputs are tapped with different signs per side, so the
//! tail is wide and decorrelated. It is linear and time-invariant, which
//! is what lets [`super::render`] fold the tail of a loop onto its start
//! and get exactly the steady-state second pass.

use serde::{Deserialize, Serialize};

/// The room every instrument of a piece shares (composition `room`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Room {
    /// RT60: seconds for the tail to fall 60 dB, [0.3, 8].
    #[serde(default = "decay")]
    pub decay_secs: f32,
    /// Gap before the reverb starts (ms), [0, 120].
    #[serde(default = "predelay")]
    pub predelay_ms: f32,
    /// 0 = bright stone, 1 = dark wood and curtains.
    #[serde(default = "damping")]
    pub damping: f32,
}

fn decay() -> f32 {
    2.4
}
fn predelay() -> f32 {
    22.0
}
fn damping() -> f32 {
    0.45
}

impl Default for Room {
    fn default() -> Self {
        Room {
            decay_secs: decay(),
            predelay_ms: predelay(),
            damping: damping(),
        }
    }
}

impl Room {
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        if !(0.3..=8.0).contains(&self.decay_secs) {
            e.push(format!("room.decay_secs {} want 0.3..=8", self.decay_secs));
        }
        if !(0.0..=120.0).contains(&self.predelay_ms) {
            e.push(format!("room.predelay_ms {} want 0..=120", self.predelay_ms));
        }
        if !(0.0..=1.0).contains(&self.damping) {
            e.push(format!("room.damping {} want 0..=1", self.damping));
        }
        e
    }
}

/// Delay-line lengths at 48 kHz (mutually prime, 29-71 ms).
const LINES: [usize; 8] = [1399, 1747, 2083, 2381, 2711, 2953, 3229, 3413];
/// Early reflections: (ms, gain, side) — side 0 = left, 1 = right.
const EARLY: [(f32, f32, usize); 8] = [
    (7.0, 0.42, 0),
    (11.0, 0.38, 1),
    (17.0, 0.30, 1),
    (23.0, 0.27, 0),
    (31.0, 0.22, 0),
    (37.0, 0.20, 1),
    (47.0, 0.15, 1),
    (53.0, 0.13, 0),
];

fn hadamard8(x: &mut [f32; 8]) {
    let mut h = 1;
    while h < 8 {
        for i in (0..8).step_by(h * 2) {
            for j in i..i + h {
                let (a, b) = (x[j], x[j + h]);
                x[j] = a + b;
                x[j + h] = a - b;
            }
        }
        h *= 2;
    }
    let s = 1.0 / 8f32.sqrt();
    for v in x.iter_mut() {
        *v *= s;
    }
}

/// Wet-only stereo reverb of `(l, r)` (same length out; leave room at the
/// end of the input for the tail).
pub fn process(l: &[f32], r: &[f32], sr: u32, room: &Room) -> (Vec<f32>, Vec<f32>) {
    let n = l.len().min(r.len());
    let scale = sr as f32 / 48_000.0;
    let pre = (room.predelay_ms * 0.001 * sr as f32) as usize;
    // Input band-limit: no mud below ~180 Hz, no fizz above ~9 kHz.
    let hp = (-std::f32::consts::TAU * 180.0 / sr as f32).exp();
    let lp = 1.0 - (-std::f32::consts::TAU * 9_000.0 / sr as f32).exp();
    let mut inp = [vec![0.0f32; n], vec![0.0f32; n]];
    for (c, src) in [l, r].iter().enumerate() {
        let (mut hp_x, mut hp_y, mut lp_y) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let x = src[i];
            hp_y = hp * (hp_y + x - hp_x);
            hp_x = x;
            lp_y += lp * (hp_y - lp_y);
            if i + pre < n {
                inp[c][i + pre] = lp_y;
            }
        }
    }
    let mut out_l = vec![0.0f32; n];
    let mut out_r = vec![0.0f32; n];
    // Early reflections (each side also hears a little of the other).
    for &(ms, g, side) in &EARLY {
        let d = (ms * 0.001 * sr as f32) as usize;
        let (own, other) = if side == 0 { (&inp[0], &inp[1]) } else { (&inp[1], &inp[0]) };
        let out = if side == 0 { &mut out_l } else { &mut out_r };
        for i in d..n {
            out[i] += g * (0.8 * own[i - d] + 0.2 * other[i - d]);
        }
    }
    // FDN tail.
    let lens: Vec<usize> = LINES.iter().map(|&d| ((d as f32 * scale) as usize).max(8)).collect();
    let mut bufs: Vec<Vec<f32>> = lens.iter().map(|&d| vec![0.0; d]).collect();
    let mut pos = [0usize; 8];
    let gains: Vec<f32> = lens
        .iter()
        .map(|&d| 10f32.powf(-3.0 * d as f32 / (room.decay_secs * sr as f32)))
        .collect();
    let damp = room.damping.clamp(0.0, 1.0) * 0.7;
    let mut lpf = [0.0f32; 8];
    let tail_gain = 0.55;
    for i in 0..n {
        let mut x = [0.0f32; 8];
        for k in 0..8 {
            x[k] = bufs[k][pos[k]];
        }
        let (mut yl, mut yr) = (0.0, 0.0);
        for (k, v) in x.iter().enumerate() {
            let sign = if k % 3 == 0 { -1.0 } else { 1.0 };
            if k % 2 == 0 {
                yl += v * sign;
            } else {
                yr += v * sign;
            }
        }
        out_l[i] += yl * tail_gain;
        out_r[i] += yr * tail_gain;
        hadamard8(&mut x);
        let (il, ir) = (inp[0][i], inp[1][i]);
        for k in 0..8 {
            lpf[k] = x[k] * (1.0 - damp) + lpf[k] * damp;
            let feed = if k < 4 { il } else { ir };
            bufs[k][pos[k]] = lpf[k] * gains[k] + feed * 0.5;
            pos[k] += 1;
            if pos[k] == lens[k] {
                pos[k] = 0;
            }
        }
    }
    (out_l, out_r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn impulse_decays_like_its_rt60_and_is_wide() {
        let sr = 48_000;
        let mut l = vec![0.0f32; sr as usize * 4];
        l[0] = 1.0;
        let r = l.clone();
        let room = Room {
            decay_secs: 1.5,
            ..Room::default()
        };
        let (wl, wr) = process(&l, &r, sr, &room);
        let early = rms(&wl[2_400..24_000]);
        let late = rms(&wl[(1.5 * sr as f32) as usize..(1.7 * sr as f32) as usize]);
        assert!(early > 1e-3, "{early}");
        // Roughly -60 dB (allow a broad band: damping and early energy).
        let drop = 20.0 * (late / early).log10();
        assert!((-75.0..-40.0).contains(&drop), "drop {drop} dB");
        assert!(wl.iter().all(|v| v.is_finite()));
        // Same input both sides, yet the tails differ (decorrelated).
        let diff: f32 = wl.iter().zip(&wr).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 0.1, "{diff}");
        assert!(Room::default().validate().is_empty());
        assert_eq!(Room { decay_secs: 20.0, ..Room::default() }.validate().len(), 1);
    }
}
