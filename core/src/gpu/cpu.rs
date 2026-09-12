//! CPU reference kernels for the GPU compute track.
//!
//! Teaching note: every GPU kernel needs a CPU twin. The twin is the
//! *specification* — simple, obviously-correct scalar code — and the GPU
//! shader must agree with it sample-for-sample (up to a small float
//! tolerance). These functions are also the guaranteed fallback: when no
//! GPU exists, the runner calls them and rendering still works.
//!
//! The one real kernel here is direct-form FIR convolution
//! ([`cpu_convolve`]): output[i] = sum over k of input[i-k]*ir[k]. That is
//! the inner loop of a partitioned-convolution reverb (a long impulse
//! response chopped into blocks, each block convolved and summed), so
//! proving GPU/CPU equivalence here proves the building block a
//! GPU reverb would be made of. [`cpu_gain`] exercises the same
//! runner plumbing with the simplest possible data-parallel op.

/// Maximum acceptable per-sample absolute difference between the GPU
/// kernel output and this CPU reference. f32 arithmetic on different
/// hardware can round the last bit differently, so tests use this
/// tolerance instead of demanding bitwise identity.
pub const EQUIVALENCE_TOLERANCE: f32 = 1e-5;

/// Elementwise gain: `output[i] = input[i] * gain`.
/// Lengths must match; the shorter length wins (mirrors [`Reverb::process`]
/// in [`crate::dsp`] which processes `min` of the two slices).
pub fn cpu_gain(input: &[f32], gain: f32, output: &mut [f32]) {
    let n = input.len().min(output.len());
    for i in 0..n {
        output[i] = input[i] * gain;
    }
}

/// Direct-form FIR convolution with a causal impulse response.
///
/// `output[i] = sum_{k=0}^{ir.len()-1} input[i-k] * ir[k]`, where
/// `input[j]` is treated as 0 for `j < 0` (zero pre-padding, i.e. the
/// line starts at rest). Processes `min(input.len(), output.len())`
/// frames. No allocation; safe to call from a realtime thread.
pub fn cpu_convolve(input: &[f32], ir: &[f32], output: &mut [f32]) {
    let n = input.len().min(output.len());
    if ir.is_empty() {
        output[..n].fill(0.0);
        return;
    }
    for i in 0..n {
        let mut acc = 0.0f32;
        // Same k order (0..m) as the WGSL shader, so rounding matches
        // as closely as f32 allows across backends.
        let kmax = ir.len().min(i + 1);
        for k in 0..kmax {
            acc += input[i - k] * ir[k];
        }
        output[i] = acc;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_scales_and_truncates_to_shorter_slice() {
        let input = [1.0, -2.0, 0.5];
        let mut out = [0.0; 2];
        cpu_gain(&input, 2.0, &mut out);
        assert_eq!(out, [2.0, -4.0]);
    }

    #[test]
    fn convolve_matches_hand_computed_values() {
        // input: impulse at 0 then [1,1,...]; ir: [0.5, 0.25].
        // out[0] = 1*0.5; out[1] = 1*0.5 + 1*0.25; out[2] = same.
        let input = [1.0, 1.0, 1.0, 1.0];
        let ir = [0.5, 0.25];
        let mut out = [0.0; 4];
        cpu_convolve(&input, &ir, &mut out);
        assert_eq!(out, [0.5, 0.75, 0.75, 0.75]);
    }

    #[test]
    fn convolve_empty_ir_yields_silence() {
        let input = [1.0, 2.0];
        let mut out = [9.0, 9.0];
        cpu_convolve(&input, &[], &mut out);
        assert_eq!(out, [0.0, 0.0]);
    }
}
