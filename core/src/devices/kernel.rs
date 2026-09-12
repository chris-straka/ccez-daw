//! Portable pure-Rust DSP kernels: the sound of every native device.
//!
//! Teaching note: a kernel is a function from samples to samples —
//! `process(input, output)` — with no hidden state and no allocation.
//! Anything a kernel needs across blocks (yesterday's filter output, the
//! delay line) lives in an explicit `*State` struct the *caller* owns.
//! That is what makes these portable: the same code compiles to native,
//! to WASM (see [`crate::devices::wasm`]), or onto the realtime thread,
//! because it never asks the OS for anything.
//!
//! Rules every kernel honors:
//!
//! - `output.len() == input.len()` or [`DeviceError::BadBlock`]. Empty
//!   blocks are legal and a no-op (lengths still agree).
//! - No allocation inside `process` (no `Vec`, no `Box`, no format).
//!   Callers that must allocate (the 2x oversample wrapper) own those
//!   buffers one layer up, in [`crate::devices::rack`].
//! - Params arrive as plain `f32`/`f64` scalars, already resolved from the
//!   frozen `Node` by the caller. Kernels never touch `model` types —
//!   they would not compile under `no_std` if they did.

use std::f32::consts::PI;

/// Errors from native-device DSP and param handling.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceError {
    /// `input.len() != output.len()`, or a non-positive sample rate.
    BadBlock(String),
    /// `node:param` names no param on the node (typos must surface).
    UnknownParam(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadBlock(m) => write!(f, "bad device block: {m}"),
            Self::UnknownParam(t) => write!(f, "unknown device param `{t}`"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// `gain` param id (linear, 0.0..=4.0, default 1.0).
pub const GAIN_PARAM: &str = "gain";
/// `cutoff` param id in Hz (20..=20000, default 1000.0).
pub const CUTOFF_PARAM: &str = "cutoff";
/// `delay_samples` param id (0..=48000, default 0.0).
pub const DELAY_SAMPLES_PARAM: &str = "delay_samples";
/// `feedback` param id, unitless (0.0..=0.95, default 0.0).
pub const FEEDBACK_PARAM: &str = "feedback";
/// `drive` param id, unitless (0.0..=10.0, default 1.0).
pub const DRIVE_PARAM: &str = "drive";

/// Hard ceiling for feedback: 1.0 would circulate forever, above 1.0
/// explodes. Clamped, never erroring (same clamp-not-error rule as
/// `set_wet_dry` and the engine's `ParamSet`).
pub const MAX_FEEDBACK: f64 = 0.95;

fn check_block(input: &[f32], output: &[f32]) -> Result<usize, DeviceError> {
    if input.len() != output.len() {
        return Err(DeviceError::BadBlock(format!(
            "input {} frames != output {} frames",
            input.len(),
            output.len()
        )));
    }
    Ok(input.len())
}

/// Gain: `out = in * gain`. Stateless, the null-device with a knob.
pub fn gain_process(gain: f32, input: &[f32], output: &mut [f32]) -> Result<(), DeviceError> {
    check_block(input, output)?;
    for (o, i) in output.iter_mut().zip(input.iter()) {
        *o = *i * gain;
    }
    Ok(())
}

/// One-pole lowpass state: yesterday's output. Zero = settled silence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LowpassState {
    pub y_prev: f32,
}

/// One-pole lowpass: `y = y_prev + a * (x - y_prev)` with
/// `a = 1 - exp(-2π·fc/sr)`. DC passes untouched; Nyquist is crushed.
/// `cutoff_hz` clamps to `(0, sr/2)`; `sample_rate <= 0` is [`DeviceError::BadBlock`].
pub fn lowpass_process(
    cutoff_hz: f64,
    sample_rate: f64,
    state: &mut LowpassState,
    input: &[f32],
    output: &mut [f32],
) -> Result<(), DeviceError> {
    check_block(input, output)?;
    let a = lowpass_coeff(cutoff_hz, sample_rate)? as f32;
    let mut y = state.y_prev;
    for (o, i) in output.iter_mut().zip(input.iter()) {
        y += a * (*i - y);
        *o = y;
    }
    state.y_prev = y;
    Ok(())
}

fn lowpass_coeff(cutoff_hz: f64, sample_rate: f64) -> Result<f64, DeviceError> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(DeviceError::BadBlock(format!(
            "sample rate must be positive, got {sample_rate}"
        )));
    }
    let nyquist = sample_rate / 2.0;
    let fc = cutoff_hz.clamp(f32::MIN_POSITIVE as f64, nyquist);
    Ok(1.0 - (-2.0 * PI as f64 * fc / sample_rate).exp())
}

/// One-pole highpass state: yesterday's input and output.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HighpassState {
    pub x_prev: f32,
    pub y_prev: f32,
}

/// One-pole highpass: `y = a * (y_prev + x - x_prev)` with the same `a`
/// as the lowpass. DC is rejected; highs pass.
pub fn highpass_process(
    cutoff_hz: f64,
    sample_rate: f64,
    state: &mut HighpassState,
    input: &[f32],
    output: &mut [f32],
) -> Result<(), DeviceError> {
    check_block(input, output)?;
    let a = (-2.0 * PI as f64 * cutoff_hz.clamp(0.0, sample_rate / 2.0) / sample_rate).exp();
    if !a.is_finite() {
        return Err(DeviceError::BadBlock(format!(
            "bad highpass coefficient for cutoff {cutoff_hz}"
        )));
    }
    let a = a as f32;
    let (mut xp, mut yp) = (state.x_prev, state.y_prev);
    for (o, i) in output.iter_mut().zip(input.iter()) {
        let y = a * (yp + *i - xp);
        xp = *i;
        yp = y;
        *o = y;
    }
    state.x_prev = xp;
    state.y_prev = yp;
    Ok(())
}

/// Delay-line state: the circulating buffer plus write position.
/// Sized by [`DelayState::ensure`]; never reallocated inside `process`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DelayState {
    pub line: Vec<f32>,
    pub pos: usize,
}

impl DelayState {
    /// Guarantee room for `delay_samples` of history. Grows (zero-filled)
    /// or shrinks as needed; growing resets position so old echoes cannot
    /// smear across a param change. Called outside `process` — this is
    /// the one place delay allocates.
    pub fn ensure(&mut self, delay_samples: usize) {
        if self.line.len() != delay_samples {
            self.line = vec![0.0; delay_samples];
            self.pos = 0;
        }
    }
}

/// Feedback comb: `y[n] = x[n] + fb * y[n-D]`. `delay_samples == 0`
/// bypasses the line (instant feedback would be a division by causality).
/// `feedback` clamps to `[0, MAX_FEEDBACK]`.
pub fn delay_process(
    delay_samples: usize,
    feedback: f64,
    state: &mut DelayState,
    input: &[f32],
    output: &mut [f32],
) -> Result<(), DeviceError> {
    check_block(input, output)?;
    let n = input.len();
    if delay_samples == 0 || n == 0 {
        output[..n].copy_from_slice(&input[..n]);
        return Ok(());
    }
    state.ensure(delay_samples);
    let fb = feedback.clamp(0.0, MAX_FEEDBACK) as f32;
    let len = state.line.len();
    for (o, i) in output.iter_mut().zip(input.iter()) {
        let echo = state.line[state.pos];
        let y = *i + fb * echo;
        state.line[state.pos] = y;
        state.pos = (state.pos + 1) % len;
        *o = y;
    }
    Ok(())
}

/// Soft-clip distortion: `y = (1+k)·x / (1+k·|x|)`. `drive == 0` is
/// unity; large drive approaches a hard clip at ±(1+1/k)-ish rails while
/// small signals stay near-linear, so it never explodes and never needs
/// a makeup-gain footgun in v1.
pub fn distortion_process(
    drive: f32,
    input: &[f32],
    output: &mut [f32],
) -> Result<(), DeviceError> {
    check_block(input, output)?;
    let k = drive.max(0.0);
    for (o, i) in output.iter_mut().zip(input.iter()) {
        *o = (1.0 + k) * *i / (1.0 + k * i.abs());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_scales_and_rejects_ragged_blocks() {
        let input = vec![1.0, -0.5, 0.25];
        let mut out = vec![0.0; 3];
        gain_process(2.0, &input, &mut out).expect("process");
        assert_eq!(out, vec![2.0, -1.0, 0.5]);
        let mut short = vec![0.0; 2];
        assert!(matches!(
            gain_process(1.0, &input, &mut short),
            Err(DeviceError::BadBlock(_))
        ));
        // Empty blocks are a legal no-op.
        let mut empty: Vec<f32> = vec![];
        gain_process(1.0, &[], &mut empty).expect("empty ok");
    }

    #[test]
    fn lowpass_passes_dc_and_crushes_nyquist() {
        let mut s = LowpassState::default();
        let dc = vec![1.0f32; 2048];
        let mut out = vec![0.0; 2048];
        lowpass_process(1000.0, 44100.0, &mut s, &dc, &mut out).expect("dc");
        assert!((out[2047] - 1.0).abs() < 1e-3, "dc settles to {}", out[2047]);

        let mut s = LowpassState::default();
        let nyquist: Vec<f32> = (0..1024).map(|t| if t % 2 == 0 { 1.0 } else { -1.0 }).collect();
        let mut out = vec![0.0; 1024];
        lowpass_process(1000.0, 44100.0, &mut s, &nyquist, &mut out).expect("hf");
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms < 0.15, "nyquist must be crushed, rms was {rms}");
    }

    #[test]
    fn highpass_rejects_dc_and_passes_nyquist() {
        let mut s = HighpassState::default();
        let dc = vec![1.0f32; 2048];
        let mut out = vec![0.0; 2048];
        highpass_process(1000.0, 44100.0, &mut s, &dc, &mut out).expect("dc");
        assert!(out[2047].abs() < 1e-3, "dc must drain, got {}", out[2047]);

        let mut s = HighpassState::default();
        let nyquist: Vec<f32> = (0..1024).map(|t| if t % 2 == 0 { 1.0 } else { -1.0 }).collect();
        let mut out = vec![0.0; 1024];
        highpass_process(1000.0, 44100.0, &mut s, &nyquist, &mut out).expect("hf");
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.8, "nyquist must pass, rms was {rms}");
    }

    #[test]
    fn delay_echoes_an_impulse_with_feedback_decay() {
        let mut s = DelayState::default();
        let mut input = vec![0.0f32; 16];
        input[0] = 1.0;
        let mut out = vec![0.0; 16];
        delay_process(4, 0.5, &mut s, &input, &mut out).expect("delay");
        assert_eq!(out[0], 1.0);
        assert_eq!(out[4], 0.5);
        assert_eq!(out[8], 0.25);
        assert_eq!(out[12], 0.125);
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn distortion_is_unity_at_zero_drive_and_bounded_above() {
        let input = vec![0.5, -2.0, 4.0];
        let mut out = vec![0.0; 3];
        distortion_process(0.0, &input, &mut out).expect("clean");
        assert_eq!(out, input);
        distortion_process(10.0, &input, &mut out).expect("driven");
        for s in &out {
            assert!(s.abs() <= 1.1, "soft clip must bound, got {s}");
        }
        // Small signals stay near-linear: no surprise gating.
        let small = vec![0.01];
        let mut tiny = vec![0.0; 1];
        distortion_process(10.0, &small, &mut tiny).expect("small");
        assert!((tiny[0] - 0.01).abs() < 0.1, "got {}", tiny[0]);
    }
}
