//! GPU compute abstraction: wgpu kernels with guaranteed CPU fallback.
//!
//! Teaching note: start at [`KernelRunner`](runner::KernelRunner). It picks
//! a backend ([`Backend`](runner::Backend)) from a caller preference, runs
//! the one real kernel — direct-form FIR convolution, the inner loop a
//! partitioned-convolution reverb is built from — and falls back to the
//! scalar CPU reference in [`cpu`] whenever no GPU exists. The wgpu
//! pipeline lives in [`wgpu_backend`] and only compiles with
//! `--features gpu` (off by default), so plain `cargo test` and
//! `bun run check` never need GPU toolchains.
//!
//! Map: [`cpu`] (reference kernels + equivalence tolerance),
//! [`runner`] (selection, fallback, dispatch), [`wgpu_backend`]
//! (compute shader + blocking device acquisition).

pub mod cpu;
pub mod runner;
pub mod wgpu_backend;

pub use cpu::{cpu_convolve, cpu_gain, EQUIVALENCE_TOLERANCE};
pub use runner::{Backend, BackendPreference, GpuDeviceInfo, GpuError, KernelRunner};
pub use wgpu_backend::PowerPreference;
