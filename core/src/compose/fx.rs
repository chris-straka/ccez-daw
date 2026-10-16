//! Per-layer EQ and the mastering pass.
//!
//! Teaching note: EQ here is a chain of RBJ-cookbook biquads (low cut,
//! low shelf, a bell, high shelf, high cut) applied to a layer's stem
//! *before* the loop tail is folded, so it stays a linear filter on the
//! whole render and the loop stays seamless.
//!
//! Mastering is the last step before a listener: remove DC and sub
//! rumble, bring the whole piece to one loudness target (EBU R128
//! integrated LUFS, so every track of a game sits at the same level), and
//! catch the few peaks that would then cross the ceiling with a
//! look-ahead true-peak limiter. For a loop, every filter and the
//! limiter run *circularly* (state primed by a first pass over the loop),
//! so the processed loop still joins end-to-start without a click.

use serde::{Deserialize, Serialize};

use crate::meter::lufs::analyse_offline;
use crate::meter::TruePeakMeter;

#[derive(Debug, Clone, Copy, Default)]
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
    fn new(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, ..Default::default() }
    }
    fn w(f: f32, sr: f32) -> (f32, f32) {
        let w0 = std::f32::consts::TAU * f.clamp(5.0, sr * 0.49) / sr;
        w0.sin_cos()
    }
    fn highpass(f: f32, q: f32, sr: f32) -> Self {
        let (s, c) = Self::w(f, sr);
        let a = s / (2.0 * q);
        Self::new((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + a, -2.0 * c, 1.0 - a)
    }
    fn lowpass(f: f32, q: f32, sr: f32) -> Self {
        let (s, c) = Self::w(f, sr);
        let a = s / (2.0 * q);
        Self::new((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + a, -2.0 * c, 1.0 - a)
    }
    fn peaking(f: f32, q: f32, db: f32, sr: f32) -> Self {
        let (s, c) = Self::w(f, sr);
        let a = 10f32.powf(db / 40.0);
        let al = s / (2.0 * q);
        Self::new(1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a)
    }
    fn shelf(f: f32, db: f32, sr: f32, high: bool) -> Self {
        let (s, c) = Self::w(f, sr);
        let a = 10f32.powf(db / 40.0);
        let al = s / 2.0 * std::f32::consts::SQRT_2;
        let sq = 2.0 * a.sqrt() * al;
        if high {
            Self::new(
                a * ((a + 1.0) + (a - 1.0) * c + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - sq),
                (a + 1.0) - (a - 1.0) * c + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - sq,
            )
        } else {
            Self::new(
                a * ((a + 1.0) - (a - 1.0) * c + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - sq),
                (a + 1.0) + (a - 1.0) * c + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - sq,
            )
        }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// A layer's EQ. Every field is optional; an empty EQ is a bypass.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct LayerEq {
    /// High-pass corner (Hz, 20..=2000): clears low mud from upper layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_cut_hz: Option<f32>,
    /// Low-pass corner (Hz, 500..=20000): darkens, pushes a layer back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_cut_hz: Option<f32>,
    /// Low shelf at 200 Hz (dB, -18..=18): warmth / thinning.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub low_db: f32,
    /// Bell at `mid_hz` (dB, -18..=18, Q 1): presence or a scoop.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub mid_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mid_hz: Option<f32>,
    /// High shelf at 6 kHz (dB, -18..=18): air / softness.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub high_db: f32,
}

fn is_zero(x: &f32) -> bool {
    *x == 0.0
}

impl LayerEq {
    pub fn is_flat(&self) -> bool {
        self.low_cut_hz.is_none() && self.high_cut_hz.is_none() && self.low_db == 0.0 && self.mid_db == 0.0 && self.high_db == 0.0
    }

    pub fn validate(&self, at: &str) -> Vec<String> {
        let mut e = Vec::new();
        if let Some(f) = self.low_cut_hz {
            if !(20.0..=2000.0).contains(&f) {
                e.push(format!("{at}: eq low_cut_hz {f} want 20..=2000"));
            }
        }
        if let Some(f) = self.high_cut_hz {
            if !(500.0..=20000.0).contains(&f) {
                e.push(format!("{at}: eq high_cut_hz {f} want 500..=20000"));
            }
        }
        if let Some(f) = self.mid_hz {
            if !(100.0..=10000.0).contains(&f) {
                e.push(format!("{at}: eq mid_hz {f} want 100..=10000"));
            }
        }
        for (name, v) in [("low_db", self.low_db), ("mid_db", self.mid_db), ("high_db", self.high_db)] {
            if !(-18.0..=18.0).contains(&v) {
                e.push(format!("{at}: eq {name} {v} want -18..=18"));
            }
        }
        e
    }

    fn chain(&self, sr: f32) -> Vec<Biquad> {
        let mut c = Vec::new();
        if let Some(f) = self.low_cut_hz {
            c.push(Biquad::highpass(f, std::f32::consts::FRAC_1_SQRT_2, sr));
        }
        if self.low_db != 0.0 {
            c.push(Biquad::shelf(200.0, self.low_db, sr, false));
        }
        if self.mid_db != 0.0 {
            c.push(Biquad::peaking(self.mid_hz.unwrap_or(1000.0), 1.0, self.mid_db, sr));
        }
        if self.high_db != 0.0 {
            c.push(Biquad::shelf(6000.0, self.high_db, sr, true));
        }
        if let Some(f) = self.high_cut_hz {
            c.push(Biquad::lowpass(f, std::f32::consts::FRAC_1_SQRT_2, sr));
        }
        c
    }

    /// Filter one channel in place (linear: safe before a loop fold).
    pub fn apply(&self, x: &mut [f32], sr: f32) {
        if self.is_flat() {
            return;
        }
        let mut chain = self.chain(sr);
        for v in x.iter_mut() {
            let mut s = *v;
            for b in chain.iter_mut() {
                s = b.run(s);
            }
            *v = s;
        }
    }
}

/// Mastering settings (preview mixes get the full chain; exported stems
/// get the same static gain so the adaptive mix sits at the target).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Master {
    /// Integrated loudness target (LUFS, -30..=-8). -18 suits an in-game
    /// bed under dialogue and effects; -16 a standalone listen.
    #[serde(default = "default_lufs")]
    pub lufs: f32,
    /// True-peak ceiling (dBTP, -6..=0).
    #[serde(default = "default_ceiling")]
    pub ceiling_dbtp: f32,
}

fn default_lufs() -> f32 {
    -16.0
}
fn default_ceiling() -> f32 {
    -1.0
}

impl Default for Master {
    fn default() -> Self {
        Master { lufs: default_lufs(), ceiling_dbtp: default_ceiling() }
    }
}

impl Master {
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        if !(-30.0..=-8.0).contains(&self.lufs) {
            e.push(format!("master: lufs {} want -30..=-8", self.lufs));
        }
        if !(-6.0..=0.0).contains(&self.ceiling_dbtp) {
            e.push(format!("master: ceiling_dbtp {} want -6..=0", self.ceiling_dbtp));
        }
        e
    }
}

/// What the mastering pass did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MasterReport {
    pub engine: String,
    pub lufs_before: Option<f32>,
    pub lufs_after: Option<f32>,
    pub true_peak_before_dbtp: f32,
    pub true_peak_after_dbtp: f32,
    pub gain_db: f32,
    /// Deepest limiter gain reduction (dB, >= 0).
    pub max_reduction_db: f32,
    /// Share of the time the limiter works more than 0.5 dB.
    pub limited_share: f32,
    pub looped: bool,
    /// Loops: wrap-around step after mastering (vs the music's own steps).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seam_step: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seamless: Option<bool>,
}

pub fn true_peak_dbtp(chs: &[&[f32]]) -> f32 {
    let mut p = 0.0f32;
    for ch in chs {
        let mut m = TruePeakMeter::new();
        m.process(ch);
        p = p.max(m.peak());
    }
    20.0 * p.max(1e-9).log10()
}

fn db(x: f32) -> f32 {
    10f32.powf(x / 20.0)
}

/// Run a stateful per-sample filter over a channel; for loops, prime the
/// state with one pass first so the output is periodic.
fn run_filter(x: &mut [f32], looped: bool, mut f: impl FnMut(f32) -> f32) {
    if looped {
        for &v in x.iter() {
            f(v);
        }
    }
    for v in x.iter_mut() {
        *v = f(*v);
    }
}

/// Peak (with Catmull-Rom inter-sample estimate) of both channels at i.
fn peak_env(l: &[f32], r: &[f32], looped: bool) -> Vec<f32> {
    let n = l.len();
    let at = |ch: &[f32], i: isize| -> f32 {
        if looped {
            ch[i.rem_euclid(n as isize) as usize]
        } else {
            ch[i.clamp(0, n as isize - 1) as usize]
        }
    };
    let mut out = vec![0.0f32; n];
    for (i, o) in out.iter_mut().enumerate() {
        let i = i as isize;
        let mut m = 0.0f32;
        for ch in [l, r] {
            let (y0, y1, y2, y3) = (at(ch, i - 1), at(ch, i), at(ch, i + 1), at(ch, i + 2));
            m = m.max(y1.abs());
            for t in [0.25f32, 0.5, 0.75] {
                let v = 0.5
                    * (2.0 * y1
                        + (-y0 + y2) * t
                        + (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3) * t * t
                        + (-y0 + 3.0 * y1 - 3.0 * y2 + y3) * t * t * t);
                m = m.max(v.abs());
            }
        }
        *o = m;
    }
    out
}

/// Look-ahead limiter gain: never above what each sample needs, ramps
/// over the look-ahead, releases over `release_s`.
fn limiter_gain(env: &[f32], ceiling: f32, sr: f32, looped: bool) -> Vec<f32> {
    let n = env.len();
    if n == 0 {
        return Vec::new();
    }
    let la = ((0.0015 * sr) as usize).max(1);
    let need: Vec<f32> = env.iter().map(|&p| if p > ceiling { ceiling / p } else { 1.0 }).collect();
    let idx = |i: usize| if looped { i % n } else { i.min(n - 1) };
    // Forward-looking min over [i, i + la).
    let mut gmin = vec![1.0f32; n];
    for i in 0..n {
        if need[i] < 1.0 {
            for k in 0..la {
                let j = if looped { (i + n - k) % n } else if i >= k { i - k } else { break };
                gmin[j] = gmin[j].min(need[i]);
            }
        }
    }
    let _ = idx;
    // Release toward 1.
    let rel = 1.0 - (-1.0 / (0.08 * sr)).exp();
    let mut g = gmin.clone();
    let passes = if looped { 2 } else { 1 };
    let mut prev = 1.0f32;
    for _ in 0..passes {
        for i in 0..n {
            let r = prev + (1.0 - prev) * rel;
            g[i] = gmin[i].min(r);
            prev = g[i];
        }
    }
    // Box average over the look-ahead (smooth attack that still meets every need).
    let mut out = vec![1.0f32; n];
    let mut acc = 0.0f32;
    for k in 0..la {
        acc += if looped { g[(n - 1 - k + n) % n] } else { 1.0 };
    }
    // acc holds g[i-la..i-1]; slide.
    for i in 0..n {
        let add = g[i];
        let drop = if looped { g[(i + n - la) % n] } else if i >= la { g[i - la] } else { 1.0 };
        acc += add - drop;
        out[i] = (acc / la as f32).min(1.0);
    }
    out
}

/// Master a stereo buffer in place.
pub fn master_stereo(l: &mut [f32], r: &mut [f32], sr: f32, m: &Master, looped: bool) -> MasterReport {
    let tp_before = true_peak_dbtp(&[l, r]);
    let (lufs_before, _, _) = analyse_offline(&[l, r], sr);
    // 1. DC offset, then a 25 Hz rumble high-pass.
    for ch in [&mut *l, &mut *r] {
        let mean = ch.iter().map(|&v| v as f64).sum::<f64>() / ch.len().max(1) as f64;
        for v in ch.iter_mut() {
            *v -= mean as f32;
        }
        let mut hp = Biquad::highpass(25.0, std::f32::consts::FRAC_1_SQRT_2, sr);
        run_filter(ch, looped, |x| hp.run(x));
    }
    // 2. Loudness to target, 3. true-peak limit; twice, so the limiter's
    // small loudness loss is made up.
    let ceiling = db(m.ceiling_dbtp - 0.1);
    let mut total_gain_db = 0.0f32;
    let mut max_red = 0.0f32;
    let mut limited = 0usize;
    for _ in 0..2 {
        let (now, _, _) = analyse_offline(&[l, r], sr);
        let Some(now) = now else { break };
        let g = (m.lufs - now).clamp(-40.0, 40.0);
        if g.abs() < 0.05 && total_gain_db != 0.0 {
            break;
        }
        total_gain_db += g;
        let gl = db(g);
        for v in l.iter_mut().chain(r.iter_mut()) {
            *v *= gl;
        }
        let env = peak_env(l, r, looped);
        let gain = limiter_gain(&env, ceiling, sr, looped);
        limited = 0;
        for (i, gi) in gain.iter().enumerate() {
            l[i] *= gi;
            r[i] *= gi;
            let red = -20.0 * gi.max(1e-6).log10();
            max_red = max_red.max(red);
            if red > 0.5 {
                limited += 1;
            }
        }
    }
    // Safety: anything the estimate missed is trimmed by a static gain.
    let tp = true_peak_dbtp(&[l, r]);
    if tp > m.ceiling_dbtp {
        let g = db(m.ceiling_dbtp - tp);
        for v in l.iter_mut().chain(r.iter_mut()) {
            *v *= g;
        }
        total_gain_db += m.ceiling_dbtp - tp;
    }
    let (lufs_after, _, _) = analyse_offline(&[l, r], sr);
    let (seam_step, seamless) = if looped && l.len() > 1 {
        // Seamless = the wrap is no bigger a jump than the music makes.
        let biggest = l.windows(2).chain(r.windows(2)).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let seam = crate::loopseam::seam_step(l).max(crate::loopseam::seam_step(r));
        (Some(seam), Some(seam <= (biggest * 1.01).max(crate::loopseam::DEFAULT_CLICK_THRESHOLD)))
    } else {
        (None, None)
    };
    MasterReport {
        engine: "ccez builtin (DC, 25 Hz high-pass, R128 loudness, look-ahead true-peak limiter)".into(),
        lufs_before,
        lufs_after,
        true_peak_before_dbtp: tp_before,
        true_peak_after_dbtp: true_peak_dbtp(&[l, r]),
        gain_db: total_gain_db,
        max_reduction_db: max_red,
        limited_share: limited as f32 / l.len().max(1) as f32,
        looped,
        seam_step,
        seamless,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(secs * 48_000.0) as usize)
            .map(|i| amp * (i as f32 * std::f32::consts::TAU * f / 48_000.0).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn eq_cuts_and_shelves_where_asked() {
        let sr = 48_000.0;
        let mut low = tone(80.0, 0.5, 1.0);
        let mut high = tone(5000.0, 0.5, 1.0);
        let eq = LayerEq { low_cut_hz: Some(400.0), ..Default::default() };
        eq.apply(&mut low, sr);
        eq.apply(&mut high, sr);
        assert!(rms(&low[24_000..]) < 0.05 * 0.35, "80 Hz cut: {}", rms(&low[24_000..]));
        assert!((rms(&high[24_000..]) - 0.354).abs() < 0.02, "5 kHz passes");
        let mut air = tone(10_000.0, 0.1, 1.0);
        LayerEq { high_db: 6.0, ..Default::default() }.apply(&mut air, sr);
        let gain = 20.0 * (rms(&air[24_000..]) / 0.0707).log10();
        assert!((gain - 6.0).abs() < 0.8, "high shelf {gain}");
        assert!(LayerEq { low_db: 30.0, ..Default::default() }.validate("layer").len() == 1);
    }

    #[test]
    fn master_hits_the_target_under_the_ceiling() {
        // A quiet tone with one loud spike: loudness up to -16, the spike
        // limited under -1 dBTP.
        let mut l = tone(440.0, 0.05, 8.0);
        for v in &mut l[100_000..100_200] {
            *v *= 18.0;
        }
        let mut r = l.clone();
        let rep = master_stereo(&mut l, &mut r, 48_000.0, &Master::default(), false);
        let after = rep.lufs_after.unwrap();
        assert!((after - -16.0).abs() < 0.6, "{rep:?}");
        assert!(rep.true_peak_after_dbtp <= -0.99, "{rep:?}");
        assert!(rep.max_reduction_db > 1.0, "the spike was limited: {rep:?}");
    }

    #[test]
    fn mastered_loops_stay_seamless() {
        // An exact-period loop (480 Hz = 100 samples) with a loud burst
        // every 0.1 s, pushed hard enough that the bursts need limiting:
        // the wrap must stay as smooth as the music.
        let burst = |mut x: Vec<f32>| {
            for k in (0..x.len()).step_by(4_800) {
                for v in &mut x[k..k + 20] {
                    *v += 0.6;
                }
            }
            x
        };
        let mut l = burst(tone(480.0, 0.1, 2.0));
        let mut r = burst(tone(480.0, 0.08, 2.0));
        let rep = master_stereo(&mut l, &mut r, 48_000.0, &Master { lufs: -10.0, ceiling_dbtp: -1.0 }, true);
        assert!(rep.max_reduction_db > 1.0, "{rep:?}");
        assert_eq!(rep.seamless, Some(true), "{rep:?}");
        assert!(rep.true_peak_after_dbtp <= -0.99, "{rep:?}");
    }
}
