//! Ported delay kernel: the guest side of the devices-to-WASM runtime.
//!
//! Teaching note: this file is simultaneously two things — the in-process
//! reference the [`crate::wasmdevices`] sandbox renders through, and the
//! exact source compiled to `wasm32-wasip2` for the real sandbox
//! (`core/wasm-guest` includes this file by path, so the two can never
//! drift). That dual role imposes one rule: **no `crate::` imports**.
//! Everything here is plain `f32`/`f64` math plus one preallocated `Vec`,
//! so the same code compiles to native, to `wasm32-wasip2`, and onto a
//! realtime thread — the kernel portability contract from
//! [`crate::devices::kernel`], now with a proof instead of a promise.
//!
//! The port target is [`crate::dsp::Delay`], the simplest kernel in
//! `core/src/dsp`: a feedback delay line with millisecond time, feedback,
//! and wet/dry mix. The math below mirrors `Delay::process` op-for-op
//! (including clamp order and the `(pos + len - d) % len` tap), which is
//! what makes the render-equivalence test bit-exact rather than
//! tolerance-based: integer index arithmetic and `f32` multiply-add
//! evaluate identically on native and wasm32 targets.
//!
//! One structural addition over the native kernel: [`GuestDelay::step`]
//! renders a single sample. The native kernel only has block `process`;
//! the WASM boundary speaks per-sample calls (`delay_sample`, one typed
//! `Func` with no shared memory — the narrowest possible sandbox
//! aperture, see [`crate::wasmdevices::sandbox`]), so the guest factors
//! the loop body out. [`GuestDelay::process`] runs the same body in the
//! same order, and a test pins `step`-loop == `process` == native.

/// `time_ms` param id in ms (1..=2000, default 375). Mirrors
/// [`crate::dsp::delay::PARAM_TIME_MS`]; re-declared here so this file
/// stays dependency-free (see module docs).
pub const PARAM_TIME_MS: &str = "time_ms";
/// `feedback` param id, unitless (0..=0.95, default 0.35). Mirrors
/// [`crate::dsp::delay::PARAM_FEEDBACK`].
pub const PARAM_FEEDBACK: &str = "feedback";
/// `mix` param id, wet proportion (0..=1, default 0.3). Mirrors
/// [`crate::dsp::delay::PARAM_MIX`].
pub const PARAM_MIX: &str = "mix";

/// Numeric param codes for the WASM boundary. Strings cannot cross the
/// `delay_param(id, value)` typed func without shared memory, so params
/// cross as `(code, f32)` pairs. Append-only, like the device-class codes
/// in [`crate::devices::class`].
pub const CODE_TIME_MS: i32 = 0;
pub const CODE_FEEDBACK: i32 = 1;
pub const CODE_MIX: i32 = 2;

/// Preallocated line length: 2 seconds at the construction sample rate,
/// plus one. Mirrors the native kernel's `MAX_DELAY_S`.
pub const MAX_DELAY_S: f32 = 2.0;

/// Return codes for the `delay_param` WASM export. `0` is success;
/// nonzero is a refused set, never a trap (traps abort the render; a bad
/// knob must not).
pub const PARAM_OK: i32 = 0;
pub const PARAM_UNKNOWN_ID: i32 = 1;

/// Marker for a refused scalar param set (unknown id). The host carries
/// the id for its error message; the guest stays string-free on this
/// path too (formatting lives host-side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownParam;

/// Feedback delay kernel, ported from [`crate::dsp::Delay`].
/// The line is preallocated at construction; `step`/`process` borrow only.
#[derive(Debug, Clone)]
pub struct GuestDelay {
    pub sample_rate: f32,
    pub time_ms: f32,
    pub feedback: f32,
    pub mix: f32,
    line: Vec<f32>,
    pos: usize,
}

impl GuestDelay {
    pub fn new(sample_rate: f32) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            1.0
        };
        Self {
            sample_rate: sr,
            time_ms: 375.0,
            feedback: 0.35,
            mix: 0.3,
            line: vec![0.0; (sr * MAX_DELAY_S) as usize + 1],
            pos: 0,
        }
    }

    /// Sources of latency for compensation to align: none (no lookahead,
    /// same as native).
    pub fn latency_samples(&self) -> u64 {
        0
    }

    pub fn reset(&mut self) {
        self.line.fill(0.0);
        self.pos = 0;
    }

    /// Current delay in samples (test probe). Same formula as native:
    /// float truncation toward zero, then clamped into the line.
    pub fn delay_samples(&self) -> usize {
        ((self.time_ms / 1000.0 * self.sample_rate) as usize)
            .clamp(1, self.line.len() - 1)
    }

    /// One sample through the loop: the body of the native block loop,
    /// factored out for the per-sample WASM boundary (`delay_sample`).
    /// Clamp order matches the native kernel exactly (`wet` then `fb` —
    /// order is irrelevant to the result but kept identical on purpose).
    pub fn step(&mut self, x: f32) -> f32 {
        let d = self.delay_samples();
        let len = self.line.len();
        let wet = self.mix.clamp(0.0, 1.0);
        let fb = self.feedback.clamp(0.0, 0.95);
        let tap = (self.pos + len - d) % len;
        let echo = self.line[tap];
        self.line[self.pos] = x + echo * fb;
        let y = x * (1.0 - wet) + echo * wet;
        self.pos += 1;
        if self.pos >= len {
            self.pos = 0;
        }
        y
    }

    /// Block render: the native loop verbatim, so `process` and a
    /// `step` loop agree bit-for-bit (pinned by test).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let n = input.len().min(output.len());
        for i in 0..n {
            output[i] = self.step(input[i]);
        }
    }

    /// Scalar param set with native clamps. Unknown ids refuse (the host
    /// maps this to `UnknownParam` with the address attached — typos must
    /// surface, the [`crate::devices::class::set_param_value`] rule).
    pub fn apply_param(&mut self, id: &str, value: f64) -> Result<(), UnknownParam> {
        match id {
            PARAM_TIME_MS => {
                self.time_ms = value.clamp(1.0, MAX_DELAY_S as f64 * 1000.0) as f32
            }
            PARAM_FEEDBACK => self.feedback = value.clamp(0.0, 0.95) as f32,
            PARAM_MIX => self.mix = value.clamp(0.0, 1.0) as f32,
            _ => return Err(UnknownParam),
        }
        Ok(())
    }

    /// Numeric-code param set for the WASM boundary (`delay_param`).
    /// Same clamps as [`GuestDelay::apply_param`]; returns a code, never
    /// traps (see [`PARAM_OK`]).
    pub fn apply_code(&mut self, code: i32, value: f32) -> i32 {
        match code {
            CODE_TIME_MS => {
                self.time_ms = (value as f64).clamp(1.0, MAX_DELAY_S as f64 * 1000.0) as f32;
                PARAM_OK
            }
            CODE_FEEDBACK => {
                self.feedback = (value as f64).clamp(0.0, 0.95) as f32;
                PARAM_OK
            }
            CODE_MIX => {
                self.mix = (value as f64).clamp(0.0, 1.0) as f32;
                PARAM_OK
            }
            _ => PARAM_UNKNOWN_ID,
        }
    }

    /// Param id for a boundary code (host-side glue for error messages).
    pub fn id_for_code(code: i32) -> Option<&'static str> {
        match code {
            CODE_TIME_MS => Some(PARAM_TIME_MS),
            CODE_FEEDBACK => Some(PARAM_FEEDBACK),
            CODE_MIX => Some(PARAM_MIX),
            _ => None,
        }
    }

    /// Boundary code for a param id (host-side glue for `delay_param`).
    pub fn code_for_id(id: &str) -> Option<i32> {
        match id {
            PARAM_TIME_MS => Some(CODE_TIME_MS),
            PARAM_FEEDBACK => Some(CODE_FEEDBACK),
            PARAM_MIX => Some(CODE_MIX),
            _ => None,
        }
    }
}
// Only `Vec`, `f32`/`f64` math, and `Option`/`Result` cross this file's
// boundary — no `crate::` imports, no OS calls in the audio path — so it
// also compiles under `#![no_std]` guests and to `wasm32-wasip2` as-is.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_loop_matches_block_process() {
        let mut a = GuestDelay::new(48_000.0);
        let mut b = GuestDelay::new(48_000.0);
        a.apply_param(PARAM_TIME_MS, 100.0).unwrap();
        b.apply_param(PARAM_TIME_MS, 100.0).unwrap();
        a.apply_param(PARAM_FEEDBACK, 0.5).unwrap();
        b.apply_param(PARAM_FEEDBACK, 0.5).unwrap();
        let input: Vec<f32> = (0..8192).map(|t| (t as f32 * 0.01).sin()).collect();
        let mut block = vec![0.0; input.len()];
        let mut stepped = vec![0.0; input.len()];
        a.process(&input, &mut block);
        for (o, i) in stepped.iter_mut().zip(input.iter()) {
            *o = b.step(*i);
        }
        assert_eq!(block, stepped);
    }

    #[test]
    fn code_and_id_sets_agree_and_clamp() {
        let mut a = GuestDelay::new(48_000.0);
        let mut b = GuestDelay::new(48_000.0);
        a.apply_param(PARAM_FEEDBACK, 99.0).unwrap();
        assert_eq!(a.feedback, 0.95);
        assert_eq!(b.apply_code(CODE_FEEDBACK, 99.0), PARAM_OK);
        assert_eq!(b.feedback, a.feedback);
        assert_eq!(b.apply_code(77, 0.5), PARAM_UNKNOWN_ID);
        assert!(b.apply_param("nope", 0.0).is_err());
        assert_eq!(GuestDelay::id_for_code(CODE_MIX), Some(PARAM_MIX));
        assert_eq!(GuestDelay::code_for_id("nope"), None);
    }
}
