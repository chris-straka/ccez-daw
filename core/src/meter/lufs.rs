//! BS.1770-style loudness: momentary (400 ms), short-term (3 s),
//! integrated (gated mean of 400 ms blocks, 75 % overlap).
//!
//! Teaching note: LUFS answers "how loud does this *feel*?", not "how
//! big are the samples?". Three ideas carry the whole standard:
//!
//! 1. **K-weighting**: two biquads (a +4 dB highshelf above ~1.7 kHz,
//!    then a 60 Hz highpass) model the head/ear. Filtering first means a
//!    dull rumble and a bright hiss with identical RMS read differently.
//! 2. **Squaring + rectangular windows**: loudness is mean-square energy
//!    over a window — 400 ms (momentary, a syllable) and 3 s
//!    (short-term, a phrase). Rectangular (not exponential) so two
//!    implementations agree sample for sample.
//! 3. **Gating for the integrated number**: chop the whole track into
//!    400 ms blocks hopping every 100 ms, drop blocks below −70 LUFS
//!    (silence must not drag the number down), average the rest, drop
//!    blocks 10 LU below that average (a quiet intro must not either),
//!    and average what survives. That is the whole "gated" mystery.
//!
//! Realtime shape: all storage is pre-sized in [`LufsMeter::new`] (the
//! 3 s energy ring plus room for 3 h of gated blocks). [`LufsMeter::process`]
//! therefore never allocates; [`LufsMeter::set_sample_rate`] reallocates
//! and is a UI-thread-only call, like swapping the render graph.

/// Offset from the BS.1770 calibration (a full-scale 1 kHz sine lands at
/// −3.01 LUFS *before* K-weighting shifts it slightly).
pub const CALIBRATION_OFFSET_DB: f32 = -0.691;
/// Absolute gate: blocks quieter than this never count.
pub const ABSOLUTE_GATE_LUFS: f32 = -70.0;
/// Relative gate: blocks this far below the ungated mean never count.
pub const RELATIVE_GATE_LU: f32 = -10.0;
/// Momentary window in seconds.
pub const MOMENTARY_SECS: f32 = 0.4;
/// Short-term window in seconds.
pub const SHORT_TERM_SECS: f32 = 3.0;
/// Gated-block length / hop in seconds (400 ms blocks, 100 ms hop).
pub const BLOCK_SECS: f32 = 0.4;
pub const HOP_SECS: f32 = 0.1;
/// Longest session the integrated store covers before it saturates
/// (3 h at 10 hops/s). Past that, new blocks are ignored — call
/// [`LufsMeter::reset`] for the next side of the album.
pub const MAX_BLOCKS: usize = 108_000;

/// Loudness of mean-square `ms`: `−0.691 + 10·log10(ms)`.
/// Zero energy has no finite loudness → `NEG_INFINITY` (silence).
pub fn lufs_of(ms: f32) -> f32 {
    if ms <= 0.0 {
        return f32::NEG_INFINITY;
    }
    CALIBRATION_OFFSET_DB + 10.0 * ms.log10()
}

/// One Direct-Form-I biquad. Scalar state — no allocation.
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

    /// RBJ highshelf with slope S (the K pre-filter shelf).
    fn highshelf(&mut self, f0: f32, gain_db: f32, s: f32, sr: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = core::f32::consts::TAU * f0 / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
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

    /// RBJ highpass (the K RLB filter).
    fn highpass(&mut self, f0: f32, q: f32, sr: f32) {
        let w0 = core::f32::consts::TAU * f0 / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q.max(0.05));
        self.set(
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
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

/// K-weighting: shelf then highpass, retuned per sample rate.
#[derive(Debug, Clone)]
struct KWeight {
    shelf: Biquad,
    hp: Biquad,
}

impl KWeight {
    fn design(sample_rate: f32) -> Self {
        let mut shelf = Biquad::default();
        shelf.highshelf(1681.97444901, 3.98642805, 0.9, sample_rate);
        let mut hp = Biquad::default();
        hp.highpass(60.0, 0.5, sample_rate);
        Self { shelf, hp }
    }

    fn run(&mut self, x: f32) -> f32 {
        self.hp.run(self.shelf.run(x))
    }
}

/// BS.1770-style loudness meter (mono).
///
/// Feed it blocks with [`LufsMeter::process`] on the audio thread, read
/// [`LufsMeter::momentary_lufs`] / [`short_term_lufs`](LufsMeter::short_term_lufs) /
/// [`integrated_lufs`](LufsMeter::integrated_lufs) anywhere. The window
/// getters divide by the samples seen so far until their window fills —
/// a meter that just started is approximate, not stuck.
#[derive(Debug, Clone)]
pub struct LufsMeter {
    sample_rate: f32,
    k: KWeight,
    /// Ring of per-sample K-weighted energies (capacity = 3 s).
    ring: Vec<f32>,
    head: usize,
    /// Total samples seen (unbounded). Each window starts evicting once
    /// *it* fills — the 400 ms window must not wait for the 3 s ring.
    count: u64,
    sum_momentary: f64,
    sum_short_term: f64,
    momentary_len: usize,
    short_term_len: usize,
    /// Gating: the stream is cut into 100 ms hops; each 400 ms block is
    /// the mean of its (up to) 4 latest hops.
    hop_len: usize,
    hop_acc: f64,
    hop_count: usize,
    hops: [f64; 4],
    hops_head: usize,
    hops_filled: usize,
    blocks: Vec<f32>,
}

impl LufsMeter {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let short_term_len = (SHORT_TERM_SECS * sr).round() as usize;
        let mut m = Self {
            sample_rate: sr,
            k: KWeight::design(sr),
            ring: vec![0.0; short_term_len.max(1)],
            head: 0,
            count: 0,
            sum_momentary: 0.0,
            sum_short_term: 0.0,
            momentary_len: (MOMENTARY_SECS * sr).round() as usize,
            short_term_len,
            hop_len: (HOP_SECS * sr).round().max(1.0) as usize,
            hop_acc: 0.0,
            hop_count: 0,
            hops: [0.0; 4],
            hops_head: 0,
            hops_filled: 0,
            blocks: Vec::new(),
        };
        m.blocks.reserve(MAX_BLOCKS);
        m
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Rebuild rings/filters for a new rate. Allocates: UI thread only
    /// (same rule as swapping the render graph).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Self::new(sample_rate);
    }

    pub fn reset(&mut self) {
        let sr = self.sample_rate;
        *self = Self::new(sr);
    }

    /// Fold one block into the windows and the gated-block store.
    /// Realtime-safe: no allocation, no locks — only filter state, ring
    /// writes, and running sums.
    pub fn process(&mut self, samples: &[f32]) {
        let cap = self.ring.len();
        debug_assert!(self.momentary_len <= cap);
        for &s in samples {
            let y = self.k.run(s);
            let e = (y as f64) * (y as f64);
            // Each window evicts once IT is full. (An early version gated
            // both on the 3 s ring, so momentary carried 7.5x too many
            // samples — a test with steady tone pins both windows equal.)
            if self.count >= self.momentary_len as u64 {
                let out = self.ring[(self.head + cap - self.momentary_len) % cap] as f64;
                self.sum_momentary -= out;
            }
            if self.count >= cap as u64 {
                self.sum_short_term -= self.ring[self.head] as f64;
            }
            self.ring[self.head] = e as f32;
            self.head = (self.head + 1) % cap;
            self.count += 1;
            self.sum_momentary += e;
            self.sum_short_term += e;
            self.push_gating(e);
        }
    }

    fn push_gating(&mut self, e: f64) {
        self.hop_acc += e;
        self.hop_count += 1;
        if self.hop_count < self.hop_len {
            return;
        }
        let hop_ms = self.hop_acc / self.hop_count as f64;
        self.hop_acc = 0.0;
        self.hop_count = 0;
        self.hops[self.hops_head] = hop_ms;
        self.hops_head = (self.hops_head + 1) % 4;
        self.hops_filled = (self.hops_filled + 1).min(4);
        let mut acc = 0.0;
        for i in 0..self.hops_filled {
            acc += self.hops[(self.hops_head + 4 - 1 - i) % 4];
        }
        let block_ms = acc / self.hops_filled as f64;
        if self.blocks.len() < MAX_BLOCKS {
            self.blocks.push(block_ms as f32);
        }
    }

    fn window_lufs(&self, sum: f64, window: usize) -> f32 {
        let n = self.count.min(window as u64).max(1) as f64;
        lufs_of((sum / n) as f32)
    }

    /// Loudness over the last 400 ms (−inf on silence).
    pub fn momentary_lufs(&self) -> f32 {
        self.window_lufs(self.sum_momentary, self.momentary_len)
    }

    /// Loudness over the last 3 s (−inf on silence).
    pub fn short_term_lufs(&self) -> f32 {
        self.window_lufs(self.sum_short_term, self.short_term_len.max(1))
    }

    /// Gated integrated loudness, or `None` when every block sits below
    /// the −70 LUFS absolute gate (silence / nothing observed yet).
    pub fn integrated_lufs(&self) -> Option<f32> {
        let abs_gate_ms = 10.0f32.powf((ABSOLUTE_GATE_LUFS - CALIBRATION_OFFSET_DB) / 10.0);
        let above: Vec<f32> = self.blocks.iter().copied().filter(|&z| z > abs_gate_ms).collect();
        if above.is_empty() {
            return None;
        }
        let mean = above.iter().sum::<f32>() / above.len() as f32;
        let rel_gate_ms = mean * 10.0f32.powf(RELATIVE_GATE_LU / 10.0);
        let gated: Vec<f32> = above.into_iter().filter(|&z| z > rel_gate_ms).collect();
        if gated.is_empty() {
            return None;
        }
        Some(lufs_of(gated.iter().sum::<f32>() / gated.len() as f32))
    }

    /// Gated 400 ms blocks observed so far (for tests / UI histograms).
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, sr: f32, secs: f32) -> Vec<f32> {
        let n = (secs * sr) as usize;
        (0..n)
            .map(|t| amp * (t as f32 * core::f32::consts::TAU * freq / sr).sin())
            .collect()
    }

    fn feed(m: &mut LufsMeter, samples: &[f32]) {
        for chunk in samples.chunks(512) {
            m.process(chunk);
        }
    }

    #[test]
    fn silence_is_gated_out() {
        let mut m = LufsMeter::new(48_000.0);
        feed(&mut m, &vec![0.0; 48_000]);
        assert_eq!(m.momentary_lufs(), f32::NEG_INFINITY);
        assert_eq!(m.short_term_lufs(), f32::NEG_INFINITY);
        assert_eq!(m.integrated_lufs(), None);
    }

    #[test]
    fn full_scale_sine_lands_near_minus_three() {
        // 0 dBFS sine: RMS = −3.01 dBFS; K-weighting at 1 kHz is ≈ 0 dB
        // by design, so integrated must sit within a dB of −3 LUFS.
        let mut m = LufsMeter::new(48_000.0);
        feed(&mut m, &sine(1000.0, 1.0, 48_000.0, 5.0));
        let integrated = m.integrated_lufs().expect("tone is far above the gate");
        assert!(
            (integrated + 3.0).abs() < 1.0,
            "full-scale 1 kHz sine read {integrated} LUFS, want ≈ −3"
        );
    }

    #[test]
    fn minus_six_db_in_is_six_lu_down() {
        // The filter chain is linear: level steps must map 1:1 to LU
        // steps, independent of the absolute calibration.
        let level = |amp: f32| {
            let mut m = LufsMeter::new(48_000.0);
            feed(&mut m, &sine(1000.0, amp, 48_000.0, 5.0));
            m.integrated_lufs().expect("tone above gate")
        };
        let loud = level(1.0);
        let quiet = level(0.5);
        assert!(
            ((loud - quiet) - 6.0206).abs() < 0.15,
            "−6 dBFS step read {loud} → {quiet} LUFS, want exactly 6.02 LU apart"
        );
    }

    #[test]
    fn momentary_and_short_term_converge_on_steady_tone() {
        let mut m = LufsMeter::new(48_000.0);
        feed(&mut m, &sine(1000.0, 0.5, 48_000.0, 6.0));
        let (mom, st, integ) = (
            m.momentary_lufs(),
            m.short_term_lufs(),
            m.integrated_lufs().expect("tone above gate"),
        );
        assert!((mom - integ).abs() < 0.6, "momentary {mom} vs integrated {integ}");
        assert!((st - integ).abs() < 0.6, "short-term {st} vs integrated {integ}");
    }

    #[test]
    fn dc_does_not_count() {
        // The 60 Hz highpass eats DC — but only in steady state: a hard
        // 0→1 step rings the filter, so ramp in over 2 s (no step, no
        // ring) and hold +1.0 for 8 s. Unfiltered that DC would read
        // −0.69 LUFS; through the RLB stage it must (nearly) gate out.
        let sr = 48_000.0;
        let ramp = (0..(2.0 * sr) as usize).map(|t| t as f32 / (2.0 * sr));
        let hold = std::iter::repeat(1.0).take((8.0 * sr) as usize);
        let signal: Vec<f32> = ramp.chain(hold).collect();
        let mut m = LufsMeter::new(sr);
        feed(&mut m, &signal);
        let integ = m.integrated_lufs();
        assert!(
            integ.is_none() || integ.unwrap() < -50.0,
            "DC must (nearly) gate out, read {integ:?}"
        );
    }

    #[test]
    fn reset_clears_windows_and_blocks() {
        let mut m = LufsMeter::new(48_000.0);
        feed(&mut m, &sine(1000.0, 1.0, 48_000.0, 2.0));
        assert!(m.integrated_lufs().is_some());
        m.reset();
        assert_eq!(m.integrated_lufs(), None);
        assert_eq!(m.momentary_lufs(), f32::NEG_INFINITY);
        assert_eq!(m.block_count(), 0);
    }
}
