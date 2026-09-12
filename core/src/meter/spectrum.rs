//! Spectrum analyzer tap: a ring the audio thread fills, spectra the
//! UI thread computes.
//!
//! Teaching note: an FFT in the audio callback is a classic dropout
//! bug — O(N log N) bursts plus allocation, right on the deadline. So
//! the work splits where the budgets split:
//!
//! - Audio thread: [`SharedRing::push_block`] — a `memcpy` into a
//!   pre-sized ring, behind `try_lock`. If the UI currently holds the
//!   lock, the block is *dropped*: a spectrum frame is indicative, and
//!   dropping one beats missing a deadline.
//! - UI thread: [`spectrum_magnitudes`] — a Hann-windowed radix-2 FFT
//!   over the latest window (no dependency: ~60 lines, exact at every
//!   DFT bin), grouped into display bins by maximum. Per-bin Goertzel
//!   was the first attempt and lost: evaluated *between* bin centers a
//!   tone phase-cancels to ~nothing, while max-grouped DFT bins report
//!   its true height. [`dominant_freq`] picks the exact DFT peak.
//!
//! Display bins are linear fractions of Nyquist (bin `b` of `B` covers
//! `b/B..(b+1)/B` of 0 Hz..Nyquist); map them to log bands in the UI
//! (the primer shows how).

use std::sync::Mutex;

/// Latest-N-samples ring shared audio → UI. Pre-sized in
/// [`SharedRing::new`]; neither [`push_block`](SharedRing::push_block)
/// nor [`latest_window`](SharedRing::latest_window) resizes it (the
/// latter allocates its *returned* window — a UI-thread-only call).
#[derive(Debug)]
pub struct SharedRing {
    buf: Vec<f32>,
    write: usize,
    filled: usize,
}

impl SharedRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity.max(1)],
            write: 0,
            filled: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Audio thread: append a block, overwriting the oldest. No
    /// allocation; callers hold the lock with `try_lock` only.
    pub fn push_block(&mut self, samples: &[f32]) {
        let cap = self.buf.len();
        for &s in samples {
            self.buf[self.write] = s;
            self.write = (self.write + 1) % cap;
            self.filled = (self.filled + 1).min(cap);
        }
    }

    pub fn len(&self) -> usize {
        self.filled
    }

    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// UI thread: clone the latest `n` samples (oldest → newest),
    /// zero-padded at the front when the ring holds fewer.
    pub fn latest_window(&self, n: usize) -> Vec<f32> {
        let mut out = vec![0.0; n];
        let take = n.min(self.filled);
        let cap = self.buf.len();
        let start = (self.write + cap - take) % cap;
        for i in 0..take {
            out[n - take + i] = self.buf[(start + i) % cap];
        }
        out
    }
}

/// Largest power of two ≤ `n` (0 below 2 — too short to transform).
fn pow2_le(n: usize) -> usize {
    if n < 2 {
        return 0;
    }
    1 << (usize::BITS - n.leading_zeros() - 1)
}

/// In-place iterative radix-2 FFT. Allocates nothing itself; the
/// caller owns `re`/`im` (a UI-thread-only call).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && n == im.len());
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j &= !bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let (s, c) = (-core::f32::consts::TAU / len as f32).sin_cos();
        let mut k = 0;
        while k < n {
            let (mut w_re, mut w_im) = (1.0f32, 0.0f32);
            for m in 0..len / 2 {
                let (u_re, u_im) = (re[k + m], im[k + m]);
                let (v_re, v_im) = (
                    re[k + m + len / 2] * w_re - im[k + m + len / 2] * w_im,
                    re[k + m + len / 2] * w_im + im[k + m + len / 2] * w_re,
                );
                re[k + m] = u_re + v_re;
                im[k + m] = u_im + v_im;
                re[k + m + len / 2] = u_re - v_re;
                im[k + m + len / 2] = u_im - v_im;
                let t = w_re * c - w_im * s;
                w_im = w_re * s + w_im * c;
                w_re = t;
            }
            k += len;
        }
        len <<= 1;
    }
}

/// Hann-windowed DFT magnitudes for bins `0..N/2`, where `N` is the
/// largest power of two ≤ `samples.len()` (excess tail samples are
/// ignored). Normalized by `N/4`: an on-bin full-scale sine reads
/// ≈ 1.0 (Hann coherent gain is 0.5, single-sided spectrum doubles
/// it back). Empty for windows under 8 samples.
fn dft_magnitudes(samples: &[f32]) -> Vec<f32> {
    let n = pow2_le(samples.len());
    if n < 8 {
        return Vec::new();
    }
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for (i, slot) in re.iter_mut().enumerate() {
        // Hann: 0.5 − 0.5·cos — tames leakage so a tone lands in one
        // or two bins instead of smearing across the spectrum.
        let w = 0.5 - 0.5 * (core::f32::consts::TAU * i as f32 / (n - 1) as f32).cos();
        *slot = samples[i] * w;
    }
    fft(&mut re, &mut im);
    let norm = (n as f32 / 4.0).max(1.0);
    (0..n / 2)
        .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() / norm)
        .collect()
}

/// Display spectrum: the DFT grouped into `bins` linear display bins
/// (bin `b` covers DFT bins `b·K/B..(b+1)·K/B`, maximum wins), so a
/// tone between two DFT bins still reports its true height in its
/// display bin. `sample_rate` only fixes the Hz mapping the UI
/// applies (`center ≈ Nyquist·(b+0.5)/B`); the magnitudes are
/// rate-independent. Allocates: UI thread only.
pub fn spectrum_magnitudes(samples: &[f32], sample_rate: f32, bins: usize) -> Vec<f32> {
    let _ = sample_rate.max(1.0);
    if bins == 0 {
        return Vec::new();
    }
    let dft = dft_magnitudes(samples);
    if dft.is_empty() {
        return vec![0.0; bins];
    }
    let k = dft.len();
    (0..bins)
        .map(|b| {
            let (lo, hi) = (b * k / bins, (b + 1) * k / bins);
            dft[lo..hi.max(lo + 1).min(k)].iter().fold(0.0f32, |a, &v| a.max(v))
        })
        .collect()
}

/// Frequency (Hz) of the strongest DFT bin, or 0.0 on silence / tiny
/// windows. Exact-bin resolution (`k·sr/N`), not display-bin.
pub fn dominant_freq(samples: &[f32], sample_rate: f32) -> f32 {
    let dft = dft_magnitudes(samples);
    if dft.is_empty() {
        return 0.0;
    }
    let (best, peak) = dft
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, &v)| (i, v))
        .unwrap_or((0, 0.0));
    if peak <= 0.0 {
        return 0.0;
    }
    let n = pow2_le(samples.len()) as f32;
    best as f32 * sample_rate.max(1.0) / n
}

/// Convenience: one mutex-wrapped ring with the tap's locking rule —
/// audio pushes with `try_lock` (drops the block when the UI is
/// reading), UI snapshots with `try_lock` (takes "no fresh frame"
/// over "wait for the audio thread").
pub fn push_shared(ring: &Mutex<SharedRing>, samples: &[f32]) {
    if let Ok(mut r) = ring.try_lock() {
        r.push_block(samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|t| amp * (t as f32 * core::f32::consts::TAU * freq / sr).sin())
            .collect()
    }

    #[test]
    fn ring_keeps_latest_and_pads() {
        let mut r = SharedRing::new(8);
        r.push_block(&[1.0, 2.0, 3.0]);
        assert_eq!(r.latest_window(2), vec![2.0, 3.0]);
        assert_eq!(r.latest_window(5), vec![0.0, 0.0, 1.0, 2.0, 3.0]);
        r.push_block(&[4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        assert_eq!(
            r.latest_window(8),
            vec![2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]
        );
    }

    #[test]
    fn fft_finds_on_bin_tone_exactly() {
        // 1500 Hz @ 48 kHz / 4096 is exactly DFT bin 128: the honest
        // anchor — an exactly-representable tone must read its exact
        // amplitude and frequency.
        let sr = 48_000.0;
        let s = sine(1500.0, 0.8, sr, 4096);
        let dft = dft_magnitudes(&s);
        assert_eq!(dft.len(), 2048);
        let best = dft.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert_eq!(best, 128, "peak DFT bin for an on-bin 1500 Hz tone");
        assert!((dft[best] - 0.8).abs() < 0.02, "magnitude {}", dft[best]);
        assert!((dominant_freq(&s, sr) - 1500.0).abs() < 1.0);
    }

    #[test]
    fn tone_peak_lands_in_right_bin() {
        let sr = 48_000.0;
        let s = sine(1000.0, 0.8, sr, 4096);
        let mags = spectrum_magnitudes(&s, sr, 64);
        assert_eq!(mags.len(), 64);
        let best = mags.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        let center = sr / 2.0 * (best as f32 + 0.5) / 64.0;
        assert!(
            (center - 1000.0).abs() / 1000.0 < 0.2,
            "peak bin center {center} Hz for a 1 kHz tone"
        );
        // Off-DFT-bin tones lose ≤ ~1.5 dB to Hann scalloping; the
        // max-grouped display bin must still report true height.
        assert!(
            (mags[best] - 0.8).abs() < 0.25,
            "tone magnitude {} vs amplitude 0.8",
            mags[best]
        );
        assert!((dominant_freq(&s, sr) - 1000.0).abs() < 30.0);
    }

    #[test]
    fn silence_and_empty_are_zero() {
        assert!(
            spectrum_magnitudes(&vec![0.0; 1024], 48_000.0, 32).iter().all(|&m| m == 0.0)
        );
        assert_eq!(dominant_freq(&vec![0.0; 1024], 48_000.0), 0.0);
        assert_eq!(spectrum_magnitudes(&[], 48_000.0, 8), vec![0.0; 8]);
        assert_eq!(spectrum_magnitudes(&[1.0; 3], 48_000.0, 8), vec![0.0; 8]);
    }

    #[test]
    fn bass_and_treble_separate() {
        // 55 Hz vs 12 kHz must peak in different bins — the meter can
        // actually tell bass from air.
        let sr = 48_000.0;
        let peak_bin = |freq: f32| {
            let s = sine(freq, 0.7, sr, 4096);
            let mags = spectrum_magnitudes(&s, sr, 64);
            mags.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0
        };
        let (lo, hi) = (peak_bin(55.0), peak_bin(12_000.0));
        assert!(lo < 4, "bass bin {lo}");
        assert!(hi > 24, "treble bin {hi}");
    }
}
