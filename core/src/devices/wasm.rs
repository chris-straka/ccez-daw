//! The devices-to-WASM seam: named, refused, documented.
//!
//! Teaching note: native kernels ([`crate::devices::kernel`]) are portable
//! *source*; WASM would make them portable *binaries* — third-party DSP
//! shipped as `.wasm` modules the DAW loads without trusting. Loading
//! untrusted code is a sandbox decision, not a DSP one, so v1 draws the
//! boundary here and stops:
//!
//! - [`WasmNote`] records the evaluation: `wasmtime` is the standard
//!   embedding runtime for this (typed `Func` boundary, `Store` isolation,
//!   `no_std`-shaped kernels already fit its call convention), with
//!   `wasm3`/`wasmi` as lighter alternatives if startup time ever
//!   dominates. v1 adds zero new cargo dependencies — the same
//!   offline-friendly rule the CLAP host kept — and proves the kernel,
//!   rack, and preset machinery with native code first.
//! - [`load_wasm_module`] is the future entry point. Today it returns
//!   [`WasmError::WasmUnimplemented`], the same mark-and-refuse pattern
//!   Track B used for remote DSP (`RemoteUnimplemented`) and Track C for
//!   real CLAP bundles (`ClapUnimplemented`). Callers match on it; when
//!   the runtime lands, this function starts succeeding and no caller
//!   changes shape.
//!
//! A WASM backend, when it lands, speaks the kernel contract: mono `f32`
//! slices in, mono `f32` slices out, state in caller-owned bytes. The
//! kernels were written to that convention on purpose, so the seam is a
//! loader plus a memory copy — never a redesign.

/// Why [`load_wasm_module`] is a documented seam rather than live code.
/// See the module docs: `wasmtime` is the evaluated pick, zero new deps
/// is the v1 rule, and the kernel signatures already match the future
/// WASM call convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmNote;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmError {
    /// Third-party `.wasm` DSP is the phase-2 seam (see [`WasmNote`]).
    WasmUnimplemented(String),
    /// The module bytes failed validation (reserved: only reachable once
    /// loading is implemented; today loading refuses first).
    BadModule(String),
}

impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WasmUnimplemented(p) => write!(
                f,
                "WASM device `{p}` cannot load yet (wasmtime seam; use native kernels)"
            ),
            Self::BadModule(m) => write!(f, "bad WASM module: {m}"),
        }
    }
}

impl std::error::Error for WasmError {}

/// Future entry point for third-party `.wasm` DSP. Today it always
/// refuses with [`WasmError::WasmUnimplemented`] and loads nothing —
/// mark, refuse, never branch.
pub fn load_wasm_module(path: &str) -> Result<WasmNote, WasmError> {
    Err(WasmError::WasmUnimplemented(path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_loading_refuses_at_the_seam() {
        let err = load_wasm_module("mods/fuzz.wasm").expect_err("wasm must refuse");
        assert!(matches!(err, WasmError::WasmUnimplemented(_)));
        assert!(err.to_string().contains("fuzz.wasm"));
    }
}
