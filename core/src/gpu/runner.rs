//! Kernel runner: backend selection with guaranteed CPU fallback.
//!
//! Teaching note: audio can never depend on hardware that might not be
//! there. So the runner owns *both* paths — a wgpu compute pipeline when
//! a GPU exists, and the scalar CPU reference always. Construction with
//! [`BackendPreference::Auto`] tries the GPU and silently keeps the CPU
//! path on any failure, recording why in [`KernelRunner::fallback_reason`].
//! Rendering code then calls [`KernelRunner::run_convolve`] and gets the
//! right answer on every machine, from a discrete GPU workstation to
//! headless CI.

use super::cpu::{cpu_convolve, cpu_gain};
use super::wgpu_backend::{PowerPreference, WgpuRunner};
use std::fmt;

/// Which compute backend actually produced the samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Scalar CPU reference kernel. Always available.
    Cpu,
    /// wgpu compute shader. Only when the `gpu` feature is on *and*
    /// adapter/device acquisition succeeded.
    Gpu,
}

/// What the caller wants at construction time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendPreference {
    /// Try GPU first, fall back to CPU on any failure. The right
    /// default for rendering: sound over speed.
    #[default]
    Auto,
    /// Never touch the GPU. Deterministic; use in tests and on the
    /// realtime thread if wgpu init cost is a concern.
    ForceCpu,
    /// Use the GPU or fail. Construction still succeeds (so callers
    /// can inspect [`KernelRunner::fallback_reason`]), but every
    /// `run_*` call returns the GPU error instead of falling back.
    /// Use to prove equivalence tests actually exercised the GPU.
    ForceGpu,
}

/// Info about one usable GPU, for device-selection UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDeviceInfo {
    /// Adapter name reported by the driver, e.g. "Apple M1 Pro".
    pub name: String,
    /// Backend label, e.g. "Metal", "Vulkan", "Dx12".
    pub backend: String,
}

/// Runner-level failure. Note there is no "device lost mid-render"
/// retry here by design: GPU errors are returned to the caller, and
/// the caller decides (the offline renderer falls back per-block).
#[derive(Debug, Clone, PartialEq)]
pub enum GpuError {
    /// No usable GPU: feature off, no adapter, or device request failed.
    /// The CPU fallback covers this transparently outside `ForceGpu`.
    Unavailable(String),
    /// The GPU ran but the work failed (shader, mapping, copy).
    Execution(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::Unavailable(s) => write!(f, "GPU unavailable: {s}"),
            GpuError::Execution(s) => write!(f, "GPU execution failed: {s}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Owns the CPU fallback state plus an optional live GPU pipeline.
///
/// Not `Clone`: a live `wgpu::Device` is expensive to duplicate, so
/// share via `Arc` if threads need it. All `run_*` methods take
/// `&self` and write into caller-owned slices (no allocation, matching
/// the realtime rules in [`crate::audio::render`]).
pub struct KernelRunner {
    backend: Backend,
    strict_gpu: bool,
    gpu: Option<WgpuRunner>,
    fallback_reason: Option<String>,
    power: PowerPreference,
}

impl KernelRunner {
    /// Build with default power preference ([`PowerPreference::HighPerformance`]).
    pub fn new(preference: BackendPreference) -> Self {
        Self::with_power(preference, PowerPreference::HighPerformance)
    }

    /// Build with an explicit adapter power preference (device selection).
    pub fn with_power(preference: BackendPreference, power: PowerPreference) -> Self {
        if matches!(preference, BackendPreference::ForceCpu) {
            return Self {
                backend: Backend::Cpu,
                strict_gpu: false,
                gpu: None,
                fallback_reason: None,
                power,
            };
        }
        match WgpuRunner::new_blocking(power) {
            Ok(gpu) => Self {
                backend: Backend::Gpu,
                strict_gpu: matches!(preference, BackendPreference::ForceGpu),
                gpu: Some(gpu),
                fallback_reason: None,
                power,
            },
            Err(e) => Self {
                backend: Backend::Cpu,
                strict_gpu: matches!(preference, BackendPreference::ForceGpu),
                gpu: None,
                fallback_reason: Some(e.to_string()),
                power,
            },
        }
    }

    /// The backend `run_*` calls will actually use, except that
    /// `ForceGpu` without a GPU returns errors instead of computing.
    pub fn backend(&self) -> Backend {
        if self.strict_gpu && self.gpu.is_none() {
            // Strict mode with no device: report Gpu so callers (and
            // tests) can see the GPU path was demanded but missing.
            return Backend::Gpu;
        }
        self.backend
    }

    /// Why the CPU fallback is active, if it is. `None` means the GPU
    /// path is live (or the CPU was explicitly forced, which needs no
    /// apology).
    pub fn fallback_reason(&self) -> Option<&str> {
        self.fallback_reason.as_deref()
    }

    /// Info about the live GPU device, if one is active.
    pub fn device_info(&self) -> Option<GpuDeviceInfo> {
        self.gpu.as_ref().map(|g| g.device_info())
    }

    /// Direct-form FIR convolution (the partitioned-reverb building block).
    /// See [`cpu_convolve`](super::cpu::cpu_convolve) for the math contract.
    pub fn run_convolve(
        &self,
        input: &[f32],
        ir: &[f32],
        output: &mut [f32],
    ) -> Result<(), GpuError> {
        match (&self.gpu, self.strict_gpu) {
            (Some(gpu), _) => gpu.run_convolve(input, ir, output),
            (None, true) => Err(GpuError::Unavailable(
                self.fallback_reason
                    .clone()
                    .unwrap_or_else(|| "no GPU device".to_string()),
            )),
            (None, false) => {
                cpu_convolve(input, ir, output);
                Ok(())
            }
        }
    }

    /// Elementwise gain. Same dispatch rules as [`Self::run_convolve`];
    /// currently always CPU (a GPU gain shader would add upload/readback
    /// cost above the kernel cost — documented here so nobody "optimizes"
    /// this onto the GPU blindly).
    pub fn run_gain(&self, input: &[f32], gain: f32, output: &mut [f32]) -> Result<(), GpuError> {
        if self.strict_gpu && self.gpu.is_none() {
            return Err(GpuError::Unavailable(
                self.fallback_reason
                    .clone()
                    .unwrap_or_else(|| "no GPU device".to_string()),
            ));
        }
        cpu_gain(input, gain, output);
        Ok(())
    }

    /// Enumerate usable GPUs without building a runner. Never panics;
    /// empty when the feature is off or no adapters exist.
    pub fn probe_devices(power: PowerPreference) -> Vec<GpuDeviceInfo> {
        // `power` is currently informational only (wgpu enumerates all
        // backends); kept in the signature so device-selection UI can
        // pass user preference through without a future break.
        let _ = power;
        WgpuRunner::probe_devices(power)
    }

    /// True when a GPU pipeline is live in this runner.
    pub fn is_gpu_live(&self) -> bool {
        self.gpu.is_some()
    }

    /// The adapter power preference this runner was built with.
    /// Used on the next GPU (re)acquisition; kept so device-selection
    /// UI can round-trip the user's choice through the runner.
    pub fn power_preference(&self) -> PowerPreference {
        self.power
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_cpu_never_touches_gpu() {
        let r = KernelRunner::new(BackendPreference::ForceCpu);
        assert_eq!(r.backend(), Backend::Cpu);
        assert!(r.fallback_reason().is_none());
        assert!(!r.is_gpu_live());
        let (input, ir) = ([1.0, 1.0, 1.0], [0.5, 0.25]);
        let mut out = [0.0; 3];
        r.run_convolve(&input, &ir, &mut out).unwrap();
        assert_eq!(out, [0.5, 0.75, 0.75]);
    }

    #[test]
    fn probe_devices_never_panics() {
        let _ = KernelRunner::probe_devices(PowerPreference::HighPerformance);
        let _ = KernelRunner::probe_devices(PowerPreference::LowPower);
    }

    #[test]
    fn error_display_is_human_readable() {
        assert!(GpuError::Unavailable("x".into()).to_string().contains("unavailable"));
        assert!(GpuError::Execution("y".into()).to_string().contains("failed"));
    }
}
