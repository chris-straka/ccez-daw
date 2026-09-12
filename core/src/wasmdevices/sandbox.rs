//! Sandbox: capability-checked renderer around the ported guest kernel.
//!
//! Teaching note: a sandbox is not a speedup — it is a *boundary*.
//! Untrusted DSP must be unable to do three things: read or write memory
//! outside its block, burn unbounded CPU, or corrupt the host with a bad
//! knob. This module enforces all three in layers, cheapest first:
//!
//! 1. **Capabilities** ([`SandboxCaps`]): block length caps, sample-rate
//!    range, and delay-line ceiling, checked on the host before the guest
//!    ever sees a sample. A refused block is [`Error::CapExceeded`], a
//!    refused address is [`Error::BadAddress`]/[`Error::UnknownDevice`]/
//!    [`Error::UnknownParam`] — errors, never traps, so a bad knob cannot
//!    abort a render (the clamp-not-error rule from
//!    [`crate::devices::kernel`], applied one layer up).
//! 2. **Narrow API**: the guest speaks init / set-param / reset /
//!    one-sample-in-one-sample-out. No shared buffers, no pointers, no
//!    strings cross the boundary — params cross as `(code, f32)` pairs
//!    (see [`crate::wasmdevices::guest`]).
//! 3. **Isolation** ([`Backend::Wasmtime`], feature `wasm-runtime`): the
//!    same guest source compiled to `wasm32-wasip2` runs inside a
//!    wasmtime `Store` — separate linear memory, typed-`Func` boundary,
//!    optional fuel metering so an infinite loop becomes a caught trap.
//!    The default [`Backend::Simulated`] runs the identical guest source
//!    in-process through the same narrow API; the render-equivalence
//!    test proves the two paths agree sample-for-sample.
//!
//! Param flow: hosts address this device with the universal grammar —
//! [`WasmDevice::set_param_address`] takes `"dev_dly:feedback"`, checks
//! the device half against its own id (mirroring
//! [`crate::plugins::chain::ChainError::UnknownDevice`]), and applies the
//! param half with guest clamps. Unknown params refuse; bulk loads
//! ([`crate::wasmdevices::params::sync_from_node`]) skip instead, so one
//! grammar serves both interactive tweaks (typos surface) and project
//! loads (future params survive).

use super::guest::GuestDelay;
#[cfg(feature = "wasm-runtime")]
use super::guest::PARAM_OK;
use super::params::{parse_address, sync_from_node};
use crate::model::Node;

/// Errors from the WASM-device sandbox. Every variant is a refused
/// operation with the culprit attached — the guest can never trap the
/// host through these paths.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// `input.len() != output.len()`.
    BadBlock(String),
    /// Address is not `node:param` (missing colon or an empty half).
    BadAddress(String),
    /// The `node` half names a different device than this one.
    UnknownDevice(String),
    /// The `param` half names no guest param (typos must surface).
    UnknownParam(String),
    /// A [`SandboxCaps`] limit refused the operation.
    CapExceeded(String),
    /// The wasmtime backend failed (load, call, or fuel trap).
    Backend(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadBlock(m) => write!(f, "bad wasm-device block: {m}"),
            Self::BadAddress(a) => write!(f, "bad param address `{a}` (want `node:param`)"),
            Self::UnknownDevice(d) => write!(f, "unknown wasm device `{d}`"),
            Self::UnknownParam(p) => write!(f, "unknown wasm-device param `{p}`"),
            Self::CapExceeded(m) => write!(f, "sandbox cap exceeded: {m}"),
            Self::Backend(m) => write!(f, "wasm backend failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Capability limits for one sandboxed device. Checked host-side before
/// every render or set, so the guest (in-process or wasmtime) only ever
/// sees in-bounds work.
#[derive(Debug, Clone)]
pub struct SandboxCaps {
    /// Largest block the guest will render in one `process` call.
    pub max_block_frames: usize,
    /// Lowest construction sample rate (Hz).
    pub min_sample_rate: f32,
    /// Highest construction sample rate (Hz).
    pub max_sample_rate: f32,
    /// Longest delay line the guest may hold (seconds). Matches the
    /// guest's preallocation ([`crate::wasmdevices::guest::MAX_DELAY_S`]).
    pub max_delay_s: f32,
    /// wasmtime fuel per `process` call. `None` = unmetered. Only used by
    /// [`Backend::Wasmtime`]; the simulated path ignores it.
    pub fuel_per_block: Option<u64>,
}

impl Default for SandboxCaps {
    fn default() -> Self {
        Self {
            max_block_frames: 8192,
            min_sample_rate: 1.0,
            max_sample_rate: 192_000.0,
            max_delay_s: super::guest::MAX_DELAY_S,
            fuel_per_block: None,
        }
    }
}

impl SandboxCaps {
    fn check_rate(&self, sample_rate: f32) -> Result<(), Error> {
        if !sample_rate.is_finite()
            || sample_rate < self.min_sample_rate
            || sample_rate > self.max_sample_rate
        {
            return Err(Error::CapExceeded(format!(
                "sample rate {sample_rate} outside [{}, {}]",
                self.min_sample_rate, self.max_sample_rate
            )));
        }
        Ok(())
    }

    fn check_block(&self, input: &[f32], output: &[f32]) -> Result<usize, Error> {
        if input.len() != output.len() {
            return Err(Error::BadBlock(format!(
                "input {} frames != output {} frames",
                input.len(),
                output.len()
            )));
        }
        if input.len() > self.max_block_frames {
            return Err(Error::CapExceeded(format!(
                "block {} frames > max {}",
                input.len(),
                self.max_block_frames
            )));
        }
        Ok(input.len())
    }
}

/// Where the guest math executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// The ported guest source, run in-process through the same narrow
    /// init/param/sample API the WASM boundary uses. Always available;
    /// what the equivalence test renders through.
    Simulated,
    /// The same guest source compiled to WASM (staged artifact: a
    /// zero-import `wasm32-unknown-unknown` core module — see the target
    /// note on the wasmtime backend), run inside a wasmtime `Store`.
    /// Requires feature `wasm-runtime` plus module bytes built by
    /// `scripts/build-wasm-guest.sh`.
    Wasmtime,
}

/// One sandboxed delay device: an id, a capability set, and guest state.
///
/// The `guest` field is the param cache and the simulated renderer. When
/// the wasmtime backend is attached, every set/reset is mirrored into the
/// guest instance and `process` renders there instead — the cache keeps
/// host-side reads (and the simulated fallback) exact.
pub struct WasmDevice {
    id: String,
    guest: GuestDelay,
    caps: SandboxCaps,
    #[cfg(feature = "wasm-runtime")]
    wasmtime: Option<WasmtimeDelay>,
}

impl WasmDevice {
    /// Build a simulated device: params default, line cleared.
    pub fn new_simulated(id: &str, sample_rate: f32) -> Result<Self, Error> {
        Self::new_simulated_with_caps(id, sample_rate, SandboxCaps::default())
    }

    /// Build a simulated device with explicit caps.
    pub fn new_simulated_with_caps(
        id: &str,
        sample_rate: f32,
        caps: SandboxCaps,
    ) -> Result<Self, Error> {
        caps.check_rate(sample_rate)?;
        Ok(Self {
            id: id.to_string(),
            guest: GuestDelay::new(sample_rate),
            caps,
            #[cfg(feature = "wasm-runtime")]
            wasmtime: None,
        })
    }

    /// Build a simulated device hydrated from a frozen [`Node`] (known
    /// params apply, unknown ids skip — the bulk-load tolerance).
    pub fn from_node_simulated(node: &Node, sample_rate: f32) -> Result<Self, Error> {
        let mut dev = Self::new_simulated(&node.id, sample_rate)?;
        sync_from_node(&mut dev.guest, node);
        Ok(dev)
    }

    /// This device's id (the `node` half it answers to).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Which backend renders audio.
    pub fn backend_kind(&self) -> BackendKind {
        #[cfg(feature = "wasm-runtime")]
        if self.wasmtime.is_some() {
            return BackendKind::Wasmtime;
        }
        BackendKind::Simulated
    }

    /// Device sample rate (Hz).
    pub fn sample_rate(&self) -> f32 {
        self.guest.sample_rate
    }

    /// Set one param through the universal address grammar:
    /// `"dev_dly:feedback"`. The device half must match [`WasmDevice::id`]
    /// ([`Error::UnknownDevice`] otherwise); the param half applies with
    /// guest clamps ([`Error::UnknownParam`] on typos — and the stored
    /// value is untouched when a set refuses).
    pub fn set_param_address(&mut self, address: &str, value: f64) -> Result<(), Error> {
        let (node, param) = parse_address(address)?;
        if node != self.id {
            return Err(Error::UnknownDevice(node.to_string()));
        }
        // Validate through the cache first: a refused set must not reach
        // the guest instance in any backend.
        self.guest
            .apply_param(param, value)
            .map_err(|_| Error::UnknownParam(format!("{node}:{param}")))?;
        #[cfg(feature = "wasm-runtime")]
        if let Some(w) = self.wasmtime.as_mut() {
            let code = GuestDelay::code_for_id(param)
                .ok_or_else(|| Error::UnknownParam(format!("{node}:{param}")))?;
            w.set_param(code, value as f32)?;
        }
        Ok(())
    }

    /// Render one block. Lengths must agree and fit [`SandboxCaps`];
    /// empty blocks are a legal no-op (the kernel rule).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), Error> {
        let n = self.caps.check_block(input, output)?;
        if n == 0 {
            return Ok(());
        }
        #[cfg(feature = "wasm-runtime")]
        if let Some(w) = self.wasmtime.as_mut() {
            return w.process(input, output);
        }
        self.guest.process(input, output);
        Ok(())
    }

    /// Clear the delay line (pending echoes vanish), in every backend.
    pub fn reset(&mut self) {
        self.guest.reset();
        #[cfg(feature = "wasm-runtime")]
        if let Some(w) = self.wasmtime.as_mut() {
            w.reset();
        }
    }

    /// No lookahead — same as native, so compensation needs no update.
    pub fn latency_samples(&self) -> u64 {
        0
    }

    // -- test / host probes (plain scalars, no model types) --

    pub fn time_ms(&self) -> f32 {
        self.guest.time_ms
    }
    pub fn feedback(&self) -> f32 {
        self.guest.feedback
    }
    pub fn mix(&self) -> f32 {
        self.guest.mix
    }
    pub fn delay_samples(&self) -> usize {
        self.guest.delay_samples()
    }
}

// -- wasmtime backend (feature `wasm-runtime`) --

/// A ported delay guest running inside wasmtime.
///
/// Boundary design (from the wasmtime embedding docs: `Engine` +
/// `Module` + `Linker` + `Store`, typed `Func`s, no imports):
/// the guest exports `delay_init`, `delay_param`, `delay_reset`, and
/// `delay_sample` and imports *nothing* — no WASI, no host functions —
/// so instantiation needs only an empty `Linker` and the guest cannot
/// reach the host except through the four typed signatures. Samples
/// cross one `f32` per call: the narrowest aperture that still runs the
/// real DSP inside the sandbox (a shared-memory block API is the
/// production follow-up; see the primer).
///
/// Target note: the staged bytes are `wasm32-unknown-unknown`, not
/// `wasm32-wasip2`, on purpose. The same guest source compiles to
/// wasip2 (proven by `scripts/build-wasm-guest.sh`), but there rustc
/// emits a Component Model component whose std pulls WASI imports
/// (clocks/cli/io) — hosting it needs a full WASI context, which widens
/// exactly the aperture this sandbox keeps at zero imports. The
/// component-API wiring is the production follow-up.
#[cfg(feature = "wasm-runtime")]
pub struct WasmtimeDelay {
    store: wasmtime::Store<()>,
    set_param_fn: wasmtime::TypedFunc<(i32, f32), i32>,
    reset_fn: wasmtime::TypedFunc<(), ()>,
    sample_fn: wasmtime::TypedFunc<f32, f32>,
    fuel_per_block: Option<u64>,
}

#[cfg(feature = "wasm-runtime")]
impl WasmtimeDelay {
    /// Instantiate `module_bytes` and init the guest to the given params.
    /// `fuel_per_block` (`Some`) meters execution: an infinite guest loop
    /// becomes a caught trap instead of a hung render.
    pub fn new(
        module_bytes: &[u8],
        sample_rate: f32,
        time_ms: f32,
        feedback: f32,
        mix: f32,
        fuel_per_block: Option<u64>,
    ) -> Result<Self, Error> {
        let mut config = wasmtime::Config::new();
        if fuel_per_block.is_some() {
            config.consume_fuel(true);
        }
        let engine =
            wasmtime::Engine::new(&config).map_err(|e| Error::Backend(e.to_string()))?;
        let module = wasmtime::Module::new(&engine, module_bytes)
            .map_err(|e| Error::Backend(format!("bad module: {e}")))?;
        let mut store = wasmtime::Store::new(&engine, ());
        // Construction runs outside the per-block budget: fund the store
        // generously up front (module init and `delay_init` are bounded),
        // then `process` meters each block. Without this, a metered store
        // traps during instantiation itself — fuel starts at zero when
        // `consume_fuel` is on.
        if fuel_per_block.is_some() {
            store
                .set_fuel(u64::MAX)
                .map_err(|e| Error::Backend(format!("fuel: {e}")))?;
        }
        // The guest imports nothing, so the linker defines nothing.
        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| Error::Backend(format!("instantiate: {e}")))?;
        let init = instance
            .get_typed_func::<(f32, f32, f32, f32), ()>(&mut store, "delay_init")
            .map_err(|e| Error::Backend(format!("missing delay_init: {e}")))?;
        let set_param_fn = instance
            .get_typed_func::<(i32, f32), i32>(&mut store, "delay_param")
            .map_err(|e| Error::Backend(format!("missing delay_param: {e}")))?;
        let reset_fn = instance
            .get_typed_func::<(), ()>(&mut store, "delay_reset")
            .map_err(|e| Error::Backend(format!("missing delay_reset: {e}")))?;
        let sample_fn = instance
            .get_typed_func::<f32, f32>(&mut store, "delay_sample")
            .map_err(|e| Error::Backend(format!("missing delay_sample: {e}")))?;
        init.call(&mut store, (sample_rate, time_ms, feedback, mix))
            .map_err(|e| Error::Backend(format!("delay_init: {e}")))?;
        Ok(Self {
            store,
            set_param_fn,
            reset_fn,
            sample_fn,
            fuel_per_block,
        })
    }

    fn set_param(&mut self, code: i32, value: f32) -> Result<(), Error> {
        let rc = self
            .set_param_fn
            .call(&mut self.store, (code, value))
            .map_err(|e| Error::Backend(format!("delay_param: {e}")))?;
        if rc == PARAM_OK {
            Ok(())
        } else {
            Err(Error::UnknownParam(format!("code {code} refused (rc {rc})")))
        }
    }

    fn reset(&mut self) {
        // Reset cannot fail on the guest side (no params, no alloc); a
        // trap here would mean the instance is already dead, in which
        // case every later call errors anyway.
        let _ = self.reset_fn.call(&mut self.store, ());
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), Error> {
        if let Some(fuel) = self.fuel_per_block {
            self.store
                .set_fuel(fuel)
                .map_err(|e| Error::Backend(format!("fuel: {e}")))?;
        }
        for (o, i) in output.iter_mut().zip(input.iter()) {
            *o = self
                .sample_fn
                .call(&mut self.store, *i)
                .map_err(|e| Error::Backend(format!("delay_sample: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(feature = "wasm-runtime")]
impl WasmDevice {
    /// Build a wasmtime-backed device from compiled guest bytes (see
    /// `scripts/build-wasm-guest.sh`). Params hydrate from `node`;
    /// unknown ids skip, like the simulated path.
    pub fn from_node_wasmtime(
        node: &Node,
        sample_rate: f32,
        module_bytes: &[u8],
        caps: SandboxCaps,
    ) -> Result<Self, Error> {
        caps.check_rate(sample_rate)?;
        let mut guest = GuestDelay::new(sample_rate);
        sync_from_node(&mut guest, node);
        let wasmtime = WasmtimeDelay::new(
            module_bytes,
            guest.sample_rate,
            guest.time_ms,
            guest.feedback,
            guest.mix,
            caps.fuel_per_block,
        )?;
        Ok(Self {
            id: node.id.clone(),
            guest,
            caps,
            wasmtime: Some(wasmtime),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn impulse(frames: usize) -> Vec<f32> {
        let mut v = vec![0.0; frames];
        v[0] = 1.0;
        v
    }

    #[test]
    fn render_equivalence_vs_native_kernel() {
        // The validation gate: the sandboxed port renders sample-identical
        // output to the native `dsp::Delay` across a param grid and two
        // signal shapes. Exact equality (not a tolerance) — the ported
        // loop is op-for-op identical, so any drift is a port bug.
        let sr = 48_000.0;
        for (time_ms, feedback, mix) in [
            (100.0, 0.5, 1.0),
            (375.0, 0.35, 0.3),
            (1.0, 0.0, 0.0),
            (2000.0, 0.95, 0.7),
            (37.5, 0.9, 1.0),
        ] {
            let mut native = crate::dsp::Delay::new(sr);
            native.apply_param(crate::dsp::delay::PARAM_TIME_MS, time_ms).unwrap();
            native.apply_param(crate::dsp::delay::PARAM_FEEDBACK, feedback).unwrap();
            native.apply_param(crate::dsp::delay::PARAM_MIX, mix).unwrap();
            let mut dev = WasmDevice::new_simulated("dev_dly", sr).unwrap();
            let id = dev.id().to_string();
            dev.set_param_address(&format!("{id}:time_ms"), time_ms).unwrap();
            dev.set_param_address(&format!("{id}:feedback"), feedback).unwrap();
            dev.set_param_address(&format!("{id}:mix"), mix).unwrap();

            // Impulse: echoes must land on the same grid with the same decay.
            // Rendered in cap-sized blocks — long delays exceed one block,
            // which also proves echo state survives block boundaries.
            let taps = native.delay_samples();
            assert_eq!(dev.delay_samples(), taps);
            let frames = taps * 3 + 64;
            let input = impulse(frames);
            let (mut want, mut got) = (vec![0.0; frames], vec![0.0; frames]);
            for (w, g, b) in want
                .chunks_mut(4096)
                .zip(got.chunks_mut(4096))
                .zip(input.chunks(4096))
                .map(|((w, g), b)| (w, g, b))
            {
                native.process(b, w);
                dev.process(b, g).unwrap();
            }
            assert_eq!(got, want, "impulse drift at t={time_ms} fb={feedback} mix={mix}");

            // Sine across two blocks: state carries over identically.
            let sine: Vec<f32> = (0..4096)
                .map(|t| (t as f32 * 220.0 * core::f32::consts::TAU / sr).sin())
                .collect();
            for block in sine.chunks(1024) {
                let (mut w, mut g) = (vec![0.0; block.len()], vec![0.0; block.len()]);
                native.process(block, &mut w);
                dev.process(block, &mut g).unwrap();
                assert_eq!(g, w, "sine drift at t={time_ms} fb={feedback} mix={mix}");
            }
        }
    }

    #[test]
    fn addressed_sets_match_native_between_blocks() {
        // A ParamSet-style update mid-stream steers both renders together.
        let sr = 44_100.0;
        let mut native = crate::dsp::Delay::new(sr);
        let mut dev = WasmDevice::new_simulated("dev_dly", sr).unwrap();
        let input = impulse(1024);
        let (mut w, mut g) = (vec![0.0; 1024], vec![0.0; 1024]);
        native.process(&input, &mut w);
        dev.process(&input, &mut g).unwrap();
        assert_eq!(g, w);
        native.apply_param(crate::dsp::delay::PARAM_FEEDBACK, 0.8).unwrap();
        dev.set_param_address("dev_dly:feedback", 0.8).unwrap();
        let silence = vec![0.0; 1024];
        native.process(&silence, &mut w);
        dev.process(&silence, &mut g).unwrap();
        assert_eq!(g, w);
    }

    #[test]
    fn boundary_refuses_loudly_and_leaves_state() {
        let mut dev = WasmDevice::new_simulated("dev_dly", 48_000.0).unwrap();
        let input = impulse(64);
        let mut short = vec![0.0; 63];
        assert!(matches!(
            dev.process(&input, &mut short),
            Err(Error::BadBlock(_))
        ));
        let big_in = vec![0.0; 9000];
        let mut big_out = vec![0.0; 9000];
        assert!(matches!(
            dev.process(&big_in, &mut big_out),
            Err(Error::CapExceeded(_))
        ));
        assert!(matches!(
            dev.set_param_address("no-colon", 0.5),
            Err(Error::BadAddress(_))
        ));
        assert!(matches!(
            dev.set_param_address("other:mix", 0.5),
            Err(Error::UnknownDevice(_))
        ));
        let before = dev.mix();
        assert!(matches!(
            dev.set_param_address("dev_dly:nope", 0.5),
            Err(Error::UnknownParam(_))
        ));
        assert_eq!(dev.mix(), before);
        // Absurd construction rates never reach the guest.
        assert!(matches!(
            WasmDevice::new_simulated("d", f32::INFINITY),
            Err(Error::CapExceeded(_))
        ));
        assert!(matches!(
            WasmDevice::new_simulated("d", 0.0),
            Err(Error::CapExceeded(_))
        ));
        // Empty blocks are a legal no-op; reset clears pending echoes.
        dev.process(&[], &mut []).unwrap();
        assert_eq!(dev.latency_samples(), 0);
        assert_eq!(dev.backend_kind(), BackendKind::Simulated);
        let mut dev = WasmDevice::from_node_simulated(
            &super::super::params::default_node("dev_dly", "D"),
            48_000.0,
        )
        .unwrap();
        assert_eq!((dev.time_ms(), dev.feedback(), dev.mix()), (375.0, 0.35, 0.3));
        dev.set_param_address("dev_dly:mix", 1.0).unwrap();
        dev.process(&impulse(128), &mut vec![0.0; 128]).unwrap();
        dev.reset();
        let mut out = vec![9.9; 8192];
        dev.process(&vec![0.0; 8192], &mut out).unwrap();
        assert!(out.iter().all(|&v| v == 0.0));
    }
}

/// wasmtime-backend tests: need feature `wasm-runtime` plus the compiled
/// guest (`scripts/build-wasm-guest.sh` writes
/// `src/wasmdevices/testdata/delay_guest.wasm`, committed so the gate is
/// hermetic). Not part of the default `cargo test`.
#[cfg(all(test, feature = "wasm-runtime"))]
mod wasmtime_tests {
    use super::super::params::default_node;
    use super::tests::impulse;
    use super::*;

    const GUEST: &[u8] = include_bytes!("testdata/delay_guest.wasm");

    #[test]
    fn real_guest_matches_native_sample_for_sample() {
        let sr = 48_000.0;
        let mut native = crate::dsp::Delay::new(sr);
        native.apply_param(crate::dsp::delay::PARAM_TIME_MS, 100.0).unwrap();
        native.apply_param(crate::dsp::delay::PARAM_FEEDBACK, 0.5).unwrap();
        native.apply_param(crate::dsp::delay::PARAM_MIX, 1.0).unwrap();
        let mut node = default_node("dev_dly", "WASM Delay");
        for (id, v) in [("time_ms", 100.0), ("feedback", 0.5), ("mix", 1.0)] {
            node.params.iter_mut().find(|p| p.id == id).unwrap().value = v;
        }
        let mut dev =
            WasmDevice::from_node_wasmtime(&node, sr, GUEST, SandboxCaps::default()).unwrap();
        assert_eq!(dev.backend_kind(), BackendKind::Wasmtime);
        let taps = native.delay_samples();
        let input = impulse(taps * 3 + 64);
        let (mut want, mut got) = (vec![0.0; input.len()], vec![0.0; input.len()]);
        for ((w, g), b) in want
            .chunks_mut(4096)
            .zip(got.chunks_mut(4096))
            .zip(input.chunks(4096))
        {
            native.process(b, w);
            dev.process(b, g).unwrap();
        }
        assert_eq!(got, want);
        // Addressed set steers the real guest too.
        dev.set_param_address("dev_dly:mix", 0.0).unwrap();
        native.apply_param(crate::dsp::delay::PARAM_MIX, 0.0).unwrap();
        for ((w, g), b) in want
            .chunks_mut(4096)
            .zip(got.chunks_mut(4096))
            .zip(input.chunks(4096))
        {
            native.process(b, w);
            dev.process(b, g).unwrap();
        }
        assert_eq!(got, want);
    }

    #[test]
    fn fuel_metering_turns_a_hang_into_an_error() {
        let node = default_node("dev_dly", "WASM Delay");
        let caps = SandboxCaps {
            fuel_per_block: Some(1),
            ..SandboxCaps::default()
        };
        let mut dev =
            WasmDevice::from_node_wasmtime(&node, 48_000.0, GUEST, caps).unwrap();
        let mut out = vec![0.0; 64];
        assert!(matches!(
            dev.process(&vec![0.0; 64], &mut out),
            Err(Error::Backend(_))
        ));
    }
}
