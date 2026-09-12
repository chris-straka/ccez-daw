//! Latency measurement and hiding for the DSP link.
//!
//! Teaching note: there are two latencies and beginners conflate them.
//! *Per-block latency* (round-trip milliseconds for one block) delays when
//! a block's output is ready; *throughput* (blocks/second) is what keeps
//! audio glitch-free. Pipelining hides the first with the second: with N
//! requests in flight, the server always has work while earlier answers
//! travel back. [`LatencyEstimator`] measures smoothed RTT from Ping/Pong
//! (and per-block turnarounds) and converts it to a prefetch depth —
//! "keep this many blocks ahead so the link never idles".

use std::collections::VecDeque;

/// Smoothed round-trip estimator over the localhost link.
#[derive(Debug, Clone)]
pub struct LatencyEstimator {
    samples: VecDeque<f64>,
    max_samples: usize,
    smoothed_ms: f64,
    /// EWMA weight for new samples (0..1). Higher = faster reaction.
    alpha: f64,
    observed_blocks: u64,
    late_blocks: u64,
}

impl LatencyEstimator {
    pub fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            max_samples: 64,
            smoothed_ms: 0.0,
            alpha: 0.3,
            observed_blocks: 0,
            late_blocks: 0,
        }
    }

    /// Record one round-trip measurement in milliseconds. The first
    /// sample seeds the smoother; later ones blend in via EWMA.
    pub fn observe_rtt_ms(&mut self, rtt_ms: f64) {
        let rtt_ms = rtt_ms.max(0.0);
        if self.samples.is_empty() {
            self.smoothed_ms = rtt_ms;
        } else {
            self.smoothed_ms += self.alpha * (rtt_ms - self.smoothed_ms);
        }
        if self.samples.len() >= self.max_samples {
            self.samples.pop_front();
        }
        self.samples.push_back(rtt_ms);
    }

    /// Record one streamed block's outcome: whether its response arrived
    /// within its deadline (`on_time`). Late blocks raise the prefetch
    /// recommendation via [`Self::recommended_depth`].
    pub fn observe_block(&mut self, on_time: bool) {
        self.observed_blocks += 1;
        if !on_time {
            self.late_blocks += 1;
        }
    }

    /// Smoothed round-trip estimate in milliseconds.
    pub fn smoothed_ms(&self) -> f64 {
        self.smoothed_ms
    }

    /// Worst recent sample — the hiding budget must cover jitter, not the
    /// average.
    pub fn worst_ms(&self) -> f64 {
        self.samples.iter().copied().fold(0.0, f64::max)
    }

    pub fn late_ratio(&self) -> f64 {
        if self.observed_blocks == 0 {
            0.0
        } else {
            self.late_blocks as f64 / self.observed_blocks as f64
        }
    }

    /// How many blocks to keep in flight so the server never idles:
    /// `ceil(worst_rtt / block_ms) + 1`, at least 1. Uses the worst
    /// sample (jitter cover), not the mean.
    pub fn recommended_depth(&self, block_ms: f64) -> usize {
        if block_ms <= 0.0 || self.samples.is_empty() {
            return 1;
        }
        ((self.worst_ms() / block_ms).ceil() as usize + 1).max(1)
    }
}

impl Default for LatencyEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_seeds_smoother() {
        let mut e = LatencyEstimator::new();
        e.observe_rtt_ms(10.0);
        assert_eq!(e.smoothed_ms(), 10.0);
        assert_eq!(e.worst_ms(), 10.0);
    }

    #[test]
    fn ewma_tracks_upward_drift() {
        let mut e = LatencyEstimator::new();
        e.observe_rtt_ms(10.0);
        for _ in 0..20 {
            e.observe_rtt_ms(20.0);
        }
        assert!(e.smoothed_ms() > 15.0, "smoothed={}", e.smoothed_ms());
        assert_eq!(e.worst_ms(), 20.0);
    }

    #[test]
    fn depth_covers_worst_case_plus_one() {
        let mut e = LatencyEstimator::new();
        assert_eq!(e.recommended_depth(5.0), 1); // no data yet
        e.observe_rtt_ms(4.0);
        e.observe_rtt_ms(12.0); // jitter spike
        // worst 12ms over 5ms blocks -> ceil(2.4)+1 = 4.
        assert_eq!(e.recommended_depth(5.0), 4);
    }

    #[test]
    fn late_ratio_counts_misses() {
        let mut e = LatencyEstimator::new();
        e.observe_block(true);
        e.observe_block(false);
        e.observe_block(false);
        assert!((e.late_ratio() - 2.0 / 3.0).abs() < 1e-12);
    }
}
