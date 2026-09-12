//! GPU equivalence + fallback tests.
//!
//! Teaching note: these tests are backend-agnostic on purpose. On a
//! machine with a GPU and `--features gpu` they prove the shader agrees
//! with the CPU reference; on headless CI (or the default feature set)
//! they prove the fallback produces the same right answer. Either way
//! green means "rendering is correct on this machine". The one
//! feature-gated test asserts that when a GPU *is* live, it was the GPU
//! that produced the matching samples — so the equivalence claim is
//! actually exercised somewhere, not just the fallback.

use ccez_core::gpu::{
    cpu_convolve, Backend, BackendPreference, KernelRunner, PowerPreference, EQUIVALENCE_TOLERANCE,
};

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

fn sine_sweep(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let t = i as f32 / frames as f32;
            (2.0 * std::f32::consts::PI * (20.0 * t + 400.0 * t * t)).sin() * 0.5
        })
        .collect()
}

fn exp_decay_ir(taps: usize) -> Vec<f32> {
    (0..taps)
        .map(|k| (-(k as f32) / (taps as f32 / 4.0)).exp() * 0.8)
        .collect()
}

#[test]
fn auto_runner_matches_cpu_reference_on_sweep() {
    let input = sine_sweep(4096);
    let ir = exp_decay_ir(256);
    let mut expected = vec![0.0; input.len()];
    cpu_convolve(&input, &ir, &mut expected);
    let runner = KernelRunner::new(BackendPreference::Auto);
    let mut got = vec![0.0; input.len()];
    runner.run_convolve(&input, &ir, &mut got).unwrap();
    let d = max_abs_diff(&got, &expected);
    assert!(
        d <= EQUIVALENCE_TOLERANCE,
        "backend {:?}: max diff {d} exceeds tolerance",
        runner.backend()
    );
}

#[test]
fn auto_runner_matches_reference_on_impulse() {
    // Impulse input exposes the raw IR through the kernel: out == ir
    // padded with zeros. Any backend disagreement shows up directly.
    let mut input = vec![0.0f32; 1024];
    input[0] = 1.0;
    let ir = exp_decay_ir(128);
    let mut expected = vec![0.0; 1024];
    cpu_convolve(&input, &ir, &mut expected);
    let runner = KernelRunner::new(BackendPreference::Auto);
    let mut got = vec![0.0; 1024];
    runner.run_convolve(&input, &ir, &mut got).unwrap();
    assert!(max_abs_diff(&got, &expected) <= EQUIVALENCE_TOLERANCE);
    // And the impulse response really is the IR up front.
    assert!(max_abs_diff(&got[..128], &ir) <= EQUIVALENCE_TOLERANCE);
}

#[test]
fn force_gpu_either_matches_or_fails_gracefully() {
    // The strict contract: a forced GPU either computes the right answer
    // or says Unavailable. It must never panic, never return garbage.
    let input = sine_sweep(512);
    let ir = exp_decay_ir(64);
    let mut expected = vec![0.0; input.len()];
    cpu_convolve(&input, &ir, &mut expected);
    let runner = KernelRunner::new(BackendPreference::ForceGpu);
    let mut got = vec![0.0; input.len()];
    match runner.run_convolve(&input, &ir, &mut got) {
        Ok(()) => {
            assert!(runner.is_gpu_live(), "ForceGpu Ok but no GPU live?");
            assert!(max_abs_diff(&got, &expected) <= EQUIVALENCE_TOLERANCE);
        }
        Err(e) => {
            assert!(!runner.is_gpu_live());
            let msg = e.to_string();
            assert!(msg.contains("unavailable"), "unexpected error kind: {msg}");
            assert!(runner.fallback_reason().is_some());
        }
    }
}

#[test]
fn device_selection_probe_is_safe_and_selection_is_honored() {
    for power in [PowerPreference::HighPerformance, PowerPreference::LowPower] {
        let devices = KernelRunner::probe_devices(power);
        // No duplicates from multi-backend enumeration.
        let mut names: Vec<_> = devices.iter().map(|d| (d.name.clone(), d.backend.clone())).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), devices.len());
        // A runner built with this preference still renders correctly.
        let runner = KernelRunner::with_power(BackendPreference::Auto, power);
        let (input, ir) = (sine_sweep(256), exp_decay_ir(32));
        let mut expected = vec![0.0; 256];
        cpu_convolve(&input, &ir, &mut expected);
        let mut got = vec![0.0; 256];
        runner.run_convolve(&input, &ir, &mut got).unwrap();
        assert!(max_abs_diff(&got, &expected) <= EQUIVALENCE_TOLERANCE);
    }
}

#[test]
fn gain_path_is_backend_independent() {
    let input = sine_sweep(1024);
    for pref in [
        BackendPreference::Auto,
        BackendPreference::ForceCpu,
        BackendPreference::ForceGpu,
    ] {
        let runner = KernelRunner::new(pref);
        let mut got = vec![0.0; input.len()];
        match runner.run_gain(&input, 0.25, &mut got) {
            Ok(()) => {
                let expected: Vec<f32> = input.iter().map(|s| s * 0.25).collect();
                assert!(max_abs_diff(&got, &expected) <= EQUIVALENCE_TOLERANCE);
            }
            Err(e) => {
                // Only strict-GPU-without-hardware may decline.
                assert_eq!(runner.backend(), Backend::Gpu);
                assert!(e.to_string().contains("unavailable"));
            }
        }
    }
}

/// When the `gpu` feature is on and hardware exists, this test fails
/// unless the samples genuinely came off the GPU — it pins the
/// equivalence claim to the shader, not the fallback.
#[cfg(feature = "gpu")]
#[test]
fn gpu_feature_build_can_drive_real_hardware() {
    let runner = KernelRunner::new(BackendPreference::ForceGpu);
    if !runner.is_gpu_live() {
        eprintln!(
            "no GPU on this machine ({}); equivalence covered by fallback tests",
            runner.fallback_reason().unwrap_or("unknown")
        );
        return;
    }
    let input = sine_sweep(2048);
    let ir = exp_decay_ir(192);
    let mut expected = vec![0.0; input.len()];
    cpu_convolve(&input, &ir, &mut expected);
    let mut got = vec![0.0; input.len()];
    runner.run_convolve(&input, &ir, &mut got).unwrap();
    let d = max_abs_diff(&got, &expected);
    let info = runner.device_info().expect("live GPU has info");
    assert!(
        d <= EQUIVALENCE_TOLERANCE,
        "GPU {} ({}) diff {d} exceeds tolerance",
        info.name,
        info.backend
    );
}
