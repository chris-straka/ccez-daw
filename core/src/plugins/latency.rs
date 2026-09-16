//! Honest plugin latency compensation (PDC): per-device latency samples,
//! delay-compensated render helpers, and the documented model.
//!
//! Teaching note: every plugin that does DSP with memory (lookahead
//! limiters, linear-phase EQs, FFT processors, oversampled clippers)
//! outputs *late* — its block N answers input block N minus L samples.
//! A DAW that ignores L hears two defects: the wet path lags the dry path
//! inside one insert (comb filtering on a 50/50 mix), and the processed
//! track lags every other track (flams on parallel drums). The fix is old
//! DSP bookkeeping, not cleverness: delay every *faster* path so it
//! arrives with the slowest one.
//!
//! Compensation model (what this module implements):
//!
//! 1. **Report.** Each device reports its latency in samples at the
//!    current sample rate ([`LatencyMap::report`]). Live values arrive via
//!    `SandboxedPlugin::latency_samples` / `PluginHost::latency_samples`
//!    (the worker's `GetLatency` op; the mock reports 0, real CLAP/VST3/AU
//!    backends query their APIs). A report is just data — it changes no
//!    audio until the render path reads it.
//! 2. **Insert level.** Inside one insert, the dry path is delayed by the
//!    device's latency ([`DelayLine`] + [`align_dry`]) so `wet + dry`
//!    mixes sample-aligned. The wet path is the plugin's own output and
//!    is never shifted by this module.
//! 3. **Track level.** Each chain's total latency is the sum of its
//!    devices ([`LatencyMap::chain_latency`]); each track is then delayed
//!    by `session_max - chain_total` ([`LatencyMap::alignment_delay`]) so
//!    all tracks land together.
//!
//! Honest limits (read before promising sample-accuracy):
//!
//! - Latency is a whole number of samples; sub-sample phase is untouched.
//! - A latency *change* (new report) requires flushing the affected
//!   [`DelayLine`]s — the first `latency` samples after a change are a
//!   documented glitch window, not silent corruption. See [`DelayLine::reset`].
//! - Reports above [`MAX_LATENCY_SAMPLES`] are rejected: no real insert
//!   needs more than ~21 s at 48 kHz, and an unbounded delay line is a
//!   memory bug wearing a feature hat.
//! - This module never talks to plugins itself: it owns the math and the
//!   delay memory. Sampling the live graph into a [`LatencyMap`] is one
//!   loop over [`PluginHost`](crate::plugins::host::PluginHost) that the
//!   caller writes (two lines; see [`LatencyMap`] docs).
//!
//! Reads no frozen types and adds no IPC or project-schema surface, so the
//! typegen drift gate (`bun run typegen -- --check`) is unaffected.

use std::collections::{BTreeMap, VecDeque};

/// Upper bound for a single device report (~2^20 samples ≈ 21.8 s at
/// 48 kHz). Anything larger is a units bug at the source, not latency.
pub const MAX_LATENCY_SAMPLES: u32 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatencyError {
    /// Report exceeds [`MAX_LATENCY_SAMPLES`].
    Excessive { device: String, samples: u32 },
}

impl std::fmt::Display for LatencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Excessive { device, samples } => write!(
                f,
                "device `{device}` reports {samples} samples latency (cap {MAX_LATENCY_SAMPLES})"
            ),
        }
    }
}

impl std::error::Error for LatencyError {}

pub type Result<T> = std::result::Result<T, LatencyError>;

/// Per-device latency table: device id → latency in samples at the
/// current sample rate. All-zero (or missing) means "no compensation".
///
/// Sampling the live graph is one loop over the host:
///
/// ```ignore
/// let mut map = LatencyMap::new();
/// for id in host.ids() {
///     map.report(&id, host.latency_samples(&id)?)?;
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatencyMap {
    samples: BTreeMap<String, u32>,
}

impl LatencyMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one device's latency. Rejects reports above
    /// [`MAX_LATENCY_SAMPLES`]; `0` clears any prior compensation delay
    /// (callers must still [`DelayLine::reset`] the live line).
    pub fn report(&mut self, device_id: &str, latency_samples: u32) -> Result<()> {
        if latency_samples > MAX_LATENCY_SAMPLES {
            return Err(LatencyError::Excessive {
                device: device_id.to_string(),
                samples: latency_samples,
            });
        }
        if latency_samples == 0 {
            self.samples.remove(device_id);
        } else {
            self.samples.insert(device_id.to_string(), latency_samples);
        }
        Ok(())
    }

    /// Forget one device (unloaded plugin). Never an error.
    pub fn forget(&mut self, device_id: &str) {
        self.samples.remove(device_id);
    }

    /// Forget everything (sample-rate change invalidates all reports).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Reported latency of one device; `0` when unreported.
    pub fn latency_of(&self, device_id: &str) -> u32 {
        self.samples.get(device_id).copied().unwrap_or(0)
    }

    /// Number of devices with a nonzero report.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Total insert latency of one chain: the saturating sum of its
    /// devices' reports in chain order. Unknown ids contribute 0 (the
    /// graph renders them as pass-through, which is latency-free).
    pub fn chain_latency(&self, chain_order: &[String]) -> u64 {
        chain_order
            .iter()
            .map(|id| u64::from(self.latency_of(id)))
            .fold(0u64, |a, b| a.saturating_add(b))
    }

    /// Track-level alignment delay: how many samples of extra delay this
    /// chain needs so it lands with the slowest chain in the session
    /// (`session_max - chain_total`, saturating at 0). Feed the result
    /// into a [`DelayLine`] on the track's output.
    pub fn alignment_delay(&self, chain_order: &[String], session_max_samples: u64) -> u64 {
        session_max_samples.saturating_sub(self.chain_latency(chain_order))
    }

    /// Slowest chain total across several chains (the `session_max` for
    /// [`LatencyMap::alignment_delay`]).
    pub fn session_max<'a, I>(&self, chains: I) -> u64
    where
        I: IntoIterator<Item = &'a [String]>,
    {
        chains
            .into_iter()
            .map(|c| self.chain_latency(c))
            .fold(0u64, u64::max)
    }
}

/// Fixed-sample delay line: the memory behind every compensation delay.
///
/// Push a block in, get the block from `delay` samples ago out. The first
/// `delay` samples out are zeros (the documented pre-roll: compensation
/// adds latency to the fast paths, it never time-travels the slow one).
/// State is plain `f32`s, so a line snapshots as a `Vec<f32>` when a
/// caller needs it to.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayLine {
    delay: usize,
    buf: VecDeque<f32>,
}

impl DelayLine {
    /// New line delaying by `delay` samples. Starts silent (pre-roll of
    /// `delay` zeros) — the honest initial condition, not a guess.
    pub fn new(delay: usize) -> Self {
        Self {
            delay,
            buf: VecDeque::with_capacity(delay),
        }
    }

    pub fn delay(&self) -> usize {
        self.delay
    }

    /// Retune to a new delay (latency report changed). Drops buffered
    /// history: the caller accepts a `new_delay`-sample glitch window
    /// rather than replaying stale audio at the wrong offset.
    pub fn reset(&mut self, delay: usize) {
        self.delay = delay;
        self.buf.clear();
        self.buf.shrink_to_fit();
    }

    /// Push `input` through, returning the delayed block (same length).
    pub fn delay_block(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        for &s in input {
            self.buf.push_back(s);
            if self.buf.len() > self.delay {
                out.push(self.buf.pop_front().unwrap_or(0.0));
            } else {
                out.push(0.0);
            }
        }
        out
    }
}

/// Delay the dry path of one insert by the device's latency so it meets
/// the wet path sample-aligned. `wet` passes through untouched (it is
/// already late by construction); the caller mixes the pair per sample
/// with [`apply_wet_dry`](crate::plugins::chain::apply_wet_dry).
pub fn align_dry(dry: &[f32], line: &mut DelayLine) -> Vec<f32> {
    line.delay_block(dry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unreported_devices_cost_nothing() {
        let map = LatencyMap::new();
        assert!(map.is_empty());
        assert_eq!(map.latency_of("ghost"), 0);
        assert_eq!(map.chain_latency(&chain(&["a", "b"])), 0);
        assert_eq!(map.alignment_delay(&chain(&["a"]), 128), 128);
    }

    #[test]
    fn report_round_trip_and_cap() {
        let mut map = LatencyMap::new();
        map.report("eq", 64).expect("report");
        map.report("lim", 512).expect("report");
        assert_eq!(map.latency_of("eq"), 64);
        assert_eq!(map.len(), 2);
        // Zero clears instead of storing a no-op entry.
        map.report("eq", 0).expect("clear");
        assert_eq!(map.latency_of("eq"), 0);
        assert_eq!(map.len(), 1);
        // Absurd reports are refused, not clamped into a giant buffer.
        let err = map
            .report("bug", MAX_LATENCY_SAMPLES + 1)
            .expect_err("over cap");
        assert!(matches!(err, LatencyError::Excessive { .. }));
        assert_eq!(map.latency_of("bug"), 0);
        // Cap itself is accepted.
        map.report("ok", MAX_LATENCY_SAMPLES).expect("cap");
        map.forget("ok");
        map.forget("lim");
        assert!(map.is_empty());
        map.report("a", 10).expect("a");
        map.clear();
        assert!(map.is_empty());
    }

    #[test]
    fn chain_totals_sum_and_session_aligns() {
        let mut map = LatencyMap::new();
        map.report("eq", 64).expect("eq");
        map.report("lim", 512).expect("lim");
        // Chain totals are the saturating sum of member reports.
        assert_eq!(map.chain_latency(&chain(&["eq", "lim"])), 576);
        assert_eq!(map.chain_latency(&chain(&["eq", "ghost"])), 64);
        // The slowest chain sets the session target; others pad up to it.
        let drum = chain(&["eq"]);
        let vox = chain(&["eq", "lim"]);
        let max = map.session_max([drum.as_slice(), vox.as_slice()]);
        assert_eq!(max, 576);
        assert_eq!(map.alignment_delay(&drum, max), 512);
        assert_eq!(map.alignment_delay(&vox, max), 0);
        // Nobody pads above the max (saturates, never underflows).
        assert_eq!(map.alignment_delay(&vox, 100), 0);
    }

    #[test]
    fn delay_line_pre_rolls_zeros_then_tracks() {
        let mut line = DelayLine::new(3);
        assert_eq!(line.delay_block(&[1.0, 2.0]), vec![0.0, 0.0]);
        assert_eq!(line.delay_block(&[3.0, 4.0, 5.0]), vec![0.0, 1.0, 2.0]);
        assert_eq!(line.delay_block(&[]), Vec::<f32>::new());
        // Zero delay is a documented pass-through.
        let mut none = DelayLine::new(0);
        assert_eq!(none.delay_block(&[1.0, -1.0]), vec![1.0, -1.0]);
    }

    #[test]
    fn delay_line_reset_flushes_stale_history() {
        let mut line = DelayLine::new(2);
        assert_eq!(line.delay_block(&[1.0, 2.0, 3.0]), vec![0.0, 0.0, 1.0]);
        line.reset(1);
        assert_eq!(line.delay(), 1);
        // Old buffered samples do not leak into the new alignment.
        assert_eq!(line.delay_block(&[9.0]), vec![0.0]);
        assert_eq!(line.delay_block(&[8.0]), vec![9.0]);
    }

    #[test]
    fn insert_compensation_aligns_dry_to_wet_impulse() {
        // A device with 4 samples of latency turns an impulse at 0 into
        // wet energy at 4; the compensated dry must arrive on the same
        // sample, or a 50/50 mix combs.
        let mut map = LatencyMap::new();
        map.report("lookahead", 4).expect("report");
        let mut line = DelayLine::new(map.latency_of("lookahead") as usize);
        let dry_in = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let wet_out = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let dry_aligned = align_dry(&dry_in, &mut line);
        assert_eq!(dry_aligned, vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(dry_aligned, wet_out.to_vec());
    }

    #[test]
    fn chain_latency_saturates_instead_of_wrapping() {
        let mut map = LatencyMap::new();
        map.report("a", MAX_LATENCY_SAMPLES).expect("a");
        map.report("b", MAX_LATENCY_SAMPLES).expect("b");
        // u64 sums of capped reports cannot realistically overflow, but
        // the fold is saturating by construction — assert the contract.
        let total = map.chain_latency(&chain(&["a", "b"]));
        assert_eq!(total, 2 * u64::from(MAX_LATENCY_SAMPLES));
    }
}
