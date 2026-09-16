//! The devices-to-WASM seam: named, and now live behind `wasm-runtime`.
//!
//! Teaching note: native kernels ([`crate::devices::kernel`]) are portable
//! *source*; WASM makes them portable *binaries* — third-party DSP
//! shipped as `.wasm` modules the DAW loads without trusting. Loading
//! untrusted code is a sandbox decision, not a DSP one, so the boundary
//! stays narrow: the guest imports *nothing* (no WASI, no host
//! functions), params cross as `(code, f32)` scalars, and audio crosses
//! one `f32` per call — the same mono-`f32` kernel contract, one sample
//! at a time.
//!
//! - [`WasmNote`] records the original evaluation: `wasmtime` is the
//!   standard embedding runtime for this (typed `Func` boundary, `Store`
//!   isolation, `no_std`-shaped kernels already fit its call convention).
//! - [`load_wasm_module`] is the entry point. Without feature
//!   `wasm-runtime` it returns [`WasmError::WasmUnimplemented`] and loads
//!   nothing — the v1 mark-and-refuse rule, and default builds download
//!   and compile zero new dependencies. With the feature it reads,
//!   validates, and instantiates a gain guest (see below) and returns a
//!   live [`WasmGain`] — no caller changes shape either way.
//! - [`WasmGain`] (feature `wasm-runtime`) is the first guest device
//!   kind: the `gain_*` exports of the `core/wasm-guest` crate, driven
//!   through the kernel contract (`process` over mono `f32` slices,
//!   caller-owned state) with params hydrated from the frozen `Node`
//!   shape. Gain is stateless on purpose, so this device proves the
//!   boundary — load, instantiate, param, process — against
//!   [`kernel::gain_process`](crate::devices::kernel::gain_process),
//!   never DSP state.
//!
//! A production WASM backend with more device kinds follows this exact
//! recipe: port the kernel to the guest crate, extend the code map,
//! mirror the narrow init/param/reset/sample host below. Delay
//! ([`WasmKDelay`], the `kdelay_*` exports) and soft-clip ([`WasmClip`],
//! the `clip_*` exports) are the first two followers: same recipe,
//! same roundtrip proof, separate export prefixes so the sandbox's
//! millisecond `delay_*` device keeps its own namespace.

/// Why [`load_wasm_module`] refuses without the `wasm-runtime` feature.
/// See the module docs: `wasmtime` is the evaluated pick, zero new deps
/// is the default-build rule, and the kernel signatures already match
/// the WASM call convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmNote;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmError {
    /// Third-party `.wasm` DSP without the `wasm-runtime` feature (or a
    /// build that left it off): mark, refuse, never branch.
    WasmUnimplemented(String),
    /// The module bytes failed validation (bad magic, missing gain
    /// exports, unreadable file) — the culprit is attached.
    BadModule(String),
    /// `input.len() != output.len()` — the kernel block rule, shared so
    /// guest renders refuse ragged blocks exactly like native ones.
    BadBlock(String),
    /// A single param set named no gain param (typos must surface, the
    /// [`crate::devices::class::set_param_value`] rule).
    UnknownParam(String),
    /// The wasmtime backend failed (instantiate or a trapped call).
    Backend(String),
}

impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WasmUnimplemented(p) => write!(
                f,
                "WASM device `{p}` cannot load yet (wasmtime seam; use native kernels)"
            ),
            Self::BadModule(m) => write!(f, "bad WASM module: {m}"),
            Self::BadBlock(m) => write!(f, "bad WASM block: {m}"),
            Self::UnknownParam(p) => write!(f, "unknown WASM param `{p}`"),
            Self::Backend(m) => write!(f, "WASM backend failed: {m}"),
        }
    }
}

impl std::error::Error for WasmError {}

/// Entry point for third-party `.wasm` DSP without the `wasm-runtime`
/// feature: always refuses with [`WasmError::WasmUnimplemented`] and
/// loads nothing — mark, refuse, never branch.
#[cfg(not(feature = "wasm-runtime"))]
pub fn load_wasm_module(path: &str) -> Result<WasmNote, WasmError> {
    Err(WasmError::WasmUnimplemented(path.to_string()))
}

/// Entry point for third-party `.wasm` DSP with the `wasm-runtime`
/// feature: reads `path`, validates and instantiates the gain guest,
/// and returns it at unity gain (hydrate params with
/// [`WasmGain::apply_node`]).
#[cfg(feature = "wasm-runtime")]
pub fn load_wasm_module(path: &str) -> Result<WasmGain, WasmError> {
    let bytes = std::fs::read(path)
        .map_err(|e| WasmError::BadModule(format!("cannot read `{path}`: {e}")))?;
    WasmGain::from_bytes(&bytes)
        .map_err(|e| WasmError::BadModule(format!("`{path}`: {e}")))
}

/// One loaded gain guest: a wasmtime instance behind the kernel contract.
///
/// Boundary design (from the wasmtime embedding docs: `Engine` +
/// `Module` + empty-`Linker` + `Store`, typed `Func`s, no imports): the
/// guest exports `gain_init`, `gain_param`, `gain_reset`, and
/// `gain_sample` and imports *nothing*, so instantiation needs only an
/// empty `Linker` and the guest cannot reach the host except through the
/// four typed signatures. Requires feature `wasm-runtime`.
#[cfg(feature = "wasm-runtime")]
pub struct WasmGain {
    store: wasmtime::Store<()>,
    set_param_fn: wasmtime::TypedFunc<(i32, f32), i32>,
    reset_fn: wasmtime::TypedFunc<(), ()>,
    sample_fn: wasmtime::TypedFunc<f32, f32>,
    gain: f32,
}

#[cfg(feature = "wasm-runtime")]
impl std::fmt::Debug for WasmGain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmGain").field("gain", &self.gain).finish()
    }
}

/// Param code for `gain` on the guest boundary (mirrors the guest
/// crate's `GAIN_CODE_GAIN`; append-only like the device-class codes).
#[cfg(feature = "wasm-runtime")]
pub const WASM_GAIN_CODE: i32 = 0;

/// Top of the frozen `Gain` node range (`class::default_params`):
/// single sets clamp here, the same clamp-not-error rule as
/// `set_param_value`.
#[cfg(feature = "wasm-runtime")]
pub const WASM_GAIN_MAX: f64 = 4.0;

#[cfg(feature = "wasm-runtime")]
impl WasmGain {
    /// Read `path` and instantiate the gain guest at unity gain.
    pub fn load(path: &str) -> Result<Self, WasmError> {
        load_wasm_module(path)
    }

    /// Instantiate the gain guest from already-loaded bytes (what
    /// [`load_wasm_module`] reads off disk; tests pass staged bytes
    /// directly so the gate is hermetic).
    pub fn from_bytes(module_bytes: &[u8]) -> Result<Self, WasmError> {
        let engine =
            wasmtime::Engine::new(&wasmtime::Config::new()).map_err(|e| WasmError::Backend(e.to_string()))?;
        let module = wasmtime::Module::new(&engine, module_bytes)
            .map_err(|e| WasmError::BadModule(format!("bad module: {e}")))?;
        let mut store = wasmtime::Store::new(&engine, ());
        // The guest imports nothing, so the linker defines nothing.
        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| WasmError::Backend(format!("instantiate: {e}")))?;
        let init = instance
            .get_typed_func::<f32, ()>(&mut store, "gain_init")
            .map_err(|e| WasmError::BadModule(format!("missing gain_init: {e}")))?;
        let set_param_fn = instance
            .get_typed_func::<(i32, f32), i32>(&mut store, "gain_param")
            .map_err(|e| WasmError::BadModule(format!("missing gain_param: {e}")))?;
        let reset_fn = instance
            .get_typed_func::<(), ()>(&mut store, "gain_reset")
            .map_err(|e| WasmError::BadModule(format!("missing gain_reset: {e}")))?;
        let sample_fn = instance
            .get_typed_func::<f32, f32>(&mut store, "gain_sample")
            .map_err(|e| WasmError::BadModule(format!("missing gain_sample: {e}")))?;
        init
            .call(&mut store, 1.0)
            .map_err(|e| WasmError::Backend(format!("gain_init: {e}")))?;
        Ok(Self {
            store,
            set_param_fn,
            reset_fn,
            sample_fn,
            gain: 1.0,
        })
    }

    /// Current gain (host-side cache; mirrors the guest).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Set one param by frozen-`Node` id: only
    /// [`GAIN_PARAM`](crate::devices::kernel::GAIN_PARAM) (`"gain"`)
    /// exists — anything else is [`WasmError::UnknownParam`] and the
    /// stored gain is untouched. Values clamp to `0.0..=4.0` (the frozen
    /// `Gain` node range), never erroring.
    pub fn set_param(&mut self, id: &str, value: f64) -> Result<(), WasmError> {
        if id != crate::devices::kernel::GAIN_PARAM {
            return Err(WasmError::UnknownParam(id.to_string()));
        }
        let v = value.clamp(0.0, WASM_GAIN_MAX) as f32;
        let rc = self
            .set_param_fn
            .call(&mut self.store, (WASM_GAIN_CODE, v))
            .map_err(|e| WasmError::Backend(format!("gain_param: {e}")))?;
        if rc != 0 {
            return Err(WasmError::UnknownParam(format!("{id} refused (rc {rc})")));
        }
        self.gain = v;
        Ok(())
    }

    /// Hydrate from a frozen [`Node`](crate::model::Node): the `gain`
    /// param applies (clamped); unknown ids skip, not errors — the
    /// bulk-load tolerance native `from_node` shows. Returns the number
    /// of params applied (0 or 1).
    pub fn apply_node(&mut self, node: &crate::model::Node) -> usize {
        match node
            .params
            .iter()
            .find(|p| p.id == crate::devices::kernel::GAIN_PARAM)
        {
            // `set_param` cannot refuse the one known id; a trap would
            // mean the instance is dead, in which case every later call
            // errors anyway.
            Some(p) => {
                let _ = self.set_param(&p.id.clone(), p.value);
                1
            }
            None => 0,
        }
    }

    /// Render one block through the guest: `out = in * gain`.
    /// `output.len() == input.len()` or [`WasmError::BadBlock`]; empty
    /// blocks are a legal no-op (the kernel rule).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), WasmError> {
        if input.len() != output.len() {
            return Err(WasmError::BadBlock(format!(
                "input {} frames != output {} frames",
                input.len(),
                output.len()
            )));
        }
        for (o, i) in output.iter_mut().zip(input.iter()) {
            *o = self
                .sample_fn
                .call(&mut self.store, *i)
                .map_err(|e| WasmError::Backend(format!("gain_sample: {e}")))?;
        }
        Ok(())
    }

    /// Back to unity gain, in the guest and the cache.
    pub fn reset(&mut self) {
        let _ = self.reset_fn.call(&mut self.store, ());
        self.gain = 1.0;
    }
}

/// Param code for `delay_samples` on the guest boundary (mirrors the
/// guest crate's `KDELAY_CODE_DELAY_SAMPLES`; append-only).
#[cfg(feature = "wasm-runtime")]
pub const WASM_KDELAY_CODE_DELAY: i32 = 0;
/// Param code for `feedback` on the guest boundary (mirrors the guest
/// crate's `KDELAY_CODE_FEEDBACK`; append-only).
#[cfg(feature = "wasm-runtime")]
pub const WASM_KDELAY_CODE_FEEDBACK: i32 = 1;

/// Top of the frozen `Delay` node range (`class::default_params`):
/// single sets clamp here, the same clamp-not-error rule as
/// `set_param_value`.
#[cfg(feature = "wasm-runtime")]
pub const WASM_KDELAY_MAX_DELAY: f64 = 48000.0;

/// One loaded kernel-delay guest: a wasmtime instance behind the
/// kernel contract.
///
/// The guest exports `kdelay_init`, `kdelay_param`, `kdelay_reset`,
/// and `kdelay_sample` and imports *nothing*, so instantiation needs
/// only an empty `Linker` — the same boundary as [`WasmGain`], with
/// the loop body ported from
/// [`kernel::delay_process`](crate::devices::kernel::delay_process)
/// (`y[n] = x[n] + fb * y[n-D]`, `0` samples bypasses the line).
/// Requires feature `wasm-runtime`.
#[cfg(feature = "wasm-runtime")]
pub struct WasmKDelay {
    store: wasmtime::Store<()>,
    set_param_fn: wasmtime::TypedFunc<(i32, f32), i32>,
    reset_fn: wasmtime::TypedFunc<(), ()>,
    sample_fn: wasmtime::TypedFunc<f32, f32>,
    delay_samples: usize,
    feedback: f32,
}

#[cfg(feature = "wasm-runtime")]
impl std::fmt::Debug for WasmKDelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmKDelay")
            .field("delay_samples", &self.delay_samples)
            .field("feedback", &self.feedback)
            .finish()
    }
}

#[cfg(feature = "wasm-runtime")]
impl WasmKDelay {
    /// Read `path` and instantiate the kernel-delay guest at the
    /// `Delay` class defaults (`delay 0`, `feedback 0`).
    pub fn load(path: &str) -> Result<Self, WasmError> {
        let bytes = std::fs::read(path)
            .map_err(|e| WasmError::BadModule(format!("cannot read `{path}`: {e}")))?;
        Self::from_bytes(&bytes).map_err(|e| WasmError::BadModule(format!("`{path}`: {e}")))
    }

    /// Instantiate the kernel-delay guest from already-loaded bytes
    /// (tests pass staged bytes directly so the gate is hermetic).
    pub fn from_bytes(module_bytes: &[u8]) -> Result<Self, WasmError> {
        let engine =
            wasmtime::Engine::new(&wasmtime::Config::new()).map_err(|e| WasmError::Backend(e.to_string()))?;
        let module = wasmtime::Module::new(&engine, module_bytes)
            .map_err(|e| WasmError::BadModule(format!("bad module: {e}")))?;
        let mut store = wasmtime::Store::new(&engine, ());
        // The guest imports nothing, so the linker defines nothing.
        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| WasmError::Backend(format!("instantiate: {e}")))?;
        let init = instance
            .get_typed_func::<(f32, f32), ()>(&mut store, "kdelay_init")
            .map_err(|e| WasmError::BadModule(format!("missing kdelay_init: {e}")))?;
        let set_param_fn = instance
            .get_typed_func::<(i32, f32), i32>(&mut store, "kdelay_param")
            .map_err(|e| WasmError::BadModule(format!("missing kdelay_param: {e}")))?;
        let reset_fn = instance
            .get_typed_func::<(), ()>(&mut store, "kdelay_reset")
            .map_err(|e| WasmError::BadModule(format!("missing kdelay_reset: {e}")))?;
        let sample_fn = instance
            .get_typed_func::<f32, f32>(&mut store, "kdelay_sample")
            .map_err(|e| WasmError::BadModule(format!("missing kdelay_sample: {e}")))?;
        init
            .call(&mut store, (0.0, 0.0))
            .map_err(|e| WasmError::Backend(format!("kdelay_init: {e}")))?;
        Ok(Self {
            store,
            set_param_fn,
            reset_fn,
            sample_fn,
            delay_samples: 0,
            feedback: 0.0,
        })
    }

    /// Current delay in samples (host-side cache; mirrors the guest).
    pub fn delay_samples(&self) -> usize {
        self.delay_samples
    }

    /// Current feedback (host-side cache; mirrors the guest).
    pub fn feedback(&self) -> f32 {
        self.feedback
    }

    /// Set one param by frozen-`Node` id:
    /// [`DELAY_SAMPLES_PARAM`](crate::devices::kernel::DELAY_SAMPLES_PARAM)
    /// (`"delay_samples"`, clamped to `0.0..=48000.0`) or
    /// [`FEEDBACK_PARAM`](crate::devices::kernel::FEEDBACK_PARAM)
    /// (`"feedback"`, clamped to `0.0..=0.95`) — anything else is
    /// [`WasmError::UnknownParam`] and the stored params are untouched.
    pub fn set_param(&mut self, id: &str, value: f64) -> Result<(), WasmError> {
        let (code, v) = if id == crate::devices::kernel::DELAY_SAMPLES_PARAM {
            (WASM_KDELAY_CODE_DELAY, value.clamp(0.0, WASM_KDELAY_MAX_DELAY) as f32)
        } else if id == crate::devices::kernel::FEEDBACK_PARAM {
            (
                WASM_KDELAY_CODE_FEEDBACK,
                value.clamp(0.0, crate::devices::kernel::MAX_FEEDBACK) as f32,
            )
        } else {
            return Err(WasmError::UnknownParam(id.to_string()));
        };
        let rc = self
            .set_param_fn
            .call(&mut self.store, (code, v))
            .map_err(|e| WasmError::Backend(format!("kdelay_param: {e}")))?;
        if rc != 0 {
            return Err(WasmError::UnknownParam(format!("{id} refused (rc {rc})")));
        }
        // The cache follows the guest's own conversion so host-side
        // reads stay exact (same truncation the guest applies).
        if code == WASM_KDELAY_CODE_DELAY {
            self.delay_samples = (v as f64).max(0.0).clamp(0.0, WASM_KDELAY_MAX_DELAY) as usize;
        } else {
            self.feedback = v;
        }
        Ok(())
    }

    /// Hydrate from a frozen [`Node`](crate::model::Node): the
    /// `delay_samples` and `feedback` params apply (clamped); unknown
    /// ids skip, not errors — the bulk-load tolerance native
    /// `from_node` shows. Returns the number of params applied (0..=2).
    pub fn apply_node(&mut self, node: &crate::model::Node) -> usize {
        let mut applied = 0;
        for p in &node.params {
            if p.id == crate::devices::kernel::DELAY_SAMPLES_PARAM
                || p.id == crate::devices::kernel::FEEDBACK_PARAM
            {
                // `set_param` cannot refuse either known id; a trap would
                // mean the instance is dead, in which case every later
                // call errors anyway.
                let _ = self.set_param(&p.id.clone(), p.value);
                applied += 1;
            }
        }
        applied
    }

    /// Render one block through the guest: the feedback comb, one
    /// `kdelay_sample` call per frame. `output.len() == input.len()`
    /// or [`WasmError::BadBlock`]; empty blocks are a legal no-op
    /// (the kernel rule).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), WasmError> {
        if input.len() != output.len() {
            return Err(WasmError::BadBlock(format!(
                "input {} frames != output {} frames",
                input.len(),
                output.len()
            )));
        }
        for (o, i) in output.iter_mut().zip(input.iter()) {
            *o = self
                .sample_fn
                .call(&mut self.store, *i)
                .map_err(|e| WasmError::Backend(format!("kdelay_sample: {e}")))?;
        }
        Ok(())
    }

    /// Back to the `Delay` class defaults, in the guest and the cache.
    pub fn reset(&mut self) {
        let _ = self.reset_fn.call(&mut self.store, ());
        self.delay_samples = 0;
        self.feedback = 0.0;
    }
}

/// Param code for `drive` on the guest boundary (mirrors the guest
/// crate's `CLIP_CODE_DRIVE`; append-only like the device-class codes).
#[cfg(feature = "wasm-runtime")]
pub const WASM_CLIP_CODE: i32 = 0;

/// Top of the frozen `Distortion` node range
/// (`class::default_params`): single sets clamp here, the same
/// clamp-not-error rule as `set_param_value`.
#[cfg(feature = "wasm-runtime")]
pub const WASM_CLIP_MAX: f64 = 10.0;

/// Default drive of the frozen `Distortion` node: what [`WasmClip`]
/// instantiates at and [`WasmClip::reset`] restores.
#[cfg(feature = "wasm-runtime")]
pub const WASM_CLIP_DEFAULT: f64 = 1.0;

/// One loaded soft-clip guest: a wasmtime instance behind the kernel
/// contract.
///
/// The guest exports `clip_init`, `clip_param`, `clip_reset`, and
/// `clip_sample` and imports *nothing*, so instantiation needs only an
/// empty `Linker` — the same boundary as [`WasmGain`], with the
/// waveshaper ported from
/// [`kernel::distortion_process`](crate::devices::kernel::distortion_process)
/// (`y = (1+k)*x / (1+k*|x|)`). Stateless like gain, so this device
/// proves the boundary against the shaper formula, never DSP state.
/// Requires feature `wasm-runtime`.
#[cfg(feature = "wasm-runtime")]
pub struct WasmClip {
    store: wasmtime::Store<()>,
    set_param_fn: wasmtime::TypedFunc<(i32, f32), i32>,
    reset_fn: wasmtime::TypedFunc<(), ()>,
    sample_fn: wasmtime::TypedFunc<f32, f32>,
    drive: f32,
}

#[cfg(feature = "wasm-runtime")]
impl std::fmt::Debug for WasmClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmClip").field("drive", &self.drive).finish()
    }
}

#[cfg(feature = "wasm-runtime")]
impl WasmClip {
    /// Read `path` and instantiate the soft-clip guest at the default
    /// drive.
    pub fn load(path: &str) -> Result<Self, WasmError> {
        let bytes = std::fs::read(path)
            .map_err(|e| WasmError::BadModule(format!("cannot read `{path}`: {e}")))?;
        Self::from_bytes(&bytes).map_err(|e| WasmError::BadModule(format!("`{path}`: {e}")))
    }

    /// Instantiate the soft-clip guest from already-loaded bytes (what
    /// [`WasmClip::load`] reads off disk; tests pass staged bytes
    /// directly so the gate is hermetic).
    pub fn from_bytes(module_bytes: &[u8]) -> Result<Self, WasmError> {
        let engine =
            wasmtime::Engine::new(&wasmtime::Config::new()).map_err(|e| WasmError::Backend(e.to_string()))?;
        let module = wasmtime::Module::new(&engine, module_bytes)
            .map_err(|e| WasmError::BadModule(format!("bad module: {e}")))?;
        let mut store = wasmtime::Store::new(&engine, ());
        // The guest imports nothing, so the linker defines nothing.
        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| WasmError::Backend(format!("instantiate: {e}")))?;
        let init = instance
            .get_typed_func::<f32, ()>(&mut store, "clip_init")
            .map_err(|e| WasmError::BadModule(format!("missing clip_init: {e}")))?;
        let set_param_fn = instance
            .get_typed_func::<(i32, f32), i32>(&mut store, "clip_param")
            .map_err(|e| WasmError::BadModule(format!("missing clip_param: {e}")))?;
        let reset_fn = instance
            .get_typed_func::<(), ()>(&mut store, "clip_reset")
            .map_err(|e| WasmError::BadModule(format!("missing clip_reset: {e}")))?;
        let sample_fn = instance
            .get_typed_func::<f32, f32>(&mut store, "clip_sample")
            .map_err(|e| WasmError::BadModule(format!("missing clip_sample: {e}")))?;
        init
            .call(&mut store, WASM_CLIP_DEFAULT as f32)
            .map_err(|e| WasmError::Backend(format!("clip_init: {e}")))?;
        Ok(Self {
            store,
            set_param_fn,
            reset_fn,
            sample_fn,
            drive: WASM_CLIP_DEFAULT as f32,
        })
    }

    /// Current drive (host-side cache; mirrors the guest).
    pub fn drive(&self) -> f32 {
        self.drive
    }

    /// Set one param by frozen-`Node` id: only
    /// [`DRIVE_PARAM`](crate::devices::kernel::DRIVE_PARAM) (`"drive"`)
    /// exists — anything else is [`WasmError::UnknownParam`] and the
    /// stored drive is untouched. Values clamp to `0.0..=10.0` (the
    /// frozen `Distortion` node range), never erroring.
    pub fn set_param(&mut self, id: &str, value: f64) -> Result<(), WasmError> {
        if id != crate::devices::kernel::DRIVE_PARAM {
            return Err(WasmError::UnknownParam(id.to_string()));
        }
        let v = value.clamp(0.0, WASM_CLIP_MAX) as f32;
        let rc = self
            .set_param_fn
            .call(&mut self.store, (WASM_CLIP_CODE, v))
            .map_err(|e| WasmError::Backend(format!("clip_param: {e}")))?;
        if rc != 0 {
            return Err(WasmError::UnknownParam(format!("{id} refused (rc {rc})")));
        }
        self.drive = v;
        Ok(())
    }

    /// Hydrate from a frozen [`Node`](crate::model::Node): the `drive`
    /// param applies (clamped); unknown ids skip, not errors — the
    /// bulk-load tolerance native `from_node` shows. Returns the number
    /// of params applied (0 or 1).
    pub fn apply_node(&mut self, node: &crate::model::Node) -> usize {
        match node
            .params
            .iter()
            .find(|p| p.id == crate::devices::kernel::DRIVE_PARAM)
        {
            // `set_param` cannot refuse the one known id; a trap would
            // mean the instance is dead, in which case every later call
            // errors anyway.
            Some(p) => {
                let _ = self.set_param(&p.id.clone(), p.value);
                1
            }
            None => 0,
        }
    }

    /// Render one block through the guest: `out = shape(in, drive)`.
    /// `output.len() == input.len()` or [`WasmError::BadBlock`]; empty
    /// blocks are a legal no-op (the kernel rule).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), WasmError> {
        if input.len() != output.len() {
            return Err(WasmError::BadBlock(format!(
                "input {} frames != output {} frames",
                input.len(),
                output.len()
            )));
        }
        for (o, i) in output.iter_mut().zip(input.iter()) {
            *o = self
                .sample_fn
                .call(&mut self.store, *i)
                .map_err(|e| WasmError::Backend(format!("clip_sample: {e}")))?;
        }
        Ok(())
    }

    /// Back to the default drive, in the guest and the cache.
    pub fn reset(&mut self) {
        let _ = self.reset_fn.call(&mut self.store, ());
        self.drive = WASM_CLIP_DEFAULT as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "wasm-runtime"))]
    #[test]
    fn wasm_loading_refuses_at_the_seam() {
        let err = load_wasm_module("mods/fuzz.wasm").expect_err("wasm must refuse");
        assert!(matches!(err, WasmError::WasmUnimplemented(_)));
        assert!(err.to_string().contains("fuzz.wasm"));
    }

    #[cfg(feature = "wasm-runtime")]
    mod runtime {
        use super::*;
        use crate::devices::class::{self, DeviceClass};
        use crate::devices::kernel;

        /// The staged guest bytes: `core/wasm-guest` built for
        /// `wasm32-unknown-unknown` by `scripts/build-wasm-guest.sh`
        /// (committed, so this gate is hermetic).
        const GUEST: &[u8] = include_bytes!("../wasmdevices/testdata/delay_guest.wasm");

        fn gain_node(id: &str, gain: f64) -> crate::model::Node {
            let mut node = class::instantiate(DeviceClass::Gain, id, "WASM Gain");
            class::set_param_value(&mut node, kernel::GAIN_PARAM, gain).unwrap();
            node
        }

        fn delay_node(id: &str, delay: f64, feedback: f64) -> crate::model::Node {
            let mut node = class::instantiate(DeviceClass::Delay, id, "WASM Delay");
            class::set_param_value(&mut node, kernel::DELAY_SAMPLES_PARAM, delay).unwrap();
            class::set_param_value(&mut node, kernel::FEEDBACK_PARAM, feedback).unwrap();
            node
        }

        fn clip_node(id: &str, drive: f64) -> crate::model::Node {
            let mut node = class::instantiate(DeviceClass::Distortion, id, "WASM Clip");
            class::set_param_value(&mut node, kernel::DRIVE_PARAM, drive).unwrap();
            node
        }

        fn impulse(frames: usize) -> Vec<f32> {
            let mut v = vec![0.0; frames];
            if !v.is_empty() {
                v[0] = 1.0;
            }
            v
        }

        #[test]
        fn load_refuses_missing_files_and_bad_bytes() {
            let err = load_wasm_module("mods/fuzz.wasm").expect_err("missing file must refuse");
            assert!(matches!(err, WasmError::BadModule(_)));
            assert!(err.to_string().contains("fuzz.wasm"));

            let err = WasmGain::from_bytes(b"not a module").expect_err("junk must refuse");
            assert!(matches!(err, WasmError::BadModule(_)));

            // Valid module, wrong shape: no gain exports.
            let empty: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
            let err = WasmGain::from_bytes(empty).expect_err("export-less module must refuse");
            assert!(matches!(err, WasmError::BadModule(_)));
            assert!(err.to_string().contains("gain_"));
        }

        #[test]
        fn blocks_and_params_refuse_like_native() {
            let mut dev = WasmGain::from_bytes(GUEST).unwrap();
            // Ragged blocks refuse (the kernel rule).
            let input = vec![0.5; 64];
            let mut output = vec![0.0; 63];
            let err = dev.process(&input, &mut output).expect_err("ragged must refuse");
            assert!(matches!(err, WasmError::BadBlock(_)));
            // Empty blocks are a legal no-op.
            dev.process(&[], &mut []).unwrap();
            // Typos surface; the stored gain is untouched.
            let err = dev.set_param("cutoff", 440.0).expect_err("typo must refuse");
            assert!(matches!(err, WasmError::UnknownParam(_)));
            assert_eq!(dev.gain(), 1.0);
            // Bulk loads skip unknown ids instead.
            let mut node = gain_node("g", 0.5);
            node.params.push(crate::model::Param {
                id: "wet".to_string(),
                label: "Wet".to_string(),
                value: 0.9,
                min: 0.0,
                max: 1.0,
                default: 1.0,
                unit: String::new(),
            });
            assert_eq!(dev.apply_node(&node), 1);
            assert_eq!(dev.gain(), 0.5);
        }

        #[test]
        fn gain_roundtrip_matches_native_kernel() {
            // Load + instantiate + hydrate from the frozen Node shape,
            // then render sample-identical output to the native kernel.
            let mut dev = WasmGain::from_bytes(GUEST).unwrap();
            assert_eq!(dev.apply_node(&gain_node("g", 0.5)), 1);
            let input: Vec<f32> = (0..1024)
                .map(|t| (t as f32 * 220.0 * core::f32::consts::TAU / 48_000.0).sin())
                .collect();
            let (mut want, mut got) = (vec![0.0; 1024], vec![0.0; 1024]);
            kernel::gain_process(0.5, &input, &mut want).unwrap();
            dev.process(&input, &mut got).unwrap();
            assert_eq!(got, want);

            // A mid-stream single set steers the next block.
            dev.set_param(kernel::GAIN_PARAM, 2.0).unwrap();
            kernel::gain_process(2.0, &input, &mut want).unwrap();
            dev.process(&input, &mut got).unwrap();
            assert_eq!(got, want);

            // Reset is unity again.
            dev.reset();
            assert_eq!(dev.gain(), 1.0);
            kernel::gain_process(1.0, &input, &mut want).unwrap();
            dev.process(&input, &mut got).unwrap();
            assert_eq!(got, want);
        }

        #[test]
        fn kdelay_blocks_and_params_refuse_like_native() {
            let mut dev = WasmKDelay::from_bytes(GUEST).unwrap();
            // Ragged blocks refuse (the kernel rule).
            let input = vec![0.5; 64];
            let mut output = vec![0.0; 63];
            let err = dev.process(&input, &mut output).expect_err("ragged must refuse");
            assert!(matches!(err, WasmError::BadBlock(_)));
            // Empty blocks are a legal no-op.
            dev.process(&[], &mut []).unwrap();
            // Typos surface; the stored params are untouched.
            let err = dev.set_param("cutoff", 440.0).expect_err("typo must refuse");
            assert!(matches!(err, WasmError::UnknownParam(_)));
            assert_eq!((dev.delay_samples(), dev.feedback()), (0, 0.0));
            // Missing files refuse at the loader, too.
            assert!(matches!(
                WasmKDelay::load("mods/nope.wasm").expect_err("missing file must refuse"),
                WasmError::BadModule(_)
            ));
            // Bulk loads skip unknown ids instead.
            let mut node = delay_node("d", 4.0, 0.5);
            node.params.push(crate::model::Param {
                id: "wet".to_string(),
                label: "Wet".to_string(),
                value: 0.9,
                min: 0.0,
                max: 1.0,
                default: 1.0,
                unit: String::new(),
            });
            assert_eq!(dev.apply_node(&node), 2);
            assert_eq!((dev.delay_samples(), dev.feedback()), (4, 0.5));
            // Clamps, never errors: delay floors at 0, feedback at 0.95.
            dev.set_param(kernel::DELAY_SAMPLES_PARAM, -3.0).unwrap();
            dev.set_param(kernel::FEEDBACK_PARAM, 99.0).unwrap();
            assert_eq!((dev.delay_samples(), dev.feedback()), (0, 0.95));
        }

        #[test]
        fn kdelay_roundtrip_matches_native_kernel() {
            // Load + instantiate + hydrate from the frozen Node shape,
            // then render sample-identical output to the native kernel —
            // exact equality, not a tolerance: the ported loop is
            // op-for-op identical, so any drift is a port bug.
            for (delay, fb) in [(0.0, 0.0), (4.0, 0.5), (64.0, 0.95), (512.0, 0.3)] {
                let mut dev = WasmKDelay::from_bytes(GUEST).unwrap();
                assert_eq!(dev.apply_node(&delay_node("d", delay, fb)), 2);
                assert_eq!(dev.delay_samples(), delay as usize);
                let d = delay as usize;
                let mut native = kernel::DelayState::default();

                // Impulse: echoes land on the same grid with the same
                // decay. Rendered in small blocks, so echoes prove they
                // survive block boundaries on both sides.
                let frames = d * 3 + 64;
                let input = impulse(frames);
                let (mut want, mut got) = (vec![0.0; frames], vec![0.0; frames]);
                for ((w, g), b) in want
                    .chunks_mut(64)
                    .zip(got.chunks_mut(64))
                    .zip(input.chunks(64))
                {
                    kernel::delay_process(d, fb, &mut native, b, w).unwrap();
                    dev.process(b, g).unwrap();
                }
                assert_eq!(got, want, "impulse drift at d={delay} fb={fb}");

                // Sine across two blocks: state carries over identically.
                let sine: Vec<f32> = (0..1024)
                    .map(|t| (t as f32 * 220.0 * core::f32::consts::TAU / 48_000.0).sin())
                    .collect();
                for block in sine.chunks(512) {
                    let (mut w, mut g) = (vec![0.0; block.len()], vec![0.0; block.len()]);
                    kernel::delay_process(d, fb, &mut native, block, &mut w).unwrap();
                    dev.process(block, &mut g).unwrap();
                    assert_eq!(g, w, "sine drift at d={delay} fb={fb}");
                }

                // A mid-stream param change steers both renders together
                // (the native `ensure` and the guest re-allocate the line
                // on the same resize, clearing stale echoes together).
                dev.set_param(kernel::DELAY_SAMPLES_PARAM, 8.0).unwrap();
                dev.set_param(kernel::FEEDBACK_PARAM, 0.25).unwrap();
                let silence = vec![0.0; 128];
                let (mut w, mut g) = (vec![0.0; 128], vec![0.0; 128]);
                kernel::delay_process(8, 0.25, &mut native, &silence, &mut w).unwrap();
                dev.process(&silence, &mut g).unwrap();
                assert_eq!(g, w, "param-change drift from d={delay} fb={fb}");

                // Reset is the Delay class defaults again.
                dev.reset();
                assert_eq!((dev.delay_samples(), dev.feedback()), (0, 0.0));
                let (mut w2, mut g2) = (vec![0.0; 1024], vec![0.0; 1024]);
                kernel::delay_process(0, 0.0, &mut kernel::DelayState::default(), &sine, &mut w2)
                    .unwrap();
                dev.process(&sine, &mut g2).unwrap();
                assert_eq!(g2, w2);
            }
        }

        #[test]
        fn clip_blocks_and_params_refuse_like_native() {
            let mut dev = WasmClip::from_bytes(GUEST).unwrap();
            // Ragged blocks refuse (the kernel rule).
            let input = vec![0.5; 64];
            let mut output = vec![0.0; 63];
            let err = dev.process(&input, &mut output).expect_err("ragged must refuse");
            assert!(matches!(err, WasmError::BadBlock(_)));
            // Empty blocks are a legal no-op.
            dev.process(&[], &mut []).unwrap();
            // Typos surface; the stored drive is untouched.
            let err = dev.set_param("cutoff", 440.0).expect_err("typo must refuse");
            assert!(matches!(err, WasmError::UnknownParam(_)));
            assert_eq!(dev.drive(), 1.0);
            // Missing files refuse at the loader, too.
            assert!(matches!(
                WasmClip::load("mods/nope.wasm").expect_err("missing file must refuse"),
                WasmError::BadModule(_)
            ));
            // Bulk loads skip unknown ids instead.
            let mut node = clip_node("c", 5.0);
            node.params.push(crate::model::Param {
                id: "wet".to_string(),
                label: "Wet".to_string(),
                value: 0.9,
                min: 0.0,
                max: 1.0,
                default: 1.0,
                unit: String::new(),
            });
            assert_eq!(dev.apply_node(&node), 1);
            assert_eq!(dev.drive(), 5.0);
        }

        #[test]
        fn clip_roundtrip_matches_native_kernel() {
            // Load + instantiate + hydrate from the frozen Node shape,
            // then render sample-identical output to the native kernel.
            for drive in [0.0, 1.0, 10.0] {
                let mut dev = WasmClip::from_bytes(GUEST).unwrap();
                assert_eq!(dev.apply_node(&clip_node("c", drive)), 1);
                assert_eq!(dev.drive(), drive as f32);
                // Hot sine: exercises the rails, not just the linear bit.
                let input: Vec<f32> = (0..1024)
                    .map(|t| {
                        2.0 * (t as f32 * 220.0 * core::f32::consts::TAU / 48_000.0).sin()
                    })
                    .collect();
                let (mut want, mut got) = (vec![0.0; 1024], vec![0.0; 1024]);
                kernel::distortion_process(drive as f32, &input, &mut want).unwrap();
                dev.process(&input, &mut got).unwrap();
                assert_eq!(got, want, "drift at drive={drive}");

                // A mid-stream single set steers the next block.
                dev.set_param(kernel::DRIVE_PARAM, 3.0).unwrap();
                kernel::distortion_process(3.0, &input, &mut want).unwrap();
                dev.process(&input, &mut got).unwrap();
                assert_eq!(got, want, "post-set drift from drive={drive}");

                // Reset is the default drive again.
                dev.reset();
                assert_eq!(dev.drive(), 1.0);
                kernel::distortion_process(1.0, &input, &mut want).unwrap();
                dev.process(&input, &mut got).unwrap();
                assert_eq!(got, want);
            }
            // Drive 0 is unity, like the kernel.
            let mut dev = WasmClip::from_bytes(GUEST).unwrap();
            dev.set_param(kernel::DRIVE_PARAM, 0.0).unwrap();
            let input = vec![0.7, -0.3, 0.0];
            let (mut want, mut got) = (vec![0.0; 3], vec![0.0; 3]);
            kernel::distortion_process(0.0, &input, &mut want).unwrap();
            dev.process(&input, &mut got).unwrap();
            assert_eq!((got, want), (input.clone(), input));
        }
    }
}
