//! Real CLAP hosting behind the plugin sandbox (`clack-host`).
//!
//! Teaching note: a `.clap` bundle is untrusted C-ABI code, so it never
//! runs in the DAW process. [`ClapBackend`] lives *inside* the sandboxed
//! [`worker`](super::worker) child: it `dlopen`s the bundle, instantiates
//! the first plugin the entry exposes, activates it at the session rate,
//! and renders mono blocks through it. A segfault still kills only the
//! child — [`PluginHost`](super::host::PluginHost) recovery (respawn +
//! replay the last-known-good [`PluginState`](super::host::PluginState))
//! works unchanged, because the backend speaks the existing worker
//! protocol and keeps the same host-side truth the mock keeps.
//!
//! v1 scope, honestly documented:
//!
//! - Mono only: one input port, one output port, one channel each. A
//!   bundle whose plugin exposes anything else fails [`ClapBackend::load`]
//!   with [`ClapError::PortLayout`] instead of rendering wrong audio.
//! - Params travel as [`ParamValueEvent`](clack_host::events::event_types::ParamValueEvent)s
//!   in every `process` block (worker id strings match CLAP param names,
//!   case-insensitively; values clamp to the plugin's `[min, max]`, the
//!   same rule the engine's `ParamSet` applies). Unknown ids are kept in
//!   the host-side snapshot but never sent — snapshots stay exact.
//! - The opaque `blob` is host-side passthrough, exactly like the mock:
//!   the CLAP state extension (`get_state`/`set_state` streams) is the
//!   follow-up, not this track.
//! - Multi-plugin bundles enumerate every plugin the entry exposes
//!   ([`ClapBackend::available_plugins`]) and instantiate by CLAP id
//!   ([`ClapBackend::load_selected`]); the id-less [`ClapBackend::load`]
//!   keeps the old default (index 0) so single-plugin bundles behave
//!   exactly as before.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};

use clack_extensions::latency::{HostLatency, HostLatencyImpl, PluginLatency};
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_host::events::event_types::ParamValueEvent;
use clack_host::events::io::EventBuffer;
use clack_host::prelude::*;

use super::host::PluginState;

/// Largest single `process` call handed to the plugin. Bigger worker
/// blocks are chunked; smaller ones pass through untouched.
const MAX_BLOCK_FRAMES: u32 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClapError {
    /// The bundle could not be opened, initialized, or instantiated.
    Load(String),
    /// The entry exposes no loadable plugin.
    NoPlugin(String),
    /// The plugin's port layout is not v1 mono in/out.
    PortLayout(String),
    /// Unknown param id on `set_param` (never sent to the plugin).
    BadParam(String),
    /// The plugin refused a `process` call.
    Process(String),
}

impl std::fmt::Display for ClapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(m) => write!(f, "clap load: {m}"),
            Self::NoPlugin(m) => write!(f, "clap bundle has no plugin: {m}"),
            Self::PortLayout(m) => write!(f, "clap port layout: {m}"),
            Self::BadParam(id) => write!(f, "unknown clap param `{id}`"),
            Self::Process(m) => write!(f, "clap process: {m}"),
        }
    }
}

impl std::error::Error for ClapError {}

pub type Result<T> = std::result::Result<T, ClapError>;

/// Host-side callback state: records the plugin's latency-change pings.
/// The fixture never pings (fixed latency), but a real limiter would —
/// the flag is polled by [`ClapBackend::latency_samples`].
#[derive(Default)]
struct SandboxShared {
    latency_changed: AtomicBool,
}

impl<'a> SharedHandler<'a> for SandboxShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

struct SandboxMain<'a> {
    shared: &'a SandboxShared,
}

impl<'a> MainThreadHandler<'a> for SandboxMain<'a> {}

impl HostLatencyImpl for SandboxMain<'_> {
    fn changed(&self) {
        self.shared.latency_changed.store(true, Ordering::SeqCst);
    }
}

struct SandboxHost;

impl HostHandlers for SandboxHost {
    type Shared<'a> = SandboxShared;
    type MainThread<'a> = SandboxMain<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostLatency>();
    }
}

/// One discovered parameter: stable CLAP id plus the range the host
/// clamps worker-side values into.
#[derive(Debug, Clone)]
struct ParamSlot {
    id: ClapId,
    name: String,
    min: f64,
    max: f64,
}

/// A live CLAP instance running inside the worker process.
///
/// Owns the [`PluginInstance`] (main thread) and its started audio
/// processor. [`Drop`] stops and deactivates cleanly so the bundle
/// unloads without leaking the plugin.
pub struct ClapBackend {
    instance: PluginInstance<SandboxHost>,
    processor: Option<StartedPluginAudioProcessor<SandboxHost>>,
    /// Canonical CLAP param names (as the plugin spells them).
    slots: Vec<ParamSlot>,
    /// Current values by canonical name, plus any unknown ids the worker
    /// was told (kept for snapshot exactness, never sent).
    params: BTreeMap<String, f64>,
    blob: Vec<u8>,
    plugin_id: String,
    plugin_name: String,
    sample_rate: f64,
}

/// One plugin inside a (possibly multi-plugin) CLAP bundle: the stable
/// id [`ClapBackend::load_selected`] instantiates plus the display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClapPluginInfo {
    pub id: String,
    pub name: String,
}

impl ClapBackend {
    /// List every plugin the bundle's entry exposes, in factory order.
    /// Pure discovery: nothing is instantiated, so this never fails with
    /// [`ClapError::PortLayout`].
    pub fn available_plugins(path: &str) -> Result<Vec<ClapPluginInfo>> {
        // SAFETY: opening a bundle maps its code, but no plugin instance
        // is created — the worker process boundary still contains it.
        let entry =
            unsafe { PluginEntry::load(path) }.map_err(|e| ClapError::Load(e.to_string()))?;
        let factory = entry
            .get_plugin_factory()
            .ok_or_else(|| ClapError::NoPlugin(path.to_string()))?;
        let mut out = Vec::new();
        for desc in factory.plugin_descriptors() {
            let Some(id) = desc.id() else { continue };
            out.push(ClapPluginInfo {
                id: id.to_string_lossy().into_owned(),
                name: desc
                    .name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            });
        }
        if out.is_empty() {
            return Err(ClapError::NoPlugin(path.to_string()));
        }
        Ok(out)
    }

    /// Load `path`, instantiate the entry's first plugin, discover its
    /// params, activate at `sample_rate`, and start processing.
    pub fn load(path: &str, sample_rate: f64) -> Result<Self> {
        let first = Self::available_plugins(path)?
            .into_iter()
            .next()
            .ok_or_else(|| ClapError::NoPlugin(path.to_string()))?;
        Self::load_selected(path, &first.id, sample_rate)
    }

    /// Load `path` and instantiate the plugin named by `plugin_id`
    /// (one of [`ClapBackend::available_plugins`]), then discover params,
    /// activate at `sample_rate`, and start processing. An unknown id is
    /// [`ClapError::NoPlugin`], never a silent fallback to index 0.
    pub fn load_selected(path: &str, plugin_id: &str, sample_rate: f64) -> Result<Self> {
        // SAFETY: loading a bundle executes its code — that is the point,
        // and the worker process boundary is what contains it.
        let entry =
            unsafe { PluginEntry::load(path) }.map_err(|e| ClapError::Load(e.to_string()))?;
        let factory = entry
            .get_plugin_factory()
            .ok_or_else(|| ClapError::NoPlugin(path.to_string()))?;
        if !factory
            .plugin_descriptors()
            .filter_map(|d| d.id().map(|id| id.to_string_lossy().into_owned()))
            .any(|id| id == plugin_id)
        {
            return Err(ClapError::NoPlugin(format!("{path}: no plugin `{plugin_id}`")));
        }
        let clap_id =
            CString::new(plugin_id).map_err(|_| ClapError::NoPlugin(plugin_id.to_string()))?;
        let plugin_name = factory
            .plugin_descriptors()
            .filter(|d| d.id().is_some_and(|id| id.to_string_lossy() == plugin_id))
            .find_map(|d| d.name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| path.to_string());
        let host_info = HostInfo::new("ccez-daw", "ccez", "https://example.com", "0.1.0")
            .map_err(|e| ClapError::Load(e.to_string()))?;

        let mut instance = PluginInstance::<SandboxHost>::new(
            |_| SandboxShared::default(),
            |shared| SandboxMain { shared },
            &entry,
            &clap_id,
            &host_info,
        )
        .map_err(|e| ClapError::Load(format!("instantiate {}: {e:?}", clap_id.to_string_lossy())))?;

        let (slots, params) = discover_params(&mut instance).map_err(|e| {
            ClapError::Load(format!("{}: {e}", clap_id.to_string_lossy()))
        })?;

        check_mono_layout(&mut instance)
            .map_err(|e| ClapError::PortLayout(format!("{}: {e}", plugin_name)))?;

        let config = PluginAudioConfiguration {
            sample_rate,
            min_frames_count: 1,
            max_frames_count: MAX_BLOCK_FRAMES,
        };
        let stopped = instance
            .activate(|_, _| (), config)
            .map_err(|e| ClapError::Load(format!("activate {plugin_name}: {e:?}")))?;
        let processor = match stopped.start_processing() {
            Ok(started) => started,
            Err(err) => {
                instance.deactivate(err.into_stopped_processor());
                return Err(ClapError::Load(format!("start {plugin_name}")));
            }
        };

        Ok(Self {
            instance,
            processor: Some(processor),
            slots,
            params,
            blob: Vec::new(),
            plugin_id: clap_id.to_string_lossy().into_owned(),
            plugin_name,
            sample_rate,
        })
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Set one param by worker id (matches CLAP names, case-insensitively;
    /// unknown ids are an error, never sent).
    pub fn set_param(&mut self, id: &str, value: f64) -> Result<()> {
        let slot = find_slot(&self.slots, id).ok_or_else(|| ClapError::BadParam(id.to_string()))?;
        self.params
            .insert(slot.name.clone(), value.clamp(slot.min, slot.max));
        Ok(())
    }

    /// Render one mono block. Every call carries the full current param
    /// set as input events, so the plugin converges even after a
    /// kill-and-respawn that skipped `set_param` replays.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(MAX_BLOCK_FRAMES as usize) {
            if chunk.is_empty() {
                continue;
            }
            let Self {
                processor,
                slots,
                params,
                plugin_name,
                ..
            } = self;
            let processor = processor
                .as_mut()
                .ok_or_else(|| ClapError::Process("processor not started".to_string()))?;
            out.extend(Self::process_chunk(processor, slots, params, plugin_name, chunk)?);
        }
        Ok(out)
    }

    /// Render one chunk (≤ [`MAX_BLOCK_FRAMES`]) through a started processor.
    /// Associated function (no `self`) so [`ClapBackend::process`] can hold
    /// the processor loan beside the param tables without double-borrowing.
    fn process_chunk(
        processor: &mut StartedPluginAudioProcessor<SandboxHost>,
    slots: &[ParamSlot],
    params: &BTreeMap<String, f64>,
    plugin_name: &str,
    chunk: &[f32],
) -> Result<Vec<f32>> {
    let mut in_events = EventBuffer::default();
    for slot in slots {
        if let Some(value) = params.get(&slot.name) {
            let event =
                ParamValueEvent::new(0, slot.id, Pckn::new(0u16, 0u16, 0u16, 0u32), *value);
            in_events.push(&event);
        }
    }
    let input_events = in_events.as_input();
    let mut out_events_buf = EventBuffer::default();
    let mut output_events = out_events_buf.as_output();

    let mut input_block = chunk.to_vec();
    let mut output_block = vec![0.0f32; chunk.len()];

    let mut in_ports = AudioPorts::with_capacity(1, 1);
    let inputs = in_ports.with_input_buffers([AudioPortBuffer {
        latency: 0,
        channels: AudioPortBufferType::f32_input_only(core::iter::once(InputChannel {
            buffer: input_block.as_mut_slice(),
            is_constant: false,
        })),
    }]);
    let mut out_ports = AudioPorts::with_capacity(1, 1);
    let mut outputs = out_ports.with_output_buffers([AudioPortBuffer {
        latency: 0,
        channels: AudioPortBufferType::f32_output_only(core::iter::once(
            output_block.as_mut_slice(),
        )),
    }]);

    processor
        .process(
            &inputs,
            &mut outputs,
            &input_events,
            &mut output_events,
            None,
            None,
        )
        .map_err(|e| ClapError::Process(format!("{plugin_name}: {e:?}")))?;
    Ok(output_block)
}

    /// Current processing latency in samples: a live query of the CLAP
    /// latency extension (0 when the plugin exposes none). Feeds
    /// [`LatencyMap`](super::latency::LatencyMap) via the worker's
    /// `GetLatency` op, like the mock's probe.
    pub fn latency_samples(&mut self) -> u32 {
        let handle = self.instance.plugin_handle();
        handle
            .get_extension::<PluginLatency>()
            .map(|lat| lat.get(&handle))
            .unwrap_or(0)
    }

    pub fn state(&self) -> PluginState {
        PluginState::new(self.params.clone(), self.blob.clone())
    }

    /// Push worker-side truth into the backend. Known ids clamp into the
    /// plugin's ranges; unknown ids ride along host-side so snapshots
    /// round-trip exactly (the mock's rule).
    pub fn set_state(&mut self, state: &PluginState) {
        for (id, value) in &state.params {
            match find_slot(&self.slots, id) {
                Some(slot) => {
                    self.params
                        .insert(slot.name.clone(), value.clamp(slot.min, slot.max));
                }
                None => {
                    self.params.insert(id.clone(), *value);
                }
            }
        }
        self.blob = state.blob.clone();
    }
}

impl Drop for ClapBackend {
    fn drop(&mut self) {
        if let Some(processor) = self.processor.take() {
            let stopped = processor.stop_processing();
            self.instance.deactivate(stopped);
        }
    }
}

/// Read the plugin's param list: canonical names, ids, ranges, and seed
/// values (live value when the extension answers, else the default).
fn discover_params(
    instance: &mut PluginInstance<SandboxHost>,
) -> std::result::Result<(Vec<ParamSlot>, BTreeMap<String, f64>), String> {
    let handle = instance.plugin_handle();
    let params = handle
        .get_extension::<PluginParams>()
        .ok_or_else(|| "plugin exposes no params extension".to_string())?;
    let count = params.count(&handle);
    let mut slots = Vec::new();
    let mut values = BTreeMap::new();
    let mut info_buf = ParamInfoBuffer::new();
    for index in 0..count {
        let Some(info) = params.get_info(&handle, index, &mut info_buf) else {
            continue;
        };
        let name = String::from_utf8_lossy(info.name).into_owned();
        let seed = params
            .get_value(&handle, info.id)
            .unwrap_or(info.default_value);
        values.insert(name.clone(), seed.clamp(info.min_value, info.max_value));
        slots.push(ParamSlot {
            id: info.id,
            name,
            min: info.min_value,
            max: info.max_value,
        });
    }
    Ok((slots, values))
}

/// v1 accepts exactly one mono input port and one mono output port.
/// Anything else is a clean refusal, never silent misrouting.
fn check_mono_layout(instance: &mut PluginInstance<SandboxHost>) -> std::result::Result<(), String> {
    use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
    let handle = instance.plugin_handle();
    let Some(ports) = handle.get_extension::<PluginAudioPorts>() else {
        // No audio-ports extension: assume the effect-style mono default
        // the fixture-free path documents (process() still drives port 0).
        return Ok(());
    };
    if ports.count(&handle, true) != 1 || ports.count(&handle, false) != 1 {
        return Err("expected 1 input and 1 output port".to_string());
    }
    let mut info_buf = AudioPortInfoBuffer::new();
    for is_input in [true, false] {
        let Some(info) = ports.get(&handle, 0, is_input, &mut info_buf) else {
            return Err("port 0 has no info".to_string());
        };
        if info.channel_count != 1 {
            return Err(format!("port has {} channels, want 1", info.channel_count));
        }
    }
    Ok(())
}

fn find_slot<'s>(slots: &'s [ParamSlot], id: &str) -> Option<&'s ParamSlot> {
    if let Some(slot) = slots.iter().find(|s| s.name == id) {
        return Some(slot);
    }
    slots
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(id.trim()))
}

#[cfg(test)]
pub(crate) mod fixture {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// The copied `.clap` bundle path, built once per test process.
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();

    /// Build the `ccez-clap-gain` cdylib fixture (when missing) and copy
    /// it to a real `.clap` path. Panics with a clear message when the
    /// build fails — a missing fixture is a setup error, not a test
    /// failure. Mirrors [`super::super::worker::ensure_worker_built`].
    pub(crate) fn ensure_fixture_built() -> PathBuf {
        BUNDLE
            .get_or_init(|| {
                let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("clap-gain/Cargo.toml");
                let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("clap-gain/target/debug");
                let lib = if cfg!(target_os = "macos") {
                    target.join("libccez_clap_gain.dylib")
                } else if cfg!(target_os = "windows") {
                    target.join("ccez_clap_gain.dll")
                } else {
                    target.join("libccez_clap_gain.so")
                };
                if !lib.exists() {
                    let status = std::process::Command::new("cargo")
                        .args(["build", "--manifest-path"])
                        .arg(&manifest)
                        .status()
                        .expect("cargo build launches");
                    assert!(status.success() && lib.exists(), "clap fixture must build");
                }
                // Load through a real `.clap` path: the extension is what
                // scanners and users see, and libloading opens any path.
                let bundle = target.join("ccez-gain.clap");
                let bytes = std::fs::read(&lib).expect("read fixture dylib");
                // Only rewrite when stale, so parallel tests never race a
                // half-written bundle under a concurrent loader.
                let stale = std::fs::read(&bundle).map(|b| b != bytes).unwrap_or(true);
                if stale {
                    let tmp = target.join("ccez-gain.clap.tmp");
                    std::fs::write(&tmp, &bytes).expect("stage fixture bundle");
                    std::fs::rename(&tmp, &bundle).expect("publish fixture bundle");
                }
                bundle
            })
            .clone()
    }

    #[test]
    fn fixture_bundle_loads_and_reports_latency() {
        let bundle = ensure_fixture_built();
        let mut backend =
            super::ClapBackend::load(&bundle.display().to_string(), 44100.0).expect("load");
        assert_eq!(backend.plugin_id(), "com.ccez.gain");
        assert_eq!(backend.latency_samples(), super::fixture_latency());
    }

    #[test]
    fn missing_bundle_is_a_clean_error() {
        match super::ClapBackend::load("/nonexistent/ghost.clap", 44100.0) {
            Err(super::ClapError::Load(_)) => {}
            other => panic!("expected Load error, got {}", other.is_ok()),
        }
    }

    #[test]
    fn bundle_enumerates_all_three_fixture_plugins_in_order() {
        let bundle = ensure_fixture_built();
        let listed =
            super::ClapBackend::available_plugins(&bundle.display().to_string()).expect("list");
        let ids: Vec<&str> = listed.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["com.ccez.gain", "com.ccez.gain-two", "com.ccez.stereo-gain"]
        );
        assert_eq!(listed[0].name, "Ccez Gain");
        assert_eq!(listed[1].name, "Ccez Gain Two");
        // Discovery instantiates nothing, so the stereo entry lists fine.
        assert_eq!(listed[2].name, "Ccez Stereo Gain");
    }

    #[test]
    fn second_plugin_selects_by_id_and_renders() {
        let bundle = ensure_fixture_built();
        let path = bundle.display().to_string();
        let mut backend =
            super::ClapBackend::load_selected(&path, "com.ccez.gain-two", 44100.0).expect("load");
        assert_eq!(backend.plugin_id(), "com.ccez.gain-two");
        assert_eq!(backend.plugin_name(), "Ccez Gain Two");
        backend.set_param("gain", 0.5).expect("set_param");
        assert_eq!(backend.process(&[1.0, -1.0]).expect("process"), vec![0.5, -0.5]);
    }

    #[test]
    fn unknown_plugin_id_is_no_plugin_not_index_zero() {
        let bundle = ensure_fixture_built();
        match super::ClapBackend::load_selected(
            &bundle.display().to_string(),
            "com.ccez.ghost",
            44100.0,
        ) {
            Err(super::ClapError::NoPlugin(_)) => {}
            other => panic!("expected NoPlugin, got {}", other.is_ok()),
        }
    }

    #[test]
    fn stereo_plugin_refuses_with_port_layout() {
        let bundle = ensure_fixture_built();
        match super::ClapBackend::load_selected(
            &bundle.display().to_string(),
            "com.ccez.stereo-gain",
            44100.0,
        ) {
            Err(super::ClapError::PortLayout(msg)) => {
                assert!(msg.contains("2 channels"), "got {msg}");
            }
            other => panic!("expected PortLayout, got ok={}", other.is_ok()),
        }
    }
}

/// Re-exported for tests: the latency the fixture declares, so the
/// roundtrip test asserts reporting instead of hard-coding a constant.
#[cfg(test)]
pub(crate) fn fixture_latency() -> u32 {
    64
}
