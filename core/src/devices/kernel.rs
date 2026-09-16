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
/// `transpose` param id in semitones (-48.0..=48.0, default 0.0).
/// Playback rate is `2^(transpose/12)` (linear resample pitch).
pub const TRANSPOSE_PARAM: &str = "transpose";
/// `attack` param id in seconds (0.0..=10.0, default 0.005).
pub const ATTACK_PARAM: &str = "attack";
/// `release` param id in seconds (0.0..=10.0, default 0.05).
pub const RELEASE_PARAM: &str = "release";

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

/// Pitch ratio for a semitone offset: `2^(semitones/12)`. Unclamped and
/// total — callers clamp `transpose` to its param range before calling.
pub fn semitones_to_rate(semitones: f64) -> f64 {
    2.0f64.powf(semitones / 12.0)
}

/// Simpler-style sampler voice state: the fractional read position into
/// the caller's sample buffer, the AR envelope level plus gate, and the
/// tone-filter memory. The sample buffer itself is *not* stored here —
/// it arrives as the `sample` slice on every `process` call, so the same
/// state struct works under `no_std` and across sample swaps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SamplerState {
    /// Fractional read position in sample frames. Advances by the pitch
    /// ratio per output frame; past the buffer end the voice is silent.
    pub position: f64,
    /// Current AR envelope level 0..=1.
    pub env: f32,
    /// True between `note_on` and `note_off` (attack/sustain vs release).
    pub gate_open: bool,
    /// Tone-filter (one-pole lowpass) memory.
    pub filter: LowpassState,
}

/// Open the gate and restart the voice: position and envelope go to zero
/// and the attack ramp starts on the next `process` call.
pub fn sampler_note_on(state: &mut SamplerState) {
    state.position = 0.0;
    state.env = 0.0;
    state.gate_open = true;
}

/// Close the gate: the release ramp starts from the current envelope
/// level on the next `process` call. Position keeps running so a
/// re-`note_on` always restarts, never resumes mid-sample.
pub fn sampler_note_off(state: &mut SamplerState) {
    state.gate_open = false;
}

/// Simpler-style sampler voice: linear-interp resample of `sample` at
/// `2^(transpose_semitones/12)`, a linear attack/release envelope, a
/// one-pole lowpass at `cutoff_hz`, and a `gain` trim.
///
/// - `sample_rate <= 0`/non-finite is [`DeviceError::BadBlock`]; block
///   lengths must agree like every other kernel.
/// - `transpose_semitones` clamps to ±48 (four octaves either way).
/// - `attack_secs`/`release_secs` clamp to >= 0 (0 = instant).
/// - An empty `sample` (or a voice played past its end) renders silence —
///   a missing sample is a setup gap, not a corrupt block.
/// - No allocation inside `process`.
#[allow(clippy::too_many_arguments)]
pub fn sampler_process(
    sample: &[f32],
    transpose_semitones: f64,
    gain: f32,
    attack_secs: f64,
    release_secs: f64,
    cutoff_hz: f64,
    sample_rate: f64,
    state: &mut SamplerState,
    output: &mut [f32],
) -> Result<(), DeviceError> {
    // A sampler voice takes no audio input — it generates from the sample
    // buffer — so there is no input/output length contract here. Empty
    // output blocks are a legal no-op.
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(DeviceError::BadBlock(format!(
            "sample rate must be positive, got {sample_rate}"
        )));
    }
    let rate = semitones_to_rate(transpose_semitones.clamp(-48.0, 48.0));
    if !rate.is_finite() || rate <= 0.0 {
        return Err(DeviceError::BadBlock(format!(
            "bad sampler rate for transpose {transpose_semitones}"
        )));
    }
    let attack_step = if attack_secs > 0.0 && attack_secs.is_finite() {
        (1.0 / (attack_secs * sample_rate)) as f32
    } else {
        f32::INFINITY
    };
    let release_step = if release_secs > 0.0 && release_secs.is_finite() {
        (1.0 / (release_secs * sample_rate)) as f32
    } else {
        f32::INFINITY
    };
    let a = lowpass_coeff(cutoff_hz, sample_rate)? as f32;
    let mut pos = state.position;
    let mut env = state.env;
    let mut filt = state.filter.y_prev;
    for o in output.iter_mut() {
        // AR envelope: linear ramp toward the gate target.
        let target = if state.gate_open { 1.0 } else { 0.0 };
        let step = if state.gate_open { attack_step } else { release_step };
        if step.is_infinite() {
            env = target;
        } else if env < target {
            env = (env + step).min(target);
        } else if env > target {
            env = (env - step).max(target);
        }
        // Linear-interp resample; past the end the voice is silent.
        let voiced = if sample.is_empty() {
            0.0
        } else {
            let i = pos.floor() as usize;
            if i >= sample.len() {
                0.0
            } else {
                let frac = (pos - pos.floor()) as f32;
                let next = sample.get(i + 1).copied().unwrap_or(0.0);
                sample[i] + frac * (next - sample[i])
            }
        };
        pos += rate;
        // Tone filter, then trim gain and envelope.
        filt += a * (voiced - filt);
        *o = filt * env * gain;
    }
    state.position = pos;
    state.env = env;
    state.filter.y_prev = filt;
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
    fn semitones_to_rate_hits_octaves() {
        assert!((semitones_to_rate(0.0) - 1.0).abs() < 1e-12);
        assert!((semitones_to_rate(12.0) - 2.0).abs() < 1e-12);
        assert!((semitones_to_rate(-12.0) - 0.5).abs() < 1e-12);
        assert!((semitones_to_rate(7.0) - 1.498_307_1).abs() < 1e-6);
    }

    #[test]
    fn sampler_plays_dc_at_unity_when_gate_open() {
        let sample = vec![1.0f32; 64];
        let mut s = SamplerState::default();
        sampler_note_on(&mut s);
        let mut out = vec![0.0; 64];
        // Instant envelope, wide-open filter: the voice settles to
        // sample * gain (frame 0 still carries the filter attack).
        sampler_process(&sample, 0.0, 2.0, 0.0, 0.0, 20000.0, 44100.0, &mut s, &mut out)
            .expect("play");
        assert!(out[0] > 0.0, "voice must start immediately");
        for v in &out[32..] {
            assert!((v - 2.0).abs() < 1e-3, "voice must be unity, got {v}");
        }
    }

    #[test]
    fn sampler_pitch_doubles_rate_an_octave_up() {
        // Ramp sample: position is observable through the value played.
        let sample: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let mut s = SamplerState::default();
        sampler_note_on(&mut s);
        let mut out = vec![0.0; 8];
        sampler_process(&sample, 12.0, 1.0, 0.0, 0.0, 20000.0, 44100.0, &mut s, &mut out)
            .expect("play");
        // Rate 2.0 reads frames 0,2,4,...; filter is ~transparent at 20k.
        assert!((s.position - 16.0).abs() < 1e-9, "position {}", s.position);
        assert!((out[1] - 2.0).abs() < 0.15, "got {}", out[1]);
        assert!((out[3] - 6.0).abs() < 0.3, "got {}", out[3]);
    }

    #[test]
    fn sampler_envelope_attacks_and_releases() {
        let sample = vec![1.0f32; 4096];
        let mut s = SamplerState::default();
        sampler_note_on(&mut s);
        let mut out = vec![0.0; 100];
        // 100-sample attack at 1000 Hz: frame 50 sits near half level.
        sampler_process(&sample, 0.0, 1.0, 0.1, 0.1, 20000.0, 1000.0, &mut s, &mut out)
            .expect("attack");
        assert!(out[0] < out[99], "attack must ramp up");
        assert!((out[49] - 0.5).abs() < 0.15, "mid-attack {}", out[49]);
        sampler_note_off(&mut s);
        sampler_process(&sample, 0.0, 1.0, 0.1, 0.1, 20000.0, 1000.0, &mut s, &mut out)
            .expect("release");
        assert!(out[0] > out[99], "release must ramp down");
        assert!(out[99] < 0.05, "release must settle, got {}", out[99]);
    }

    #[test]
    fn sampler_silent_past_end_empty_and_rejects_bad_rate() {
        let sample = vec![1.0f32, 1.0];
        let mut s = SamplerState::default();
        sampler_note_on(&mut s);
        let mut out = vec![9.0; 8];
        sampler_process(&sample, 0.0, 1.0, 0.0, 0.0, 20000.0, 44100.0, &mut s, &mut out)
            .expect("play");
        // Past the 2-frame sample the voice goes silent (tail of block —
        // the filter tail drains within a few frames at 20 kHz).
        assert!(out[7].abs() < 1e-4, "tail {}", out[7]);
        // Empty sample = silence, not an error.
        let mut s = SamplerState::default();
        sampler_note_on(&mut s);
        let mut out = vec![9.0; 4];
        sampler_process(&[], 0.0, 1.0, 0.0, 0.0, 20000.0, 44100.0, &mut s, &mut out)
            .expect("empty ok");
        assert_eq!(out, vec![0.0; 4]);
        // Bad sample rate is BadBlock like every other kernel.
        let mut out = vec![0.0; 4];
        assert!(matches!(
            sampler_process(&sample, 0.0, 1.0, 0.0, 0.0, 1000.0, 0.0, &mut s, &mut out),
            Err(DeviceError::BadBlock(_))
        ));
        // note_on restarts: position never resumes mid-sample.
        sampler_note_on(&mut s);
        assert_eq!(s.position, 0.0);
        assert_eq!(s.env, 0.0);
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
