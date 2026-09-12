//! Agent 5: metering + mastering DSP — graph taps that never block audio.
//!
//! Teaching note: a meter is a *tap*, not an insert. An insert rewrites
//! the signal (and a slow one clicks); a tap only *watches* it. So every
//! type here splits in two halves with opposite budgets:
//!
//! - The **audio half** (`observe` / `process`) runs on the realtime
//!   thread: no allocation, no locks, no channel `recv` — the same rule
//!   as [`crate::audio::device`]. It only folds samples into pre-sized
//!   rings and running sums.
//! - The **UI half** (snapshots, spectra) may allocate freely but must
//!   never wait for the audio thread: it reads through `try_lock` and
//!   takes "stale by one block" over "blocked", exactly like
//!   [`ParamBank`](crate::audio::device::ParamBank).
//!
//! Map: [`lufs`] (BS.1770-style integrated / short-term / momentary),
//! [`true_peak`] (4x-oversampled peak), [`spectrum`] (Hann-windowed FFT
//! tap), [`tap`] (the combined graph tap + non-blocking reader),
//! [`export`] (Track Q: loudness-normalized bounce target — integrated LUFS
//! + true-peak ceiling over the existing WAV stem codec),
//! [`conform`](conform) (Track S-3: platform preset table + one-click
//! conform + report artifact, measured with [`export`]).
//! Reads the frozen `Project` routing only via buffer names — adds no new
//! IPC or project-schema surface, so the typegen drift gate stays green.

pub mod conform;
pub mod export;
pub mod lufs;
pub mod spectrum;
pub mod tap;
pub mod true_peak;

pub use conform::{
    conform_mix_to_preset, conform_to_preset, measure_stem, preset_by_id, preview_conform_gain,
    ConformancePreset, ConformanceReport, ConformedBounce, ConformedStem, CONFORM_CEILING_SLACK_DB,
    CONFORM_TOLERANCE_LU, PRESETS, PRESET_MOBILE, PRESET_PC, PRESET_PLAYSTATION, PRESET_SWITCH,
    PRESET_XBOX, REPORT_SUFFIX,
};
pub use export::{LoudnessReport, LoudnessTarget, NormalizedBounce};
pub use lufs::LufsMeter;
pub use spectrum::{dominant_freq, spectrum_magnitudes, SharedRing};
pub use tap::{MeterReader, MeterSnapshot, MeterTap};
pub use true_peak::TruePeakMeter;
