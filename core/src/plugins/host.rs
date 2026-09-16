//! Plugin registry: descriptors, state snapshots, and the host.
//!
//! Teaching note: a DAW never trusts plugin code. A `.clap` binary is an
//! untrusted shared library — a null dereference in it must kill *the
//! insert*, never the session. So the host keeps two things per plugin:
//! a [`SandboxedPlugin`] (the live child process doing DSP) and a
//! [`PluginState`] (the last known-good params + opaque blob). Audio goes
//! to the child; truth stays in the host. When the child dies, `recover()`
//! respawns it and pushes the snapshot back in.
//!
//! [`PluginState`] is JSON-serializable on purpose: the same bytes ride in
//! engine asset files (`ASSET_KIND_PLUGIN`) and portable bundles, so a
//! frozen plugin stem and a live insert restore through one shape.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::au::AuComponentDesc;
use super::sandbox::{SandboxError, SandboxedPlugin};

/// What backs one plugin instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginKind {
    /// Built-in mock gain plugin (runs in [`crate::plugins::worker`]).
    /// What tests and CI exercise — no `.clap` binary needed.
    Mock,
    /// A real CLAP bundle at `descriptor.path`. Loading runs through the
    /// same worker process via `clack-host`; see [`ClapNote`].
    Clap,
    /// A real VST3 bundle at `descriptor.path`. Loading runs through the
    /// same worker process via `vst3-host`; see [`Vst3Note`].
    Vst3,
    /// A real Audio Unit named by `descriptor.au_desc`. Loading runs
    /// through the same worker process via
    /// [`AuBackend`](crate::plugins::au::AuBackend) (mono v1 scope,
    /// first match in the registry, bridged params, live latency) —
    /// macOS only, no `CCEZ_ENABLE_AU` consult: the tested render path
    /// is its own opt-in. See [`AuNote`].
    Au,
    /// A `.wasm` device module at `descriptor.path`. Loading runs
    /// through the same worker process via
    /// [`WasmBackend`](crate::plugins::wasm::WasmBackend) (mono `f32`
    /// at the `Init` rate, params as `SetParam`, state as
    /// `GetState`/`SetState`). Needs feature `wasm-runtime` — without
    /// it the load fails cleanly. See [`WasmNote`].
    Wasm,
}

/// Why [`PluginKind::Au`] loads through [`crate::plugins::au`].
///
/// Evaluation: hand-rolled FFI against the always-present `AudioToolbox`
/// (+ `CoreFoundation` for display names), zero new deps — the v1
/// find-then-open seam grew parameter bridging (global-scope
/// list/get/set, clamped, names case-insensitive) and render bridging
/// (`AudioUnitRender`; effects pull mono input through an input
/// callback, sources render without one). Untested component types
/// (output/mixer/panner/converter/…) refuse with `UnsupportedType`
/// before anything loads. Full record: `docs/notes/track-c.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuNote;

/// Why [`PluginKind::Clap`] loads through [`crate::plugins::clap`].
///
/// Evaluation (via repo docs, Sept 2026):
///
/// - `robbert-vdh/nih-plug` (the NIH-plug framework) is in **maintenance
///   mode**; the recommended community fork is `BillyDM/nih-plug`
///   (Codeberg) — but nih-plug is a plugin-*authoring* framework (the
///   `nih_export_clap!()` macro writes plugins), not a host library.
/// - Hosting CLAP in Rust is `prokopyl/clack` (`clack-host`:
///   `PluginEntry::load` + `PluginInstance::new` + `HostHandlers`;
///   feature-complete, still evolving).
///
/// The sandbox + snapshot + recovery machinery was first proved with the
/// mock backend (zero new deps at the time); real loading now runs
/// *inside the worker process* via [`crate::plugins::clap::ClapBackend`]
/// (clack-host + libloading + clack-extensions) — untrusted C-ABI code
/// stays behind the process boundary — speaking the existing
/// [`crate::plugins::worker`] protocol, so the host and recovery paths
/// below did not change shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClapNote;

/// Why [`PluginKind::Vst3`] loads through [`crate::plugins::vst3`].
///
/// Evaluation (via crates.io, Sept 2026):
///
/// - `vst3-host 0.9` (HelgeSverre/rust-vst3-host, MIT) is a real VST3 host:
///   `Vst3Host::builder` + `load_plugin` + `set_parameter` + `process_audio`
///   over the genuine Steinberg COM ABI (it vendors its own `vst3`
///   bindings, no Steinberg SDK needed). Chosen, with
///   `default-features = false` (no cpal backend, no helper binaries —
///   isolation stays in our worker process, audio stays offline).
/// - `sinkingsugar/rack 0.4.8` also hosts VST3, but drags a native
///   C++/ObjC++ build layer and an unstable API — too heavy for v1.
/// - Hand-rolled COM FFI stays where it already works: factory enumeration
///   for discovery (`Vst3Host::load_module`). Driving audio by hand would
///   re-implement `setupProcessing`/`ProcessData` against every vendor's
///   quirks; the crate owns that now.
///
/// Real loading runs *inside the worker process* via
/// [`crate::plugins::vst3::Vst3Backend`] (mono v1 scope, first
/// audio-effect class, normalized params, live latency, real state chunks)
/// speaking the existing [`crate::plugins::worker`] protocol (`Init`, then
/// one additive `LoadVst3` round-trip), so host and recovery code did not
/// change shape. Full record: `docs/notes/track-c.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vst3Note;

/// Why [`PluginKind::Wasm`] loads through [`crate::plugins::wasm`].
///
/// Evaluation (inspected, Sept 2026 — verdict: `wasmtime` 48.0.2):
///
/// - `wasmtime` 48.0.2 is already the pinned devices-side sandbox
///   runtime ([`crate::wasmdevices`]): typed-`Func` boundary, `Store`
///   isolation, empty-`Linker` instantiation for zero-import guests, and
///   an optional dependency so default builds download and compile
///   nothing new. Reusing the pin adds zero new dependency surface.
/// - `wasm3`/`wasmi` are lighter (faster startup, smaller binary) but
///   would each add a new dependency for an unproven embedding — startup
///   time does not dominate a device load, so the proven runtime wins.
/// - The guest contract is four exports and zero imports (`wasm_init` /
///   `wasm_set_param` / `wasm_sample` / `wasm_reset`), mono `f32`
///   per-sample — the [`crate::devices::kernel`] shape, so the seam is a
///   loader plus a memory copy, never a redesign. A shared-memory block
///   API is the production follow-up, not a second device kind.
///
/// Loading runs *inside the worker process* via
/// [`crate::plugins::wasm::WasmBackend`] speaking the existing
/// [`crate::plugins::worker`] protocol (`Init`, then one additive
/// `LoadWasm` round-trip), so host and recovery code did not change
/// shape. Full record: `docs/notes/track-c.md` §11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmNote;

/// One loadable plugin: identity plus where its DSP comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// Stable id (matches the device [`Node`](crate::model::Node) id when
    /// the plugin backs a chain insert).
    pub id: String,
    pub name: String,
    pub kind: PluginKind,
    /// Filesystem path to the `.clap` / `*.vst3` / `.wasm` module.
    /// Required for [`PluginKind::Clap`] / [`PluginKind::Vst3`] /
    /// [`PluginKind::Wasm`], ignored otherwise.
    pub path: Option<String>,
    /// FourCC identity for [`PluginKind::Au`]. Required for `Au`,
    /// ignored for every other kind (mirrors `path` for Clap/Vst3).
    /// Skipped in JSON when absent so pre-AU snapshots read back
    /// byte-identical.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub au_desc: Option<AuComponentDesc>,
    /// Bundle-internal selection for multi-plugin bundles: the CLAP
    /// plugin id for [`PluginKind::Clap`] or the VST3 class uid for
    /// [`PluginKind::Vst3`]. `None` keeps the old default (first
    /// plugin / first audio-effect class). Ignored for every other
    /// kind. Skipped in JSON when absent so pre-selection snapshots
    /// read back byte-identical.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plugin_uid: Option<String>,
}

impl PluginDescriptor {
    pub fn mock(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Mock,
            path: None,
            au_desc: None,
            plugin_uid: None,
        }
    }

    pub fn clap(id: &str, name: &str, path: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Clap,
            path: Some(path.to_string()),
            au_desc: None,
            plugin_uid: None,
        }
    }

    /// A CLAP bundle with a non-first plugin selected (one of
    /// [`ClapBackend::available_plugins`](crate::plugins::clap::ClapBackend::available_plugins)).
    pub fn clap_with_plugin(id: &str, name: &str, path: &str, plugin_id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Clap,
            path: Some(path.to_string()),
            au_desc: None,
            plugin_uid: Some(plugin_id.to_string()),
        }
    }

    pub fn vst3(id: &str, name: &str, path: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Vst3,
            path: Some(path.to_string()),
            au_desc: None,
            plugin_uid: None,
        }
    }

    /// A VST3 bundle with a non-first audio-effect class selected (one of
    /// [`Vst3Backend::available_classes`](crate::plugins::vst3::Vst3Backend::available_classes)).
    pub fn vst3_with_class(id: &str, name: &str, path: &str, class_id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Vst3,
            path: Some(path.to_string()),
            au_desc: None,
            plugin_uid: Some(class_id.to_string()),
        }
    }

    pub fn au(id: &str, name: &str, desc: AuComponentDesc) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Au,
            path: None,
            au_desc: Some(desc),
            plugin_uid: None,
        }
    }

    /// A `.wasm` device module at `path`, loaded inside the sandboxed
    /// worker via [`crate::plugins::wasm::WasmBackend`].
    pub fn wasm(id: &str, name: &str, path: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Wasm,
            path: Some(path.to_string()),
            au_desc: None,
            plugin_uid: None,
        }
    }
}

/// Everything needed to resurrect a plugin exactly: its params plus the
/// plugin's own opaque blob (CLAP state chunk / preset bytes). The host
/// treats `blob` as bytes it never interprets — round-trip exactness is
/// the only contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginState {
    pub params: BTreeMap<String, f64>,
    pub blob: Vec<u8>,
}

impl PluginState {
    pub fn new(params: BTreeMap<String, f64>, blob: Vec<u8>) -> Self {
        Self { params, blob }
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| HostError::BadState(e.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| HostError::BadState(e.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    UnknownPlugin(String),
    DuplicatePlugin(String),
    Sandbox(SandboxError),
    BadState(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPlugin(id) => write!(f, "unknown plugin `{id}`"),
            Self::DuplicatePlugin(id) => write!(f, "plugin `{id}` already loaded"),
            Self::Sandbox(e) => write!(f, "plugin sandbox: {e}"),
            Self::BadState(m) => write!(f, "bad plugin state: {m}"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<SandboxError> for HostError {
    fn from(e: SandboxError) -> Self {
        Self::Sandbox(e)
    }
}

pub type Result<T> = std::result::Result<T, HostError>;

struct Entry {
    descriptor: PluginDescriptor,
    plugin: SandboxedPlugin,
}

/// The plugin host: one registry, one worker process per plugin.
///
/// Session-survival rule: every method that touches a dead plugin returns
/// an error *for that plugin only*. The registry itself never poisons —
/// other plugins keep rendering, and [`PluginHost::recover`] brings the
/// dead one back from its snapshot.
pub struct PluginHost {
    sample_rate: f64,
    worker_bin: PathBuf,
    entries: HashMap<String, Entry>,
}

impl PluginHost {
    pub fn new(sample_rate: f64, worker_bin: PathBuf) -> Self {
        Self {
            sample_rate,
            worker_bin,
            entries: HashMap::new(),
        }
    }

    /// Locate the `plugin-worker` binary next to the current executable
    /// (unit tests run from `target/{debug,release}/deps/`, the worker
    /// sits one level up). Falls back to `debug/` when that fails.
    pub fn default_worker_bin() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(deps) = exe.parent() {
                let bin = if deps.file_name().is_some_and(|n| n == "deps") {
                    deps.parent()
                        .unwrap_or(deps)
                        .join(super::worker::WORKER_BIN_NAME)
                } else {
                    deps.join(super::worker::WORKER_BIN_NAME)
                };
                #[cfg(windows)]
                let bin = bin.with_extension("exe");
                if bin.exists() {
                    return bin;
                }
            }
        }
        PathBuf::from(format!("target/debug/{}", super::worker::WORKER_BIN_NAME))
    }

    /// Load a plugin: spawn its sandbox worker and take an initial
    /// snapshot (the baseline `recover()` restores to). Mock, Clap, Vst3,
    /// Au, and Wasm descriptors share this path — a real bundle/unit /
    /// module loads inside the worker via `LoadClap` / `LoadVst3` /
    /// `LoadAu` / `LoadWasm`, so a bad bundle is a `Sandbox` error here,
    /// never a half-loaded entry.
    pub fn load(&mut self, descriptor: PluginDescriptor) -> Result<()> {
        if self.entries.contains_key(&descriptor.id) {
            return Err(HostError::DuplicatePlugin(descriptor.id.clone()));
        }
        let mut plugin =
            SandboxedPlugin::spawn(&self.worker_bin, &descriptor, self.sample_rate)?;
        // Baseline snapshot: even a never-touched plugin restores exactly.
        let state = plugin.snapshot()?;
        self.entries.insert(
            descriptor.id.clone(),
            Entry { descriptor, plugin },
        );
        let _ = state;
        Ok(())
    }

    pub fn unload(&mut self, id: &str) -> Result<PluginDescriptor> {
        self.entries
            .remove(id)
            .map(|e| e.descriptor)
            .ok_or_else(|| HostError::UnknownPlugin(id.to_string()))
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.entries.keys().cloned().collect();
        ids.sort();
        ids
    }

    fn entry_mut(&mut self, id: &str) -> Result<&mut Entry> {
        self.entries
            .get_mut(id)
            .ok_or_else(|| HostError::UnknownPlugin(id.to_string()))
    }

    /// Render `input` through the plugin (mono block). A crashed plugin
    /// returns [`SandboxError::Crashed`] — the session is untouched; call
    /// [`PluginHost::recover`] to bring it back.
    pub fn process(&mut self, id: &str, input: &[f32]) -> Result<Vec<f32>> {
        Ok(self.entry_mut(id)?.plugin.process(input)?)
    }

    pub fn set_param(&mut self, id: &str, param: &str, value: f64) -> Result<()> {
        Ok(self.entry_mut(id)?.plugin.set_param(param, value)?)
    }

    /// Current state of the plugin (also refreshes the host-side
    /// last-known-good copy used by `recover()`).
    pub fn snapshot(&mut self, id: &str) -> Result<PluginState> {
        Ok(self.entry_mut(id)?.plugin.snapshot()?)
    }

    /// Processing latency of the plugin in samples at the current rate.
    /// Sample the whole graph into a [`LatencyMap`](super::latency::LatencyMap)
    /// with one loop over [`PluginHost::ids`]; unknown ids stay an error
    /// (a latency table must never silently skip a loaded insert).
    pub fn latency_samples(&mut self, id: &str) -> Result<u32> {
        Ok(self.entry_mut(id)?.plugin.latency_samples()?)
    }

    pub fn restore(&mut self, id: &str, state: &PluginState) -> Result<()> {
        Ok(self.entry_mut(id)?.plugin.restore(state)?)
    }

    /// `true` when the plugin's worker process is up.
    pub fn alive(&mut self, id: &str) -> Result<bool> {
        Ok(self.entry_mut(id)?.plugin.alive())
    }

    /// Heartbeat: a no-op round-trip proving the worker is answering, not
    /// just unreaped. A hung worker reports `Crashed` here (then
    /// [`PluginHost::recover`] brings it back); use it as the watchdog
    /// poll between renders.
    pub fn ping(&mut self, id: &str) -> Result<()> {
        Ok(self.entry_mut(id)?.plugin.ping()?)
    }

    /// Watchdog deadline for one worker round-trip (see
    /// [`SandboxedPlugin::set_timeout`](super::sandbox::SandboxedPlugin::set_timeout)).
    pub fn set_timeout(&mut self, id: &str, timeout: std::time::Duration) -> Result<()> {
        self.entry_mut(id)?.plugin.set_timeout(timeout);
        Ok(())
    }

    /// Bring a crashed plugin back: respawn the worker and push its last
    /// known-good snapshot. Returns the restored state. A live plugin
    /// just re-snapshots (idempotent, never destructive).
    pub fn recover(&mut self, id: &str) -> Result<PluginState> {
        Ok(self.entry_mut(id)?.plugin.recover()?)
    }

    /// Test/crash hook: kill the worker the way a segfault would — no
    /// handshake, no cleanup. The entry (descriptor + snapshot) survives.
    pub fn kill(&mut self, id: &str) -> Result<()> {
        Ok(self.entry_mut(id)?.plugin.kill()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::worker::ensure_worker_built;

    fn host() -> PluginHost {
        ensure_worker_built();
        PluginHost::new(44100.0, PluginHost::default_worker_bin())
    }

    #[test]
    fn load_process_unload_mock() {
        let mut host = host();
        host.load(PluginDescriptor::mock("gain1", "Gain")).expect("load");
        assert_eq!(host.ids(), vec!["gain1".to_string()]);
        host.set_param("gain1", "gain", 0.5).expect("set_param");
        let out = host.process("gain1", &[1.0, 0.5, -1.0]).expect("process");
        assert_eq!(out, vec![0.5, 0.25, -0.5]);
        let desc = host.unload("gain1").expect("unload");
        assert_eq!(desc.name, "Gain");
        assert!(host.ids().is_empty());
    }

    #[test]
    fn duplicate_and_unknown_ids_are_clean_errors() {
        let mut host = host();
        host.load(PluginDescriptor::mock("g", "Gain")).expect("load");
        assert!(matches!(
            host.load(PluginDescriptor::mock("g", "Gain")),
            Err(HostError::DuplicatePlugin(_))
        ));
        assert!(matches!(
            host.process("ghost", &[1.0]),
            Err(HostError::UnknownPlugin(_))
        ));
        assert!(matches!(
            host.unload("ghost"),
            Err(HostError::UnknownPlugin(_))
        ));
    }

    #[test]
    fn clap_bundle_loads_processes_and_reports_latency() {
        // The real-CLAP roundtrip: a genuine `.clap` bundle (the
        // `ccez-clap-gain` cdylib fixture) loads inside the sandboxed
        // worker, audio flows through its DSP, and its declared latency
        // lands in the compensation table.
        use crate::plugins::clap::{fixture::ensure_fixture_built, fixture_latency};
        use crate::plugins::latency::LatencyMap;
        let mut host = host();
        let bundle = ensure_fixture_built();
        host.load(PluginDescriptor::clap(
            "gain",
            "Ccez Gain",
            &bundle.display().to_string(),
        ))
        .expect("load real bundle");
        host.set_param("gain", "gain", 0.5).expect("set_param");
        let out = host.process("gain", &[1.0, 0.5, -1.0]).expect("process");
        assert_eq!(out, vec![0.5, 0.25, -0.5]);
        // Latency reporting feeds the delay-compensation map.
        let mut map = LatencyMap::new();
        let lat = host.latency_samples("gain").expect("latency");
        assert_eq!(lat, fixture_latency());
        map.report("gain", lat).expect("report");
        assert_eq!(
            map.chain_latency(&["gain".to_string()]),
            u64::from(fixture_latency())
        );
        // Snapshot/restore round-trips across the real backend.
        let state = host.snapshot("gain").expect("snapshot");
        host.set_param("gain", "gain", 2.0).expect("retune");
        assert_eq!(host.process("gain", &[1.0]).expect("renders"), vec![2.0]);
        host.restore("gain", &state).expect("restore");
        assert_eq!(host.process("gain", &[1.0]).expect("renders"), vec![0.5]);
        assert_eq!(host.snapshot("gain").expect("state"), state);
    }

    #[test]
    fn clap_crash_recovers_with_exact_state() {
        // Same crash contract as the mock: kill the worker mid-session,
        // prove the session survives, recover the exact pre-crash state —
        // except respawn re-loads the real bundle behind the same path.
        use crate::plugins::clap::fixture::ensure_fixture_built;
        let mut host = host();
        let bundle = ensure_fixture_built();
        host.load(PluginDescriptor::clap(
            "doomed",
            "Doom",
            &bundle.display().to_string(),
        ))
        .expect("load");
        host.set_param("doomed", "gain", 0.25).expect("gain");
        let before = host.snapshot("doomed").expect("snapshot");

        host.kill("doomed").expect("kill");
        assert!(!host.alive("doomed").expect("alive check"));
        let err = host.process("doomed", &[1.0]).expect_err("must fail");
        assert!(matches!(
            err,
            HostError::Sandbox(SandboxError::Crashed(_))
        ));

        let restored = host.recover("doomed").expect("recover");
        assert_eq!(restored, before);
        assert_eq!(host.process("doomed", &[1.0]).expect("renders"), vec![0.25]);
    }

    #[test]
    fn clap_missing_bundle_is_a_clean_error() {
        let mut host = host();
        let err = host
            .load(PluginDescriptor::clap(
                "ghost",
                "Ghost",
                "/nonexistent/ghost.clap",
            ))
            .expect_err("missing bundle must fail");
        assert!(matches!(err, HostError::Sandbox(_)), "got {err:?}");
        // The failure leaves no half-loaded entry behind.
        assert!(host.ids().is_empty());
    }

    #[test]
    fn clap_unknown_param_is_a_clean_error() {
        use crate::plugins::clap::fixture::ensure_fixture_built;
        let mut host = host();
        let bundle = ensure_fixture_built();
        host.load(PluginDescriptor::clap(
            "gain",
            "Ccez Gain",
            &bundle.display().to_string(),
        ))
        .expect("load");
        let err = host
            .set_param("gain", "cutoff", 1.0)
            .expect_err("unknown param must fail");
        assert!(
            matches!(err, HostError::Sandbox(SandboxError::Protocol(_))),
            "got {err:?}"
        );
    }

    #[test]
    fn vst3_bundle_loads_processes_and_reports_latency() {
        // The real-VST3 roundtrip: a genuine `*.vst3` bundle (the
        // `ccez-vst3-gain` cdylib fixture, Steinberg ABI via vst3-host)
        // loads inside the sandboxed worker, audio flows through its DSP,
        // and its declared latency lands in the compensation table.
        use crate::plugins::latency::LatencyMap;
        use crate::plugins::vst3::{fixture::ensure_fixture_built, fixture_latency};
        let mut host = host();
        let bundle = ensure_fixture_built();
        host.load(PluginDescriptor::vst3(
            "vgain",
            "Ccez VST3 Gain",
            &bundle.display().to_string(),
        ))
        .expect("load real bundle");
        host.set_param("vgain", "gain", 0.5).expect("set_param");
        let out = host.process("vgain", &[1.0, 0.5, -1.0]).expect("process");
        assert_eq!(out, vec![0.5, 0.25, -0.5]);
        // Latency reporting feeds the delay-compensation map.
        let mut map = LatencyMap::new();
        let lat = host.latency_samples("vgain").expect("latency");
        assert_eq!(lat, fixture_latency());
        map.report("vgain", lat).expect("report");
        assert_eq!(
            map.chain_latency(&["vgain".to_string()]),
            u64::from(fixture_latency())
        );
        // Snapshot/restore round-trips across the real backend, blob included:
        // the blob is a genuine VST3 state chunk, so it is non-empty.
        let state = host.snapshot("vgain").expect("snapshot");
        assert!(
            !state.blob.is_empty(),
            "vst3 snapshot must carry a real state chunk"
        );
        host.set_param("vgain", "gain", 1.0).expect("retune");
        assert_eq!(host.process("vgain", &[1.0]).expect("renders"), vec![1.0]);
        host.restore("vgain", &state).expect("restore");
        assert_eq!(host.process("vgain", &[1.0]).expect("renders"), vec![0.5]);
        assert_eq!(host.snapshot("vgain").expect("state"), state);
    }

    #[test]
    fn vst3_crash_recovers_with_exact_state() {
        // Same crash contract as CLAP: kill the worker mid-session, prove
        // the session survives, recover the exact pre-crash state — except
        // respawn re-loads the real VST3 bundle behind the same path.
        use crate::plugins::vst3::fixture::ensure_fixture_built;
        let mut host = host();
        let bundle = ensure_fixture_built();
        host.load(PluginDescriptor::vst3(
            "doomed-vst3",
            "Doom",
            &bundle.display().to_string(),
        ))
        .expect("load");
        host.set_param("doomed-vst3", "gain", 0.25).expect("gain");
        let before = host.snapshot("doomed-vst3").expect("snapshot");

        host.kill("doomed-vst3").expect("kill");
        assert!(!host.alive("doomed-vst3").expect("alive check"));
        let err = host.process("doomed-vst3", &[1.0]).expect_err("must fail");
        assert!(matches!(
            err,
            HostError::Sandbox(SandboxError::Crashed(_))
        ));

        let restored = host.recover("doomed-vst3").expect("recover");
        assert_eq!(restored, before);
        assert_eq!(
            host.process("doomed-vst3", &[1.0]).expect("renders"),
            vec![0.25]
        );
    }

    #[test]
    fn vst3_missing_bundle_is_a_clean_error() {
        let mut host = host();
        let err = host
            .load(PluginDescriptor::vst3(
                "ghost-vst3",
                "Ghost",
                "/nonexistent/ghost.vst3",
            ))
            .expect_err("missing bundle must fail");
        assert!(matches!(err, HostError::Sandbox(_)), "got {err:?}");
        // The failure leaves no half-loaded entry behind.
        assert!(host.ids().is_empty());
    }

    #[test]
    fn vst3_unknown_param_is_a_clean_error() {
        use crate::plugins::vst3::fixture::ensure_fixture_built;
        let mut host = host();
        let bundle = ensure_fixture_built();
        host.load(PluginDescriptor::vst3(
            "vgain",
            "Ccez VST3 Gain",
            &bundle.display().to_string(),
        ))
        .expect("load");
        let err = host
            .set_param("vgain", "cutoff", 1.0)
            .expect_err("unknown param must fail");
        assert!(
            matches!(err, HostError::Sandbox(SandboxError::Protocol(_))),
            "got {err:?}"
        );
    }

    /// The real-AU roundtrip: Apple's bundled AUDelay (no fixture to
    /// build — macOS ships it) loads inside the sandboxed worker with no
    /// `CCEZ_ENABLE_AU` consult, silence renders to silence through the
    /// effect's input-pull callback, latency lands in the compensation
    /// table, and kill → `Crashed` → recover replays the re-loaded unit.
    /// macOS only; other targets prove the refusal in `au.rs` instead.
    #[cfg(target_os = "macos")]
    #[test]
    fn au_delay_loads_processes_and_reports_latency() {
        use crate::plugins::au::{fourcc, AuComponentDesc, AU_TYPE_EFFECT};
        use crate::plugins::latency::LatencyMap;
        let mut host = host();
        let desc = AuComponentDesc::apple(AU_TYPE_EFFECT, fourcc([b'd', b'e', b'l', b'y']));
        host.load(PluginDescriptor::au("audelay", "AUDelay", desc))
            .expect("load system unit");
        host.set_param("audelay", "Delay Time", 0.5)
            .expect("set_param");
        let out = host.process("audelay", &[0.0; 8]).expect("process");
        assert_eq!(out, vec![0.0; 8]);
        // Latency reporting feeds the delay-compensation map.
        let mut map = LatencyMap::new();
        let lat = host.latency_samples("audelay").expect("latency");
        assert_eq!(lat, 0);
        map.report("audelay", lat).expect("report");
        assert_eq!(map.chain_latency(&["audelay".to_string()]), 0);
        // Snapshot/restore round-trips across the real backend.
        let state = host.snapshot("audelay").expect("snapshot");
        assert_eq!(state.params.get("Delay Time"), Some(&0.5));
        host.set_param("audelay", "Feedback", 10.0).expect("retune");
        host.restore("audelay", &state).expect("restore");
        assert_eq!(host.snapshot("audelay").expect("state"), state);
        // Unknown params are clean errors, not crashes.
        let err = host
            .set_param("audelay", "cutoff", 1.0)
            .expect_err("unknown param must fail");
        assert!(
            matches!(err, HostError::Sandbox(SandboxError::Protocol(_))),
            "got {err:?}"
        );
        // Same crash contract as every backend: kill mid-session, prove
        // the session survives, recover the exact pre-crash state —
        // except respawn re-loads the system unit behind `LoadAu`.
        host.kill("audelay").expect("kill");
        assert!(!host.alive("audelay").expect("alive check"));
        let err = host.process("audelay", &[0.0]).expect_err("must fail");
        assert!(
            matches!(err, HostError::Sandbox(SandboxError::Crashed(_))),
            "got {err:?}"
        );
        let restored = host.recover("audelay").expect("recover");
        assert_eq!(restored, state);
        assert_eq!(
            host.process("audelay", &[0.0; 4]).expect("renders"),
            vec![0.0; 4]
        );
    }

    /// A `Wasm` descriptor without the `wasm-runtime` feature is a clean
    /// protocol error (and leaves no half-loaded entry): the default
    /// build carries no wasmtime, so there is nothing to hand the module
    /// to. (With the feature, the worker-process roundtrip lives in the
    /// `wasm_worker_tests` module below.)
    #[cfg(not(feature = "wasm-runtime"))]
    #[test]
    fn wasm_without_feature_is_a_clean_error() {
        let mut host = host();
        let err = host
            .load(PluginDescriptor::wasm("wgain", "WASM Gain", "/lib/gain.wasm"))
            .expect_err("wasm without the feature must fail");
        assert!(
            matches!(err, HostError::Sandbox(SandboxError::Protocol(_))),
            "got {err:?}"
        );
        assert!(host.ids().is_empty());
    }

    /// An `Au` descriptor without component codes is a clean protocol
    /// error (and leaves no half-loaded entry) — on every platform.
    #[test]
    fn au_descriptor_without_codes_is_a_clean_error() {
        let mut host = host();
        let mut broken = PluginDescriptor::mock("broken-au", "Broken");
        broken.kind = PluginKind::Au;
        broken.au_desc = None;
        let err = host.load(broken).expect_err("missing codes must fail");
        assert!(matches!(err, HostError::Sandbox(_)), "got {err:?}");
        assert!(host.ids().is_empty());
    }

    /// Pre-AU descriptor JSON (no `au_desc` key) still deserializes, and
    /// non-AU descriptors serialize without the key — snapshots from
    /// before this track read back byte-identical.
    #[test]
    fn descriptor_json_stays_compatible() {
        let desc = PluginDescriptor::mock("m", "Mock");
        let json = serde_json::to_string(&desc).expect("serialize");
        assert!(!json.contains("au_desc"), "no new key for old kinds: {json}");
        assert!(
            !json.contains("plugin_uid"),
            "no selection key unless set: {json}"
        );
        let legacy = r#"{"id":"m","name":"Mock","kind":"Mock","path":null}"#;
        let back: PluginDescriptor = serde_json::from_str(legacy).expect("legacy reads");
        assert_eq!(back, desc);
        // A selection, once set, round-trips exactly.
        let sel = PluginDescriptor::clap_with_plugin("s", "Sel", "/lib/a.clap", "com.ccez.gain-two");
        let json = serde_json::to_string(&sel).expect("serialize");
        assert!(json.contains("plugin_uid"), "selection must persist: {json}");
        assert_eq!(
            serde_json::from_str::<PluginDescriptor>(&json).expect("back"),
            sel
        );
    }

    #[test]
    fn clap_second_plugin_loads_by_selection() {
        // The sandboxed path for multi-plugin bundles: the descriptor's
        // selection rides `LoadClap` into the worker, and audio flows
        // through the non-first plugin.
        use crate::plugins::clap::fixture::ensure_fixture_built;
        let mut host = host();
        let bundle = ensure_fixture_built();
        host.load(PluginDescriptor::clap_with_plugin(
            "two",
            "Two",
            &bundle.display().to_string(),
            "com.ccez.gain-two",
        ))
        .expect("load second plugin");
        host.set_param("two", "gain", 0.25).expect("set_param");
        assert_eq!(host.process("two", &[1.0]).expect("process"), vec![0.25]);
        host.ping("two").expect("heartbeat");
        // Crash recovery replays the same selection, not index 0.
        let before = host.snapshot("two").expect("snapshot");
        host.kill("two").expect("kill");
        let restored = host.recover("two").expect("recover");
        assert_eq!(restored, before);
        host.set_param("two", "gain", 0.25).expect("retune");
        assert_eq!(host.process("two", &[1.0]).expect("renders"), vec![0.25]);
    }

    #[test]
    fn clap_unknown_selection_is_a_clean_error() {
        use crate::plugins::clap::fixture::ensure_fixture_built;
        let mut host = host();
        let bundle = ensure_fixture_built();
        let err = host
            .load(PluginDescriptor::clap_with_plugin(
                "ghost",
                "Ghost",
                &bundle.display().to_string(),
                "com.ccez.ghost",
            ))
            .expect_err("unknown selection must fail");
        assert!(matches!(err, HostError::Sandbox(_)), "got {err:?}");
        assert!(host.ids().is_empty());
    }

    #[test]
    fn vst3_second_class_loads_by_selection() {
        // Same contract for VST3: the descriptor's class uid rides
        // `LoadVst3` into the worker, and audio flows through it.
        use crate::plugins::vst3::{fixture::ensure_fixture_built, Vst3Backend};
        let mut host = host();
        let bundle = ensure_fixture_built();
        let path = bundle.display().to_string();
        let uid = Vst3Backend::available_classes(&path)
            .expect("list")
            .into_iter()
            .find(|c| c.name == "Ccez Gain Two")
            .expect("second effect")
            .uid;
        host.load(PluginDescriptor::vst3_with_class(
            "vtwo", "VTwo", &path, &uid,
        ))
        .expect("load second class");
        host.set_param("vtwo", "gain", 0.5).expect("set_param");
        assert_eq!(
            host.process("vtwo", &[1.0, -1.0]).expect("process"),
            vec![0.5, -0.5]
        );
        host.ping("vtwo").expect("heartbeat");
    }

    #[test]
    fn host_heartbeat_answers_on_live_workers() {
        let mut host = host();
        host.load(PluginDescriptor::mock("h", "H")).expect("load");
        host.ping("h").expect("ping");
        assert!(host.ping("ghost").is_err());
    }

    #[test]
    fn state_json_round_trip_is_exact() {
        let state = PluginState::new(
            BTreeMap::from([("gain".to_string(), 0.75)]),
            vec![1, 2, 3, 255],
        );
        let json = state.to_json().expect("to json");
        assert_eq!(PluginState::from_json(&json).expect("from json"), state);
        assert!(PluginState::from_json("{bad").is_err());
    }

    #[test]
    fn latency_table_samples_every_loaded_plugin() {
        use crate::plugins::latency::LatencyMap;
        let mut host = host();
        host.load(PluginDescriptor::mock("a", "A")).expect("load");
        host.load(PluginDescriptor::mock("b", "B")).expect("load");
        let mut map = LatencyMap::new();
        for id in host.ids() {
            let lat = host.latency_samples(&id).expect("latency");
            map.report(&id, lat).expect("report");
        }
        assert_eq!(map.latency_of("a"), 0);
        assert_eq!(map.chain_latency(&["a".to_string(), "b".to_string()]), 0);
        assert!(host.latency_samples("ghost").is_err());
    }

    #[test]
    fn kill_plugin_session_survives_and_state_restores() {
        // The crash-a-plugin recovery test: kill the worker mid-session,
        // prove the host (and a sibling plugin) survive, then recover
        // with the exact pre-crash state.
        let mut host = host();
        host.load(PluginDescriptor::mock("doomed", "Doom")).expect("load");
        host.load(PluginDescriptor::mock("sibling", "Sib")).expect("load");
        host.set_param("doomed", "gain", 0.25).expect("gain");
        host.set_param("sibling", "gain", 2.0).expect("gain");
        let before = host.snapshot("doomed").expect("snapshot");

        // The kill: no handshake, like a segfault.
        host.kill("doomed").expect("kill");
        assert!(!host.alive("doomed").expect("alive check"));

        // Crashed plugin errors; the session and sibling are untouched.
        let err = host.process("doomed", &[1.0]).expect_err("must fail");
        assert!(matches!(
            err,
            HostError::Sandbox(SandboxError::Crashed(_))
        ));
        assert_eq!(
            host.process("sibling", &[1.0]).expect("sibling renders"),
            vec![2.0]
        );
        assert_eq!(host.ids().len(), 2);

        // Recovery: respawn + restore the pre-crash snapshot.
        let restored = host.recover("doomed").expect("recover");
        assert_eq!(restored, before);
        assert!(host.alive("doomed").expect("alive again"));
        assert_eq!(host.process("doomed", &[1.0]).expect("renders"), vec![0.25]);
        assert_eq!(host.snapshot("doomed").expect("state"), before);
    }
}

/// WASM worker-process tests: need feature `wasm-runtime` plus a worker
/// binary built with it (`ensure_wasm_worker_built` stages a temp copy).
/// The guest is the inline gain device from [`crate::plugins::wasm`],
/// staged to a temp file and loaded through the real `LoadWasm` file
/// path — so load → process → param roundtrips cross the actual pipe,
/// and kill → recover replays `Init` + `LoadWasm` + `SetState`. Not part
/// of the default `cargo test`.
#[cfg(all(test, feature = "wasm-runtime"))]
mod wasm_worker_tests {
    use super::*;
    use crate::plugins::wasm::write_gain_guest_file;
    use crate::plugins::worker::ensure_wasm_worker_built;

    fn wasm_host() -> (PluginHost, std::path::PathBuf) {
        let worker_bin = ensure_wasm_worker_built();
        let guest = write_gain_guest_file();
        let host = PluginHost::new(44_100.0, worker_bin);
        (host, guest)
    }

    #[test]
    fn wasm_gain_load_process_param_round_trip() {
        let (mut host, guest) = wasm_host();
        host.load(PluginDescriptor::wasm(
            "wgain",
            "WASM Gain",
            &guest.display().to_string(),
        ))
        .expect("load gain guest");
        // Unity default, like the mock backend.
        assert_eq!(host.process("wgain", &[1.0, -2.0]).expect("process"), vec![1.0, -2.0]);
        // Params cross the worker protocol as `SetParam`.
        host.set_param("wgain", "gain", 0.5).expect("set_param");
        assert_eq!(
            host.process("wgain", &[1.0, 0.5, -1.0]).expect("process"),
            vec![0.5, 0.25, -0.5]
        );
        // Honestly zero-latency, like memory-free native gain.
        assert_eq!(host.latency_samples("wgain").expect("latency"), 0);
        // Snapshot/restore round-trips across the worker boundary.
        let state = host.snapshot("wgain").expect("snapshot");
        assert_eq!(state.params.get("gain"), Some(&0.5));
        host.set_param("wgain", "gain", 2.0).expect("retune");
        assert_eq!(host.process("wgain", &[1.0]).expect("renders"), vec![2.0]);
        host.restore("wgain", &state).expect("restore");
        assert_eq!(host.process("wgain", &[1.0]).expect("renders"), vec![0.5]);
        assert_eq!(host.snapshot("wgain").expect("state"), state);
    }

    #[test]
    fn wasm_crash_recovers_by_reloading_the_module() {
        let (mut host, guest) = wasm_host();
        host.load(PluginDescriptor::wasm(
            "doomed",
            "Doom",
            &guest.display().to_string(),
        ))
        .expect("load");
        host.set_param("doomed", "gain", 0.5).expect("gain");
        let before = host.snapshot("doomed").expect("snapshot");
        assert_eq!(host.process("doomed", &[1.0]).expect("process"), vec![0.5]);

        // The crash: ungraceful kill, no handshake.
        host.kill("doomed").expect("kill");
        assert!(!host.alive("doomed").expect("alive check"));
        let err = host.process("doomed", &[1.0]).expect_err("dead worker must fail");
        assert!(matches!(err, HostError::Sandbox(SandboxError::Crashed(_))), "got {err:?}");

        // Recovery respawns the worker and replays `Init` + `LoadWasm` +
        // `SetState`: the resurrected insert holds the exact pre-crash
        // gain without the test re-sending any param.
        let back = host.recover("doomed").expect("recover");
        assert_eq!(back, before);
        assert!(host.alive("doomed").expect("alive again"));
        assert_eq!(host.process("doomed", &[1.0]).expect("renders"), vec![0.5]);
        assert_eq!(host.snapshot("doomed").expect("state"), before);
    }

    #[test]
    fn wasm_missing_module_is_a_clean_error() {
        let worker_bin = ensure_wasm_worker_built();
        let mut host = PluginHost::new(44_100.0, worker_bin);
        let err = host
            .load(PluginDescriptor::wasm("ghost", "Ghost", "/nonexistent/ghost.wasm"))
            .expect_err("missing module must fail");
        assert!(matches!(err, HostError::Sandbox(_)), "got {err:?}");
        // The failure leaves no half-loaded entry behind (the worker's
        // mock keeps running so the error is observable; `load` then
        // tears the worker down by dropping it).
        assert!(host.ids().is_empty());
    }

    #[test]
    fn wasm_unknown_param_is_a_clean_error() {
        let (mut host, guest) = wasm_host();
        host.load(PluginDescriptor::wasm(
            "wgain",
            "WASM Gain",
            &guest.display().to_string(),
        ))
        .expect("load");
        let err = host
            .set_param("wgain", "cutoff", 1.0)
            .expect_err("unknown param must fail");
        assert!(
            matches!(err, HostError::Sandbox(SandboxError::Protocol(_))),
            "got {err:?}"
        );
        // The refused set never reached the guest: still unity.
        assert_eq!(host.process("wgain", &[1.0]).expect("renders"), vec![1.0]);
    }
}
