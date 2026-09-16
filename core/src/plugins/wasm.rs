//! Worker-side WASM device backend: untrusted `.wasm` DSP behind the process boundary.
//!
//! Teaching note: this module answers "where does WASM DSP run?" the
//! same way Track C answered it for CLAP/VST3/AU — *inside the worker
//! process*, never in the host. The [`WasmBackend`] loads one module
//! with `wasmtime` (the evaluated pick, see
//! [`WasmNote`](super::host::WasmNote)) and speaks the existing
//! mono-`f32` kernel contract from [`crate::devices::kernel`]: samples
//! in, samples out, one `f32` per call, with all state in caller-owned
//! bytes. Params cross the existing worker protocol (`SetParam` /
//! `GetState` / `SetState` in [`crate::plugins::worker`]) — no new
//! transport, no new grammar.
//!
//! Guest contract (the whole aperture — the guest imports *nothing*, so
//! the host instantiates it with an empty `Linker`):
//!
//! - `wasm_init(sample_rate: f32)` — construct/reset the instance.
//! - `wasm_set_param(code: i32, value: f32) -> i32` — scalar set; `0`
//!   ok, `1` unknown id. Never traps (a bad knob must not abort a
//!   render — the clamp-not-error rule from
//!   [`crate::devices::kernel`], applied at the boundary).
//! - `wasm_sample(x: f32) -> f32` — one mono sample through the DSP.
//! - `wasm_reset()` — clear delay lines / phases. Never traps.
//!
//! Params cross as `(code, f32)` pairs ([`WASM_GAIN_CODE`], append-only
//! like the device-class codes); strings never enter the guest. State
//! the host must resurrect (params plus the plugin's opaque blob) lives
//! in [`PluginState`](super::host::PluginState) owned by the worker —
//! caller-owned bytes, so `recover()` replays `Init` + `LoadWasm` +
//! `SetState` with zero guest cooperation.
//!
//! Requires feature `wasm-runtime` (same optional `wasmtime` pin the
//! devices-side [`crate::wasmdevices`] sandbox uses): default builds
//! download and compile nothing new. Without the feature the
//! [`crate::plugins::worker`] `LoadWasm` op fails cleanly and loading a
//! [`PluginKind::Wasm`](super::host::PluginKind) descriptor refuses with
//! a protocol error — never a half-loaded entry.
//!
//! The test guest is [`GAIN_GUEST_WAT`]: a gain device (unity default,
//! the null-device with a knob, like [`MockDsp`](super::worker::MockDsp))
//! as inline WAT. `wasmtime` parses module text hermetically (the `wat`
//! cargo feature is on by default), so the roundtrip tests stage it to
//! a temp file and load it through the real file path — no committed
//! bytes, no network, no new crate.

#[cfg(feature = "wasm-runtime")]
use std::collections::BTreeMap;

#[cfg(feature = "wasm-runtime")]
use super::host::PluginState;

/// `gain` param id (linear, default 1.0). Mirrors the mock backend's one
/// knob, so the worker-process roundtrip asserts the same math.
pub const WASM_GAIN_PARAM: &str = "gain";

/// Default gain: unity, like the mock backend.
pub const WASM_DEFAULT_GAIN: f64 = 1.0;

/// Numeric param code for `gain` on the WASM boundary. Append-only.
pub const WASM_GAIN_CODE: i32 = 0;

/// Success return code for `wasm_set_param`. Mirrors the delay guest's
/// `PARAM_OK` convention (zero is success, nonzero is a refused set).
pub const WASM_PARAM_OK: i32 = 0;
/// Unknown-id return code for `wasm_set_param`.
pub const WASM_PARAM_UNKNOWN_ID: i32 = 1;

/// Test-guest device: a gain stage as WAT (WebAssembly text).
///
/// Zero imports, four exports (`wasm_init`, `wasm_set_param`,
/// `wasm_sample`, `wasm_reset`), one mutable global for the gain. Tests
/// stage this to a temp file and `LoadWasm` it through the sandboxed
/// worker, proving load → process → param roundtrips through the real
/// pipe. Production loads real `.wasm` files: `Module::new` accepts
/// both binary and text encodings.
pub const GAIN_GUEST_WAT: &str = r#"(module
  (global $gain (mut f32) (f32.const 1))
  (func (export "wasm_init") (param f32)
    (global.set $gain (f32.const 1)))
  (func (export "wasm_set_param") (param i32 f32) (result i32)
    (if (result i32) (i32.eq (local.get 0) (i32.const 0))
      (then (global.set $gain (local.get 1)) (i32.const 0))
      (else (i32.const 1))))
  (func (export "wasm_sample") (param f32) (result f32)
    (f32.mul (local.get 0) (global.get $gain)))
  (func (export "wasm_reset"))
)"#;

/// Errors from the worker-side WASM backend. Loading and param refusals
/// are errors with the culprit attached — the guest can never trap the
/// worker through these paths.
#[derive(Debug, Clone, PartialEq)]
pub enum WasmError {
    /// The file could not be read.
    Io(String),
    /// The bytes failed validation or instantiation.
    BadModule(String),
    /// A required export (`wasm_init`, `wasm_set_param`, `wasm_sample`)
    /// is missing or mistyped.
    MissingExport(String),
    /// A guest call trapped.
    Call(String),
    /// The param id names no guest param (typos must surface).
    UnknownParam(String),
}

impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "wasm io: {m}"),
            Self::BadModule(m) => write!(f, "bad wasm module: {m}"),
            Self::MissingExport(m) => write!(f, "wasm guest missing export: {m}"),
            Self::Call(m) => write!(f, "wasm guest call failed: {m}"),
            Self::UnknownParam(p) => write!(f, "unknown wasm param `{p}`"),
        }
    }
}

impl std::error::Error for WasmError {}

/// One WASM device instance inside the worker process: the wasmtime
/// guest plus the caller-owned state bytes (params the worker validates
/// before they reach the guest, and the opaque blob it never reads).
#[cfg(feature = "wasm-runtime")]
pub struct WasmBackend {
    store: wasmtime::Store<()>,
    set_param_fn: wasmtime::TypedFunc<(i32, f32), i32>,
    sample_fn: wasmtime::TypedFunc<f32, f32>,
    sample_rate: f64,
    params: BTreeMap<String, f64>,
    blob: Vec<u8>,
}

#[cfg(feature = "wasm-runtime")]
impl WasmBackend {
    /// Instantiate `module` (binary or WAT text) and init the guest at
    /// `sample_rate` with default params.
    pub fn load_bytes(module: &[u8], sample_rate: f64) -> Result<Self, WasmError> {
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, module)
            .map_err(|e| WasmError::BadModule(e.to_string()))?;
        let mut store = wasmtime::Store::new(&engine, ());
        // The guest imports nothing, so the linker defines nothing: the
        // four signatures in the module docs are the whole aperture.
        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| WasmError::BadModule(format!("instantiate: {e}")))?;
        let init = instance
            .get_typed_func::<f32, ()>(&mut store, "wasm_init")
            .map_err(|e| WasmError::MissingExport(format!("wasm_init: {e}")))?;
        let set_param_fn = instance
            .get_typed_func::<(i32, f32), i32>(&mut store, "wasm_set_param")
            .map_err(|e| WasmError::MissingExport(format!("wasm_set_param: {e}")))?;
        let sample_fn = instance
            .get_typed_func::<f32, f32>(&mut store, "wasm_sample")
            .map_err(|e| WasmError::MissingExport(format!("wasm_sample: {e}")))?;
        init.call(&mut store, sample_rate as f32)
            .map_err(|e| WasmError::Call(format!("wasm_init: {e}")))?;
        Ok(Self {
            store,
            set_param_fn,
            sample_fn,
            sample_rate,
            params: BTreeMap::from([(WASM_GAIN_PARAM.to_string(), WASM_DEFAULT_GAIN)]),
            blob: Vec::new(),
        })
    }

    /// Load a `.wasm` module from disk (binary or text). A missing or
    /// invalid file is an error, never a half-built backend.
    pub fn load(path: &str, sample_rate: f64) -> Result<Self, WasmError> {
        let bytes = std::fs::read(path).map_err(|e| WasmError::Io(format!("read {path}: {e}")))?;
        Self::load_bytes(&bytes, sample_rate)
    }

    /// Rate from the `Init` handshake. Unity-gain DSP ignores it; real
    /// device DSP (filters, delays) is built from it.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Set one param by id. Unknown ids refuse *before* touching the
    /// guest, so a refused set leaves stored state untouched.
    pub fn set_param(&mut self, id: &str, value: f64) -> Result<(), WasmError> {
        let code = match id {
            WASM_GAIN_PARAM => WASM_GAIN_CODE,
            _ => return Err(WasmError::UnknownParam(id.to_string())),
        };
        let rc = self
            .set_param_fn
            .call(&mut self.store, (code, value as f32))
            .map_err(|e| WasmError::Call(format!("wasm_set_param: {e}")))?;
        if rc == WASM_PARAM_OK {
            self.params.insert(id.to_string(), value);
            Ok(())
        } else {
            Err(WasmError::UnknownParam(format!("{id} refused (rc {rc})")))
        }
    }

    /// Processing latency in samples (memory-free gain is honestly zero;
    /// real backends query the module).
    pub fn latency_samples(&self) -> u32 {
        0
    }

    pub fn state(&self) -> PluginState {
        PluginState::new(self.params.clone(), self.blob.clone())
    }

    /// Adopt `state` wholesale (params the worker never interprets ride
    /// along byte-exact) and push the gain into the guest. A trapped
    /// push keeps the old state — adoption only lands on success.
    pub fn set_state(&mut self, state: &PluginState) -> Result<(), WasmError> {
        let gain = state
            .params
            .get(WASM_GAIN_PARAM)
            .copied()
            .unwrap_or(WASM_DEFAULT_GAIN);
        let rc = self
            .set_param_fn
            .call(&mut self.store, (WASM_GAIN_CODE, gain as f32))
            .map_err(|e| WasmError::Call(format!("wasm_set_param: {e}")))?;
        if rc != WASM_PARAM_OK {
            return Err(WasmError::Call(format!("guest refused gain (rc {rc})")));
        }
        self.params = state.params.clone();
        self.blob = state.blob.clone();
        Ok(())
    }

    /// Render one mono block: one `f32` per guest call, the narrowest
    /// aperture that still runs real DSP inside the sandbox (a
    /// shared-memory block API is the production follow-up, same as the
    /// devices-side sandbox).
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, WasmError> {
        let mut output = Vec::with_capacity(input.len());
        for sample in input {
            output.push(
                self.sample_fn
                    .call(&mut self.store, *sample)
                    .map_err(|e| WasmError::Call(format!("wasm_sample: {e}")))?,
            );
        }
        Ok(output)
    }
}

/// Stage the test-guest device to a temp file and return its path.
/// Tests `LoadWasm` this path, so the file read is exercised exactly as
/// production exercises it.
#[cfg(all(test, feature = "wasm-runtime"))]
pub(crate) fn write_gain_guest_file() -> std::path::PathBuf {
    // Unique per call: parallel tests stage (and their workers read)
    // concurrently, and sharing one path would interleave writes with a
    // worker's read of a half-written file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "ccez-gain-guest-{}-{n}.wat",
        std::process::id()
    ));
    std::fs::write(&path, GAIN_GUEST_WAT).expect("stage gain guest");
    path
}

#[cfg(all(test, feature = "wasm-runtime"))]
mod tests {
    use super::*;

    #[test]
    fn gain_guest_parses_as_zero_import_module() {
        // The aperture proof at the text level: four exports, no imports.
        assert!(GAIN_GUEST_WAT.contains("(export \"wasm_init\")"));
        assert!(GAIN_GUEST_WAT.contains("(export \"wasm_set_param\")"));
        assert!(GAIN_GUEST_WAT.contains("(export \"wasm_sample\")"));
        assert!(GAIN_GUEST_WAT.contains("(export \"wasm_reset\")"));
        assert!(!GAIN_GUEST_WAT.contains("(import"));
    }

    #[test]
    fn load_process_param_round_trip() {
        let mut dsp =
            WasmBackend::load_bytes(GAIN_GUEST_WAT.as_bytes(), 48_000.0).expect("guest loads");
        assert_eq!(dsp.sample_rate(), 48_000.0);
        // Unity default, like the mock backend.
        assert_eq!(dsp.process(&[1.0, -2.0]).expect("process"), vec![1.0, -2.0]);
        dsp.set_param(WASM_GAIN_PARAM, 0.5).expect("set gain");
        assert_eq!(dsp.process(&[1.0, -2.0]).expect("process"), vec![0.5, -1.0]);
        assert_eq!(dsp.latency_samples(), 0);
    }

    #[test]
    fn unknown_param_refuses_with_state_untouched() {
        let mut dsp =
            WasmBackend::load_bytes(GAIN_GUEST_WAT.as_bytes(), 44_100.0).expect("guest loads");
        dsp.set_param(WASM_GAIN_PARAM, 0.5).expect("set gain");
        let err = dsp.set_param("nope", 9.0).expect_err("typo must surface");
        assert!(matches!(err, WasmError::UnknownParam(_)), "got {err:?}");
        // The refused set never reached the guest.
        assert_eq!(dsp.process(&[1.0]).expect("process"), vec![0.5]);
        assert_eq!(dsp.state().params.get(WASM_GAIN_PARAM), Some(&0.5));
    }

    #[test]
    fn state_blob_round_trips_byte_exact() {
        let mut dsp =
            WasmBackend::load_bytes(GAIN_GUEST_WAT.as_bytes(), 44_100.0).expect("guest loads");
        dsp.set_param(WASM_GAIN_PARAM, 0.25).expect("set gain");
        let mut st = dsp.state();
        st.blob = vec![7, 7, 7];
        dsp.set_param(WASM_GAIN_PARAM, 0.0).expect("retune");
        dsp.set_state(&st).expect("restore");
        assert_eq!(dsp.process(&[1.0]).expect("renders"), vec![0.25]);
        assert_eq!(dsp.state(), st);
    }

    #[test]
    fn bad_bytes_and_missing_exports_refuse() {
        assert!(matches!(
            WasmBackend::load_bytes(b"not a module", 44_100.0),
            Err(WasmError::BadModule(_))
        ));
        let no_exports = "(module)";
        assert!(matches!(
            WasmBackend::load_bytes(no_exports.as_bytes(), 44_100.0),
            Err(WasmError::MissingExport(_))
        ));
        assert!(matches!(
            WasmBackend::load("/does/not/exist.wasm", 44_100.0),
            Err(WasmError::Io(_))
        ));
    }
}
