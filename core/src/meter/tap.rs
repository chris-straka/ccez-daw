//! The combined graph tap: one `observe` per rendered block, one
//! non-blocking reader for the UI.
//!
//! Teaching note: picture the master bus. After
//! [`RenderGraph::render`](crate::audio::render::RenderGraph::render)
//! returns every node's buffer, the engine hands the master's buffer to
//! [`MeterTap::observe`] — still on the audio thread, still inside the
//! deadline — then the UI polls [`MeterReader`] whenever it repaints
//! (60 Hz is plenty; meters are watched, not heard).
//!
//! The handoff rule (same as [`ParamBank`](crate::audio::device::ParamBank)):
//! the audio side *never locks* — [`MeterTap::publish`] uses `try_lock`
//! and skips when the UI is reading — while the UI side *never waits* —
//! [`MeterReader::snapshot`] returns `None` under contention and the UI
//! repaints the previous frame. Completion of `observe`/`publish` is not
//! conditional on the other thread doing anything.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::lufs::LufsMeter;
use super::spectrum::{dominant_freq, push_shared, spectrum_magnitudes, SharedRing};
use super::true_peak::TruePeakMeter;

/// Default spectrum ring: 8192 samples (~170 ms at 48 kHz — enough for
/// a 4096-point analysis window with history to spare).
pub const SPECTRUM_RING: usize = 8192;
/// Default analysis window / bin count for [`MeterReader::spectrum`].
pub const SPECTRUM_WINDOW: usize = 4096;
pub const SPECTRUM_BINS: usize = 64;

/// Plain-data meter frame. `Clone`d across the thread boundary under
/// `try_lock` — the UI owns its copy outright.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterSnapshot {
    pub momentary_lufs: f32,
    pub short_term_lufs: f32,
    pub integrated_lufs: Option<f32>,
    pub true_peak: f32,
    pub true_peak_dbtp: f32,
}

impl MeterSnapshot {
    pub fn silent() -> Self {
        Self {
            momentary_lufs: f32::NEG_INFINITY,
            short_term_lufs: f32::NEG_INFINITY,
            integrated_lufs: None,
            true_peak: 0.0,
            true_peak_dbtp: f32::NEG_INFINITY,
        }
    }
}

/// Audio-thread half of the tap. Owns the meters; shares only two
/// mutexes with the UI, both touched via `try_lock`.
#[derive(Debug)]
pub struct MeterTap {
    lufs: LufsMeter,
    peak: TruePeakMeter,
    sample_rate: f32,
    snapshot: std::sync::Arc<Mutex<MeterSnapshot>>,
    ring: std::sync::Arc<Mutex<SharedRing>>,
}

impl MeterTap {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            lufs: LufsMeter::new(sample_rate),
            peak: TruePeakMeter::new(),
            sample_rate: sample_rate.max(1.0),
            snapshot: std::sync::Arc::new(Mutex::new(MeterSnapshot::silent())),
            ring: std::sync::Arc::new(Mutex::new(SharedRing::new(SPECTRUM_RING))),
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// UI-thread handle. Shares the snapshot + spectrum ring.
    pub fn reader(&self) -> MeterReader {
        MeterReader {
            sample_rate: self.sample_rate,
            snapshot: self.snapshot.clone(),
            ring: self.ring.clone(),
        }
    }

    /// Fold one rendered block in. Realtime-safe: meter math plus one
    /// `try_lock` for the spectrum ring (dropped, never waited on).
    /// `publish` is separate so the engine can observe several nodes
    /// and publish once per block.
    pub fn observe(&mut self, samples: &[f32]) {
        self.lufs.process(samples);
        self.peak.process(samples);
        push_shared(&self.ring, samples);
    }

    /// Tap a named node straight out of a rendered buffer map (the
    /// [`RenderGraph`](crate::audio::render::RenderGraph::render) return).
    /// Unknown nodes are a no-op — half-specified graphs still render,
    /// and a missing tap must never break them.
    pub fn observe_node(&mut self, buffers: &BTreeMap<String, Vec<f32>>, node: &str) {
        if let Some(buf) = buffers.get(node) {
            self.observe(buf);
        }
    }

    /// Refresh the shared snapshot. Never blocks: a UI-held lock means
    /// this frame is skipped, not waited for.
    pub fn publish(&mut self) {
        if let Ok(mut s) = self.snapshot.try_lock() {
            *s = MeterSnapshot {
                momentary_lufs: self.lufs.momentary_lufs(),
                short_term_lufs: self.lufs.short_term_lufs(),
                integrated_lufs: self.lufs.integrated_lufs(),
                true_peak: self.peak.peak(),
                true_peak_dbtp: self.peak.peak_dbtp(),
            };
        }
    }

    pub fn reset(&mut self) {
        self.lufs.reset();
        self.peak.reset();
        if let Ok(mut r) = self.ring.try_lock() {
            *r = SharedRing::new(SPECTRUM_RING);
        }
        if let Ok(mut s) = self.snapshot.try_lock() {
            *s = MeterSnapshot::silent();
        }
    }
}

/// UI-thread half of the tap. Every method returns immediately —
/// `None` means "audio is mid-write / nothing observed yet; repaint
/// the old frame".
#[derive(Debug, Clone)]
pub struct MeterReader {
    sample_rate: f32,
    snapshot: std::sync::Arc<Mutex<MeterSnapshot>>,
    ring: std::sync::Arc<Mutex<SharedRing>>,
}

impl MeterReader {
    /// Latest published frame, or `None` under contention.
    pub fn snapshot(&self) -> Option<MeterSnapshot> {
        self.snapshot.try_lock().ok().map(|s| s.clone())
    }

    /// Spectrum magnitudes over the latest window (defaults:
    /// [`SPECTRUM_WINDOW`] / [`SPECTRUM_BINS`]), or `None` under
    /// contention. Allocates — UI thread only.
    pub fn spectrum(&self, window: usize, bins: usize) -> Option<Vec<f32>> {
        let samples = self.ring.try_lock().ok()?.latest_window(window);
        Some(spectrum_magnitudes(&samples, self.sample_rate, bins))
    }

    /// Dominant frequency (Hz, exact DFT bin) in the latest window, or
    /// `None` under contention / on silence (which reads 0.0).
    pub fn spectrum_peak_hz(&self, window: usize) -> Option<f32> {
        let samples = self.ring.try_lock().ok()?.latest_window(window);
        Some(dominant_freq(&samples, self.sample_rate))
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
    fn observe_publish_read_round_trip() {
        let sr = 48_000.0;
        let mut tap = MeterTap::new(sr);
        let reader = tap.reader();
        let tone = sine(1000.0, 0.5, sr, sr as usize);
        for chunk in tone.chunks(512) {
            tap.observe(chunk);
        }
        tap.publish();
        let snap = reader.snapshot().expect("published");
        assert!((snap.true_peak - 0.5).abs() < 0.02, "peak {}", snap.true_peak);
        let integ = snap.integrated_lufs.expect("tone above gate");
        assert!((integ + 9.0).abs() < 1.0, "integrated {integ}");
        let peak_hz = reader.spectrum_peak_hz(SPECTRUM_WINDOW).expect("spectrum");
        assert!((peak_hz - 1000.0).abs() / 1000.0 < 0.2, "spectrum peak {peak_hz}");
    }

    #[test]
    fn publish_never_blocks_on_a_held_lock() {
        // The UI parks on the snapshot lock; the audio side must still
        // finish `publish` (it just skips the write). Completion IS the
        // assertion — a blocking publish would deadlock this test.
        let mut tap = MeterTap::new(48_000.0);
        let reader = tap.reader();
        let guard = reader.snapshot.try_lock().expect("lock");
        tap.observe(&[0.5; 512]);
        tap.publish(); // must return despite the held lock
        drop(guard);
        tap.publish();
        assert_eq!(reader.snapshot().expect("read").true_peak, 0.5);
    }

    #[test]
    fn missing_node_is_a_no_op() {
        let mut tap = MeterTap::new(48_000.0);
        tap.observe_node(&BTreeMap::new(), "nope");
        tap.publish();
        let reader = tap.reader();
        assert_eq!(reader.snapshot().expect("read"), MeterSnapshot::silent());
    }

    #[test]
    fn tap_watches_a_render_graph_node() {
        // End-to-end with Track B's renderer: impulse source → mixer,
        // tap on the mixer. Proves the tap plugs into the real buffer
        // map shape, not just a hand-fed slice.
        use crate::audio::graph::AudioGraph;
        use crate::audio::render::{Proc, RenderGraph};
        use crate::model::{Edge, EdgeKind, Project, Track};
        let mut p = Project::new("p", "Tap");
        p.tracks.push(Track {
            id: "a".into(),
            name: "a".into(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        p.routing.push(Edge {
            id: "e1".into(),
            from_node: "a".into(),
            from_port: "out".into(),
            to_node: "mix".into(),
            to_port: "in".into(),
            kind: EdgeKind::Audio,
        });
        let mut g = RenderGraph::from_audio_graph(AudioGraph::from_project(&p));
        g.set_proc("a", Proc::Constant(0.25));
        g.set_proc("mix", Proc::Mix);
        let bufs = g.render(512, 1, &BTreeMap::new()).expect("renders");
        let mut tap = MeterTap::new(48_000.0);
        tap.observe_node(&bufs, "mix");
        tap.publish();
        let snap = tap.reader().snapshot().expect("read");
        assert!((snap.true_peak - 0.25).abs() < 1e-6, "peak {}", snap.true_peak);
    }

    #[test]
    fn reset_returns_to_silence() {
        let mut tap = MeterTap::new(48_000.0);
        tap.observe(&sine(1000.0, 0.9, 48_000.0, 8192));
        tap.publish();
        tap.reset();
        assert_eq!(tap.reader().snapshot().expect("read"), MeterSnapshot::silent());
    }
}
