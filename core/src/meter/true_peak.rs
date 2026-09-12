//! 4x-oversampled true peak: the peak *between* the samples.
//!
//! Teaching note: a DAC does not play your samples — it reconstructs a
//! smooth wave *through* them, and that wave routinely overshoots the
//! samples (up to +3 dB on hot masters). A sample-peak meter therefore
//! lies right where it hurts: at the ceiling. True peak re-samples the
//! wave 4x denser (BS.1770's oversampling factor) with Catmull-Rom cubic
//! interpolation and keeps the biggest absolute value it sees.
//!
//! Realtime shape: [`TruePeakMeter`] holds 3 history samples plus one
//! running maximum. [`TruePeakMeter::process`] is branch-light scalar
//! math — no allocation, no locks.

/// Oversampling factor (BS.1770 § "true peak" = 4x).
pub const OVERSAMPLE: usize = 4;

/// True-peak meter (mono). Feed blocks on the audio thread; read
/// [`TruePeakMeter::peak`] / [`peak_dbtp`](TruePeakMeter::peak_dbtp)
/// anywhere.
#[derive(Debug, Clone)]
pub struct TruePeakMeter {
    /// Last 3 samples of the previous block (cubic needs x[-1..2]).
    hist: [f32; 3],
    peak: f32,
    /// Samples seen so far. The very first sample seeds the history —
    /// otherwise a stream opening on a constant would read a phantom
    /// 0 → value step (and its cubic overshoot) as the peak.
    total: u64,
}

impl TruePeakMeter {
    pub fn new() -> Self {
        Self {
            hist: [0.0; 3],
            peak: 0.0,
            total: 0,
        }
    }

    pub fn reset(&mut self) {
        self.hist = [0.0; 3];
        self.peak = 0.0;
        self.total = 0;
    }

    /// Fold one block in. Realtime-safe: no allocation, no locks.
    pub fn process(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if self.total == 0 {
            self.hist = [samples[0]; 3];
        }
        self.total += samples.len() as u64;
        // Chain: hist[0..3] then the new block, so the cubic sees a
        // continuous stream across block boundaries.
        let h = self.hist;
        let n = samples.len();
        let at = |i: isize| -> f32 {
            if i < 0 {
                h[(3 + i) as usize]
            } else if (i as usize) < n {
                samples[i as usize]
            } else {
                samples[n - 1]
            }
        };
        // Start one sample before the block so the interval
        // (hist[2] → samples[0]) is interpolated too.
        for i in -1..n as isize {
            let (x0, x1, x2, x3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            let m = interval_peak(x0, x1, x2, x3);
            if m > self.peak {
                self.peak = m;
            }
        }
        // Roll the history forward: the last (up to) 3 samples of the
        // continuous stream, so the next block's cubic starts seamless.
        if n >= 3 {
            self.hist = [samples[n - 3], samples[n - 2], samples[n - 1]];
        } else {
            let mut next = self.hist;
            for (i, slot) in next.iter_mut().enumerate() {
                *slot = if i + n >= 3 { samples[i + n - 3] } else { h[i + n] };
            }
            self.hist = next;
        }
    }

    /// Biggest absolute (possibly inter-sample) value seen, linear.
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// Same in dBTP. Silence → `NEG_INFINITY`.
    pub fn peak_dbtp(&self) -> f32 {
        if self.peak <= 0.0 {
            return f32::NEG_INFINITY;
        }
        20.0 * self.peak.log10()
    }
}

impl Default for TruePeakMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Max |y| of the Catmull-Rom cubic through x0..x3 over t in [0, 1),
/// including the left endpoint (so plain sample peaks are covered).
fn interval_peak(x0: f32, x1: f32, x2: f32, x3: f32) -> f32 {
    let mut m = x1.abs();
    for k in 1..OVERSAMPLE {
        let t = k as f32 / OVERSAMPLE as f32;
        let t2 = t * t;
        let t3 = t2 * t;
        let y = 0.5
            * ((2.0 * x1)
                + (-x0 + x2) * t
                + (2.0 * x0 - 5.0 * x1 + 4.0 * x2 - x3) * t2
                + (-x0 + 3.0 * x1 - 3.0 * x2 + x3) * t3);
        let a = y.abs();
        if a > m {
            m = a;
        }
    }
    m
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

    fn sample_peak(v: &[f32]) -> f32 {
        v.iter().map(|s| s.abs()).fold(0.0, f32::max)
    }

    #[test]
    fn silence_stays_zero() {
        let mut t = TruePeakMeter::new();
        t.process(&vec![0.0; 4096]);
        assert_eq!(t.peak(), 0.0);
        assert_eq!(t.peak_dbtp(), f32::NEG_INFINITY);
    }

    #[test]
    fn sine_peak_matches_amplitude() {
        // A 1 kHz tone is densely sampled: true peak ≈ amplitude.
        let mut t = TruePeakMeter::new();
        let s = sine(1000.0, 0.89, 48_000.0, 1.0);
        for chunk in s.chunks(512) {
            t.process(chunk);
        }
        assert!(
            (t.peak() - 0.89).abs() < 0.02,
            "true peak {} vs amplitude 0.89",
            t.peak()
        );
        assert!((t.peak_dbtp() + 1.01).abs() < 0.25, "dBTP {}", t.peak_dbtp());
    }

    #[test]
    fn oversampled_peak_never_below_sample_peak() {
        // Near Nyquist the wave overshoots the samples badly — the
        // whole reason this meter exists. True peak must catch at
        // least the sample peak, and strictly more on this signal.
        let sr = 48_000.0;
        let s = sine(11_000.0, 0.9, sr, 1.0);
        let mut t = TruePeakMeter::new();
        for chunk in s.chunks(211) {
            // Odd chunk sizes: block seams must not lose peaks either.
            t.process(chunk);
        }
        let sp = sample_peak(&s);
        assert!(t.peak() >= sp, "true {} < sample {sp}", t.peak());
        assert!(
            t.peak() > sp + 1e-4,
            "expected inter-sample overshoot above {sp}, got {}",
            t.peak()
        );
    }

    #[test]
    fn constant_and_reset() {
        let mut t = TruePeakMeter::new();
        t.process(&[0.5; 1024]);
        assert!((t.peak() - 0.5).abs() < 1e-6, "constant {}", t.peak());
        t.process(&[]);
        assert_eq!(t.peak(), 0.5, "empty block is a no-op");
        t.reset();
        assert_eq!(t.peak(), 0.0);
    }
}
