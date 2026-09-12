# Perf track primer: GPU compute abstraction (`core/src/gpu/`)

New to GPU audio? Start here. This note teaches the three ideas behind the
GPU track — **why a fallback is the feature**, **what the one real kernel
proves**, and **how device selection stays graceful** — then lists exactly
what this track owns so later perf work can build without re-deciding it.

## 1. The fallback is the feature

Audio can never depend on hardware that might not be there. A listener's
machine may have a discrete GPU, an integrated one, or nothing wgpu can
talk to (headless CI, servers, VMs). So the runner owns *both* paths:

- `KernelRunner::new(BackendPreference::Auto)` tries the GPU and silently
  keeps the CPU path on *any* failure, recording why in
  `fallback_reason()`. Rendering code calls `run_convolve()` and gets the
  right answer on every machine. Sound over speed, always.
- `ForceCpu` never touches the GPU — deterministic, for tests and the
  realtime thread. `ForceGpu` uses the GPU or returns `GpuError` instead
  of computing — for proving equivalence tests really exercised the GPU.

There is deliberately no mid-render retry: a GPU error is returned to the
caller, and the caller (e.g. the offline renderer) decides per block.

## 2. One real kernel: FIR convolution

`cpu_convolve` in `core/src/gpu/cpu.rs` is the CPU twin and the
specification: `output[i] = sum_k input[i-k] * ir[k]`, zero pre-padding
(the line starts at rest), `k` ascending. The WGSL shader in
`core/src/gpu/wgpu_backend.rs` (`CONVOLVE_WGSL`) is the same double loop
with the outer loop over `i` spread across GPU threads (one thread per
output sample, workgroup size 64) and the inner loop over `k` kept in the
*same order* so f32 rounding agrees.

Why convolution? It is the inner loop of a partitioned-convolution reverb
(a long impulse response chopped into blocks, each convolved and summed —
the standard GPU-reverb architecture). Proving GPU/CPU equivalence here
proves the building block a GPU reverb would be made of. `cpu_gain` rides
along to exercise the runner plumbing with the simplest data-parallel op;
note `run_gain` intentionally stays CPU — a GPU gain shader would spend
more on upload/readback than on the kernel itself.

Equivalence bar: per-sample absolute diff ≤ `EQUIVALENCE_TOLERANCE`
(1e-5), not bitwise identity — different hardware rounds the last bit
differently. Measured on Apple M4 / Metal: max diff 5.7e-6 over a 4096
frame sweep through a 256-tap decaying IR.

## 3. Device selection without drama

- `PowerPreference::{HighPerformance, LowPower}` maps to wgpu's adapter
  power classes (discrete vs integrated). `KernelRunner::with_power`
  threads the choice through; `power_preference()` round-trips it for UI.
- `KernelRunner::probe_devices()` enumerates adapters for selection UI.
  It never panics and returns empty (not an error) when the `gpu`
  feature is off or no adapters exist — an empty list is a valid answer.
- The whole wgpu pipeline is behind the `gpu` cargo feature, **off by
  default** (`core/Cargo.toml`). Plain `cargo test` and `bun run check`
  never need GPU toolchains; the feature-off `WgpuRunner` is a stub that
  returns `GpuError::Unavailable`, so fallback tests run everywhere.

## 4. What this track owns (frozen for others)

- `core/src/gpu/`: `mod.rs` (re-exports), `cpu.rs` (reference kernels +
  tolerance), `runner.rs` (`Backend`, `BackendPreference`,
  `GpuDeviceInfo`, `GpuError`, `KernelRunner`), `wgpu_backend.rs`
  (shader + blocking acquisition + stub).
- `core/Cargo.toml`: optional `wgpu 0.19` / `pollster 0.3` /
  `bytemuck 1` deps + `gpu = [...]` feature (default off).
- `core/tests/gpu_equivalence.rs`: sweep + impulse equivalence,
  strict-GPU graceful-failure, probe safety, backend-independent gain,
  and one `#[cfg(feature = "gpu")]` test pinning the claim to real
  hardware when present.
- No contract changes: no new `model`/`ipc` types, so `bun run typegen`
  output is untouched and the drift gate stays green.

## 5. Open threads (not this track)

- Per-call buffer upload/readback is correct, not fast: a production
  path wants persistent buffers and a block-ring, and must stay off the
  realtime thread (wgpu submission can block).
- `probe_devices` enumerates all backends; preferred-power ordering is
  best-effort. A device picker with persistence belongs in a UI track.
- Next kernels (partitioned IR scheduler, FFT analysis) reuse this exact
  runner shape: add `run_*` + CPU twin + tolerance test.
