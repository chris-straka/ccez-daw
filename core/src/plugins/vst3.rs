//! Track C (VST3 side): discover, load, and run VST3 plugins.
//!
//! New to plugin hosting? Start here. A VST3 plugin is a native module
//! (`.vst3` bundle) that exports one C symbol, `GetPluginFactory`. The host
//! calls it, enumerates the plugin classes inside, and drives the audio
//! ones. This module implements that host side in three layers:
//!
//! 1. **Discovery** ([`Vst3Bundle::scan_dir`]): find `*.vst3` bundles in a
//!    directory. Pure path logic, no loading, never fails loud (a missing
//!    plugin dir just means "no plugins").
//! 2. **Loading** ([`Vst3Host::load_bundle`]): open one bundle as a
//!    [`Vst3Instance`]. The `descriptor.json` dev/test format loads
//!    in-process; native binaries go through [`Vst3Host::load_module`],
//!    which really `dlopen`s the module and enumerates its factory classes
//!    (discovery), while [`Vst3Backend::load`] instantiates the first
//!    audio-effect class for real audio through `vst3-host`.
//! 3. **Running**: the host-side plugin contract
//!    ([`Vst3Instance`]: params as frozen [`Param`](crate::model::Param)s,
//!    block processing, reset) implemented in-process by
//!    [`DescriptorPlugin`] and [`NullVst3Plugin`]; and [`Vst3Backend`],
//!    the sandboxed-worker twin of the CLAP backend (`vst3-host` mono
//!    process, normalized params, live latency, real state chunks).
//!
//! Frozen-contract rule: plugin params reuse the frozen `Param` shape and
//! [`Vst3Instance::as_device_node`] maps a plugin to a frozen
//! `NodeKind::Device` node, so this track adds no project-schema or IPC
//! surface and the typegen drift gate is unaffected. (`Vst3Descriptor` is
//! deliberately *not* registered in `emit.rs`: it is an unstable dev/test
//! format, never written to project files.)
//!
//! Sandbox seam (shared with the sibling CLAP track): out-of-process
//! isolation lives in `plugins/sandbox.rs` + `plugins/worker.rs`.
//! [`Vst3Backend`] runs *inside* that worker behind the additive `LoadVst3`
//! op — the same convergence the CLAP backend uses — so a segfault still
//! kills only the child and [`PluginHost`](super::host::PluginHost)
//! recovery (respawn + replay the last-known-good
//! [`PluginState`](super::host::PluginState)) works unchanged.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::host::PluginState;
use crate::model::{Node, NodeKind, Param};

/// File extension (and bundle marker) for VST3 plugins on every platform.
pub const VST3_EXTENSION: &str = "vst3";

/// Class category string the VST3 SDK assigns to audio effects
/// (`kAudioEffectClass`). Factory classes carrying this category are the
/// ones a DAW instantiates for insert/device slots.
pub const AUDIO_EFFECT_CLASS: &str = "Audio Module Class";

/// `tresult` success code (`kResultOk`). Every factory call is checked
/// against this; anything else is a structured error, never UB.
const K_RESULT_OK: i32 = 0;

#[derive(Debug)]
pub enum Vst3Error {
    /// Filesystem failure while scanning or reading a bundle.
    Io(std::io::Error),
    /// Malformed `descriptor.json`.
    Json(serde_json::Error),
    /// The bundle holds neither a descriptor nor a native module.
    NoModule(PathBuf),
    /// A native module opened but is not a VST3 plugin.
    NoFactory(String),
    /// The factory lists no audio-effect class.
    NoAudioEffect(String),
    /// Malformed dev/test descriptor (missing name, bad channel counts…).
    BadDescriptor(String),
    /// Unknown param id on `set_param`.
    BadParam(String),
    /// Block-shape violation on `process` (empty or mismatched channels).
    Process(String),
    /// The native module file exists but the OS refused to open it.
    Load(String),
}

impl fmt::Display for Vst3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "vst3 io: {e}"),
            Self::Json(e) => write!(f, "vst3 descriptor json: {e}"),
            Self::NoModule(p) => write!(
                f,
                "vst3 bundle has no descriptor.json or native module: {}",
                p.display()
            ),
            Self::NoFactory(m) => write!(f, "not a VST3 module: {m}"),
            Self::NoAudioEffect(m) => write!(f, "no audio effect in factory: {m}"),
            Self::BadDescriptor(m) => write!(f, "bad vst3 descriptor: {m}"),
            Self::BadParam(id) => write!(f, "unknown vst3 param `{id}`"),
            Self::Process(m) => write!(f, "vst3 process: {m}"),
            Self::Load(m) => write!(f, "vst3 load: {m}"),
        }
    }
}

impl std::error::Error for Vst3Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Vst3Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for Vst3Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, Vst3Error>;

/// Parsed metadata for one factory class: the identity a DAW browser,
/// device slot, or op log shows. Built either from a native
/// `PClassInfo` or from a `descriptor.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst3ClassInfo {
    /// 16-byte class id (TUID). `[0; 16]` for descriptor-based plugins.
    pub cid: [u8; 16],
    pub name: String,
    pub category: String,
    pub vendor: String,
    pub version: String,
}

impl Vst3ClassInfo {
    /// `true` when the class is drivable as an insert effect.
    pub fn is_effect(&self) -> bool {
        self.category == AUDIO_EFFECT_CLASS
    }

    /// Lowercase hex of the class id (browser/op-log display).
    pub fn cid_hex(&self) -> String {
        self.cid.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// One `*.vst3` bundle on disk: a directory (all platforms) or a single
/// module file (Linux convention). v1 understands two payloads inside:
///
/// - `Contents/descriptor.json` — unstable dev/test format (see
///   [`Vst3Descriptor`]), loaded in-process.
/// - the platform-native module (`Contents/MacOS/<stem>` on macOS,
///   `Contents/x86_64-linux/<stem>.so` on Linux,
///   `Contents/x86_64-win/<stem>.vst3` on Windows) — opened with
///   [`Vst3Host::load_module`] for factory enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst3Bundle {
    pub path: PathBuf,
    pub name: String,
}

impl Vst3Bundle {
    fn new(path: PathBuf) -> Self {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self { path, name }
    }

    /// Find every `*.vst3` bundle directly inside `dir`, sorted by name.
    /// A missing or unreadable dir yields an empty list — standard plugin
    /// locations often do not exist yet, and that is not an error.
    pub fn scan_dir(dir: &Path) -> Vec<Vst3Bundle> {
        let entries = fs::read_dir(dir);
        let mut out: Vec<Vst3Bundle> = entries
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|ext| ext == VST3_EXTENSION)
                    && (p.is_dir() || p.is_file())
            })
            .map(Vst3Bundle::new)
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Path of the dev/test descriptor payload, if present.
    pub fn descriptor_path(&self) -> PathBuf {
        self.path.join("Contents").join("descriptor.json")
    }

    /// Conventional native-module path for this platform, when the file
    /// actually exists. `None` means "no binary payload" (not an error by
    /// itself — the bundle may be descriptor-based).
    pub fn module_file(&self) -> Option<PathBuf> {
        let stem = self.path.file_stem()?.to_string_lossy().into_owned();
        let rel: PathBuf = if cfg!(target_os = "macos") {
            Path::new("Contents").join("MacOS").join(stem)
        } else if cfg!(target_os = "windows") {
            Path::new("Contents")
                .join("x86_64-win")
                .join(format!("{stem}.vst3"))
        } else {
            Path::new("Contents")
                .join("x86_64-linux")
                .join(format!("{stem}.so"))
        };
        let full = self.path.join(rel);
        full.is_file().then_some(full)
    }
}

/// Unstable v1 dev/test bundle format. This is scaffolding for host
/// development and tests — NOT a frozen contract: it is never written to
/// project files, never mirrored to TypeScript, and may disappear once
/// full binary hosting instantiates real processors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vst3Descriptor {
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub version: String,
    /// Audio channel counts (mono = 1, stereo = 2). Must be equal and
    /// non-zero: v1 effects are channel-preserving.
    #[serde(default = "one")]
    pub inputs: usize,
    #[serde(default = "one")]
    pub outputs: usize,
    /// Initial (and default) param set, in the frozen `Param` shape.
    #[serde(default)]
    pub params: Vec<Param>,
}

fn one() -> usize {
    1
}

impl Vst3Descriptor {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)?;
        let desc: Self = serde_json::from_slice(&bytes)?;
        if desc.name.trim().is_empty() {
            return Err(Vst3Error::BadDescriptor("descriptor needs a name".into()));
        }
        if desc.inputs == 0 || desc.inputs != desc.outputs {
            return Err(Vst3Error::BadDescriptor(format!(
                "v1 effects are channel-preserving; got {} in / {} out",
                desc.inputs, desc.outputs
            )));
        }
        Ok(desc)
    }

    fn class_info(&self) -> Vst3ClassInfo {
        Vst3ClassInfo {
            cid: [0; 16],
            name: self.name.clone(),
            category: AUDIO_EFFECT_CLASS.to_string(),
            vendor: self.vendor.clone(),
            version: self.version.clone(),
        }
    }
}

/// The host-side plugin contract: identity, automatable params, audio,
/// reset. `Send` so a boxed instance can cross into the render thread or
/// a future sandbox worker without changing this interface.
pub trait Vst3Instance: Send {
    fn info(&self) -> &Vst3ClassInfo;
    fn params(&self) -> &[Param];
    fn param(&self, id: &str) -> Option<f64>;
    /// Set a param, clamped to its `[min, max]` range (same clamp rule as
    /// the engine's `ParamSet`). Unknown ids are [`Vst3Error::BadParam`].
    fn set_param(&mut self, id: &str, value: f64) -> Result<()>;
    /// Process one block: `input.len() == output.len() != 0`, all channels
    /// the same length. Implementations must not allocate per call.
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) -> Result<()>;
    /// Restore every param to its default.
    fn reset(&mut self);
    /// Map this plugin to a frozen device node, so tracks can host it
    /// without any project-schema change.
    fn as_device_node(&self, node_id: &str) -> Node {
        Node {
            id: node_id.to_string(),
            kind: NodeKind::Device,
            name: self.info().name.clone(),
            params: self.params().to_vec(),
        }
    }
}

/// Boxed, ready-to-process plugin returned by the host.
pub type LoadedPlugin = Box<dyn Vst3Instance>;

fn clamp_param(p: &mut Param, value: f64) {
    p.value = value.clamp(p.min, p.max);
}

fn check_block(input: &[&[f32]], output: &[&mut [f32]]) -> Result<usize> {
    if input.is_empty() || output.is_empty() {
        return Err(Vst3Error::Process("empty input or output block".into()));
    }
    if input.len() != output.len() {
        return Err(Vst3Error::Process(format!(
            "channel mismatch: {} in / {} out",
            input.len(),
            output.len()
        )));
    }
    let n = input[0].len();
    if input.iter().any(|c| c.len() != n) || output.iter().any(|c| c.len() != n) {
        return Err(Vst3Error::Process("ragged channel lengths".into()));
    }
    Ok(n)
}

/// In-process gain effect behind the dev/test descriptor: every output
/// sample is the matching input sample times the `gain` param (1.0 when
/// the descriptor declares no params).
pub struct DescriptorPlugin {
    info: Vst3ClassInfo,
    params: Vec<Param>,
}

impl DescriptorPlugin {
    pub fn from_descriptor(desc: Vst3Descriptor) -> Self {
        Self {
            info: desc.class_info(),
            params: desc.params,
        }
    }

    fn gain(&self) -> f64 {
        self.param("gain").unwrap_or(1.0)
    }
}

impl Vst3Instance for DescriptorPlugin {
    fn info(&self) -> &Vst3ClassInfo {
        &self.info
    }

    fn params(&self) -> &[Param] {
        &self.params
    }

    fn param(&self, id: &str) -> Option<f64> {
        self.params.iter().find(|p| p.id == id).map(|p| p.value)
    }

    fn set_param(&mut self, id: &str, value: f64) -> Result<()> {
        match self.params.iter_mut().find(|p| p.id == id) {
            Some(p) => {
                clamp_param(p, value);
                Ok(())
            }
            None => Err(Vst3Error::BadParam(id.to_string())),
        }
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) -> Result<()> {
        check_block(input, output)?;
        let gain = self.gain() as f32;
        for (inn, out) in input.iter().zip(output.iter_mut()) {
            for (i, o) in inn.iter().zip(out.iter_mut()) {
                *o = *i * gain;
            }
        }
        Ok(())
    }

    fn reset(&mut self) {
        for p in &mut self.params {
            p.value = p.default;
        }
    }
}

/// Test double proving the host contract (Track B's `NullBackend`
/// pattern): silent-tolerant passthrough with one `gain` param.
pub struct NullVst3Plugin {
    info: Vst3ClassInfo,
    params: Vec<Param>,
}

impl NullVst3Plugin {
    pub fn new(name: &str) -> Self {
        Self {
            info: Vst3ClassInfo {
                cid: [0; 16],
                name: name.to_string(),
                category: AUDIO_EFFECT_CLASS.to_string(),
                vendor: "ccez-test".to_string(),
                version: "0".to_string(),
            },
            params: vec![Param {
                id: "gain".to_string(),
                label: "Gain".to_string(),
                value: 1.0,
                min: 0.0,
                max: 4.0,
                default: 1.0,
                unit: String::new(),
            }],
        }
    }
}

impl Vst3Instance for NullVst3Plugin {
    fn info(&self) -> &Vst3ClassInfo {
        &self.info
    }

    fn params(&self) -> &[Param] {
        &self.params
    }

    fn param(&self, id: &str) -> Option<f64> {
        self.params.iter().find(|p| p.id == id).map(|p| p.value)
    }

    fn set_param(&mut self, id: &str, value: f64) -> Result<()> {
        match self.params.iter_mut().find(|p| p.id == id) {
            Some(p) => {
                clamp_param(p, value);
                Ok(())
            }
            None => Err(Vst3Error::BadParam(id.to_string())),
        }
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) -> Result<()> {
        check_block(input, output)?;
        let gain = self.params[0].value as f32;
        for (inn, out) in input.iter().zip(output.iter_mut()) {
            for (i, o) in inn.iter().zip(out.iter_mut()) {
                *o = *i * gain;
            }
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.params[0].value = self.params[0].default;
    }
}

// ---------------------------------------------------------------------------
// Native module loading: real `dlopen` + `GetPluginFactory` enumeration.
// ---------------------------------------------------------------------------

/// Factory info struct, byte-layout mirror of the VST3 SDK `PFactoryInfo`
/// (three 64-byte C strings + flags). Only read, never constructed.
#[repr(C)]
struct PFactoryInfoRaw {
    vendor: [u8; 64],
    url: [u8; 64],
    email: [u8; 64],
    flags: i32,
}

/// Class info struct, byte-layout mirror of the VST3 SDK `PClassInfo`
/// (16-byte TUID + cardinality + 32-byte category + 64-byte name).
#[repr(C)]
struct PClassInfoRaw {
    cid: [u8; 16],
    cardinality: i32,
    category: [u8; 32],
    name: [u8; 64],
}

#[repr(C)]
struct IPluginFactory {
    vtbl: *const IPluginFactoryVtbl,
}

/// `IPluginFactory` vtable prefix the host actually calls. `createInstance`
/// is declared (layout correctness) but not called in v1 — instantiation
/// of real binary processors is the phase-2 follow-up.
#[repr(C)]
struct IPluginFactoryVtbl {
    query_interface: unsafe extern "system" fn(
        this: *mut IPluginFactory,
        iid: *const u8,
        obj: *mut *mut c_void,
    ) -> i32,
    add_ref: unsafe extern "system" fn(this: *mut IPluginFactory) -> u32,
    release: unsafe extern "system" fn(this: *mut IPluginFactory) -> u32,
    get_factory_info:
        unsafe extern "system" fn(this: *mut IPluginFactory, info: *mut PFactoryInfoRaw) -> i32,
    count_classes: unsafe extern "system" fn(this: *mut IPluginFactory) -> i32,
    get_class_info:
        unsafe extern "system" fn(this: *mut IPluginFactory, index: i32, info: *mut PClassInfoRaw) -> i32,
    create_instance:
        unsafe extern "system" fn(
            this: *mut IPluginFactory,
            cid: *const u8,
            iid: *const u8,
            obj: *mut *mut c_void,
        ) -> i32,
}

type GetFactoryFn = unsafe extern "system" fn() -> *mut IPluginFactory;

fn c_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// The host entry point: scan directories, open bundles, enumerate native
/// factories. Stateless — all plugin state lives in the returned instance.
pub struct Vst3Host;

impl Vst3Host {
    /// Scan `dir` for bundles (see [`Vst3Bundle::scan_dir`]).
    pub fn scan(dir: &Path) -> Vec<Vst3Bundle> {
        Vst3Bundle::scan_dir(dir)
    }

    /// Open one bundle. Descriptor payloads load in-process today; bundles
    /// holding only a native module report their factory classes via
    /// [`Vst3Host::load_module`] and return [`Vst3Error::NoAudioEffect`]
    /// until binary instantiation lands (phase 2) — the classes are still
    /// enumerated first, so a loadable module never misreports as missing.
    pub fn load_bundle(bundle: &Vst3Bundle) -> Result<LoadedPlugin> {
        let desc_path = bundle.descriptor_path();
        if desc_path.is_file() {
            let desc = Vst3Descriptor::read(&desc_path)?;
            return Ok(Box::new(DescriptorPlugin::from_descriptor(desc)));
        }
        if let Some(module) = bundle.module_file() {
            let classes = Self::load_module(&module)?;
            let names: Vec<&str> = classes.iter().map(|c| c.name.as_str()).collect();
            return Err(Vst3Error::NoAudioEffect(format!(
                "binary instantiation is phase 2; factory of {} lists: {}",
                module.display(),
                names.join(", ")
            )));
        }
        Err(Vst3Error::NoModule(bundle.path.clone()))
    }

    /// Really open a native module and enumerate its factory classes.
    /// This is a genuine `dlopen` round-trip: a library without the
    /// `GetPluginFactory` export yields [`Vst3Error::NoFactory`], a null
    /// factory or unreadable class list likewise. The library is released
    /// on return — callers keep only copied-out plain data, which is why
    /// v1 stops at enumeration (phase 2 retains the handle to
    /// instantiate).
    pub fn load_module(path: &Path) -> Result<Vec<Vst3ClassInfo>> {
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| Vst3Error::Load(format!("{}: {e}", path.display())))?;
        let get_factory: libloading::Symbol<GetFactoryFn> = unsafe {
            lib.get(b"GetPluginFactory")
                .map_err(|_| {
                    Vst3Error::NoFactory(format!(
                        "{} exports no GetPluginFactory",
                        path.display()
                    ))
                })?
        };
        let factory = unsafe { get_factory() };
        if factory.is_null() {
            return Err(Vst3Error::NoFactory(format!(
                "{} returned a null factory",
                path.display()
            )));
        }
        let count = unsafe { ((*(*factory).vtbl).count_classes)(factory) };
        if count < 0 {
            return Err(Vst3Error::NoFactory(format!(
                "{} reported a negative class count",
                path.display()
            )));
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut raw = PClassInfoRaw {
                cid: [0; 16],
                cardinality: 0,
                category: [0; 32],
                name: [0; 64],
            };
            let res = unsafe { ((*(*factory).vtbl).get_class_info)(factory, i, &mut raw) };
            if res != K_RESULT_OK {
                continue;
            }
            out.push(Vst3ClassInfo {
                cid: raw.cid,
                name: c_string(&raw.name),
                category: c_string(&raw.category),
                vendor: String::new(),
                version: String::new(),
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Real binary hosting: VST3 audio through `vst3-host`, inside the worker.
// ---------------------------------------------------------------------------
//
// Teaching note: this is the VST3 twin of [`ClapBackend`](super::clap::ClapBackend)
// and lives in the same place — *inside* the sandboxed [`worker`](super::worker)
// child, behind the additive `LoadVst3` op. Untrusted C-ABI code stays behind
// the process boundary; the host, sandbox, and recovery paths never branch on
// format. Compare the two backends side by side: same shape (load at a rate,
// mono `process`, clamped params, live latency query, host-side state truth),
// different plugin ABI underneath.
//
// v1 scope, honestly documented (mirrors the CLAP v1 limits):
//
// - Mono only: one input bus, one output bus, one channel each. Anything
//   else fails [`Vst3Backend::load`] with [`Vst3BackendError::PortLayout`]
//   instead of rendering wrong audio.
// - Params are VST3 normalized values (`0.0..=1.0` by spec). Worker ids
//   match plugin param names case-insensitively and clamp to the plugin's
//   `[min, max]` (the same rule the engine's `ParamSet` applies). Unknown
//   ids are kept in the host-side snapshot but never sent — snapshots stay
//   exact.
// - The opaque `blob` is a *real* VST3 state chunk now (not passthrough):
//   [`Vst3Backend::state`] reads it from `save_state` (`IComponent::getState`
//   on the fixture) and [`Vst3Backend::set_state`] pushes it back through
//   `load_state` — best-effort on the plugin side, exact on the host side,
//   which is what `recover()` replays.
// - Multi-effect bundles enumerate every factory class
//   ([`Vst3Backend::available_classes`]) and instantiate by class id
//   ([`Vst3Backend::load_class`]); the id-less [`Vst3Backend::load`]
//   keeps the old default (first audio-effect class).

/// Largest single `process` call handed to the plugin. Bigger worker
/// blocks are chunked; smaller ones pass through untouched.
const VST3_MAX_BLOCK_FRAMES: usize = 1024;

/// One audio-effect class inside a (possibly multi-effect) VST3 bundle:
/// the 32-hex-char uid [`Vst3Backend::load_class`] instantiates plus the
/// display name and factory category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst3BackendClass {
    pub uid: String,
    pub name: String,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vst3BackendError {
    /// The bundle could not be opened, negotiated, or started.
    /// (A factory with no audio-effect class fails inside `load_plugin`
    /// and surfaces here too — the loader never returns a plugin the host
    /// cannot drive.)
    Load(String),
    /// The plugin's bus layout is not v1 mono in/out.
    PortLayout(String),
    /// Unknown param id on `set_param` (never sent to the plugin).
    BadParam(String),
    /// The plugin refused a `process` call.
    Process(String),
}

impl fmt::Display for Vst3BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(m) => write!(f, "vst3 load: {m}"),
            Self::PortLayout(m) => write!(f, "vst3 bus layout: {m}"),
            Self::BadParam(id) => write!(f, "unknown vst3 param `{id}`"),
            Self::Process(m) => write!(f, "vst3 process: {m}"),
        }
    }
}

impl std::error::Error for Vst3BackendError {}

pub type BackendResult<T> = std::result::Result<T, Vst3BackendError>;

/// One discovered parameter: stable VST3 id plus the normalized range the
/// host clamps worker-side values into.
#[derive(Debug, Clone)]
struct Vst3ParamSlot {
    id: u32,
    name: String,
    min: f64,
    max: f64,
}

/// A live VST3 instance running inside the worker process.
///
/// Owns the [`vst3_host::Plugin`](https://docs.rs/vst3-host) (loaded
/// in-process — which *is* the worker child, so a crash still kills only
/// the child). [`Drop`] stops processing so the bundle unloads cleanly.
pub struct Vst3Backend {
    plugin: vst3_host::Plugin,
    /// Canonical param names (as the plugin spells them).
    slots: Vec<Vst3ParamSlot>,
    /// Current values by canonical name, plus any unknown ids the worker
    /// was told (kept for snapshot exactness, never sent).
    params: BTreeMap<String, f64>,
    blob: Vec<u8>,
    plugin_id: String,
    plugin_name: String,
    sample_rate: f64,
}

impl Vst3Backend {
    /// List every class the bundle's factory exports (effects and
    /// controllers alike), in factory order. Pure discovery: nothing is
    /// instantiated, so this never fails with
    /// [`Vst3BackendError::PortLayout`].
    pub fn available_classes(path: &str) -> BackendResult<Vec<Vst3BackendClass>> {
        let info = vst3_host::discovery::get_detailed_plugin_info(std::path::Path::new(path))
            .map_err(|e| Vst3BackendError::Load(format!("{path}: {e}")))?;
        Ok(info
            .classes
            .into_iter()
            .map(|c| Vst3BackendClass {
                uid: c.class_id,
                name: c.name,
                category: c.category,
            })
            .collect())
    }

    /// Load `path` (a real `*.vst3` bundle dir), instantiate its first
    /// audio-effect class, negotiate mono buses at `sample_rate`, discover
    /// its params, and start processing.
    pub fn load(path: &str, sample_rate: f64) -> BackendResult<Self> {
        Self::load_inner(path, sample_rate, None)
    }

    /// Load `path` and instantiate the audio-effect class named by
    /// `class_id` (one of [`Vst3Backend::available_classes`]), then
    /// negotiate mono buses at `sample_rate`, discover params, and start
    /// processing. An unknown id is [`Vst3BackendError::Load`], never a
    /// silent fallback to the first class.
    pub fn load_class(path: &str, sample_rate: f64, class_id: &str) -> BackendResult<Self> {
        Self::load_inner(path, sample_rate, Some(class_id))
    }

    fn load_inner(
        path: &str,
        sample_rate: f64,
        class_id: Option<&str>,
    ) -> BackendResult<Self> {
        // SAFETY: loading a bundle executes its code — that is the point,
        // and the worker process boundary is what contains it.
        let mut host = vst3_host::Vst3Host::builder()
            .sample_rate(sample_rate)
            .block_size(VST3_MAX_BLOCK_FRAMES)
            .input_channels(1)
            .output_channels(1)
            .build()
            .map_err(|e| Vst3BackendError::Load(format!("{path}: {e}")))?;
        let mut plugin = match class_id {
            Some(id) => host
                .load_plugin_class(path, id)
                .map_err(|e| Vst3BackendError::Load(format!("{path}: {e}")))?,
            None => host
                .load_plugin(path)
                .map_err(|e| Vst3BackendError::Load(format!("{path}: {e}")))?,
        };

        check_mono_layout(&plugin, path)?;

        let (slots, params) = discover_vst3_params(&plugin)
            .map_err(|e| Vst3BackendError::Load(format!("{path}: {e}")))?;

        plugin
            .start_processing()
            .map_err(|e| Vst3BackendError::Load(format!("start {}: {e}", plugin.info().name)))?;

        let blob = plugin.save_state().unwrap_or_default();
        Ok(Self {
            plugin_id: plugin.info().uid.clone(),
            plugin_name: plugin.info().name.clone(),
            plugin,
            slots,
            params,
            blob,
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

    /// Set one param by worker id (matches plugin names,
    /// case-insensitively; unknown ids are an error, never sent).
    pub fn set_param(&mut self, id: &str, value: f64) -> BackendResult<()> {
        let slot = find_vst3_slot(&self.slots, id)
            .ok_or_else(|| Vst3BackendError::BadParam(id.to_string()))?;
        let clamped = value.clamp(slot.min, slot.max);
        self.plugin
            .set_parameter(slot.id, clamped)
            .map_err(|e| Vst3BackendError::Process(e.to_string()))?;
        self.params.insert(slot.name.clone(), clamped);
        Ok(())
    }

    /// Render one mono block. Every call re-sends the full current param
    /// set first, so the plugin converges even after a kill-and-respawn
    /// that skipped `set_param` replays (the CLAP backend's rule).
    pub fn process(&mut self, input: &[f32]) -> BackendResult<Vec<f32>> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(VST3_MAX_BLOCK_FRAMES) {
            if chunk.is_empty() {
                continue;
            }
            let Self {
                plugin,
                slots,
                params,
                plugin_name,
                sample_rate,
                ..
            } = self;
            out.extend(Self::process_chunk(
                plugin,
                slots,
                params,
                plugin_name,
                *sample_rate,
                chunk,
            )?);
        }
        Ok(out)
    }

    /// Render one chunk (≤ [`VST3_MAX_BLOCK_FRAMES`]) through a started
    /// plugin. Associated function (no `self`) so [`Vst3Backend::process`]
    /// can hold the plugin loan beside the param tables without
    /// double-borrowing.
    fn process_chunk(
        plugin: &mut vst3_host::Plugin,
        slots: &[Vst3ParamSlot],
        params: &BTreeMap<String, f64>,
        plugin_name: &str,
        sample_rate: f64,
        chunk: &[f32],
    ) -> BackendResult<Vec<f32>> {
        for slot in slots {
            if let Some(value) = params.get(&slot.name) {
                plugin
                    .set_parameter(slot.id, *value)
                    .map_err(|e| Vst3BackendError::Process(format!("{plugin_name}: {e}")))?;
            }
        }
        let mut buffers =
            vst3_host::audio::AudioBuffers::new(1, 1, chunk.len(), sample_rate);
        buffers.inputs[0].copy_from_slice(chunk);
        plugin
            .process_audio(&mut buffers)
            .map_err(|e| Vst3BackendError::Process(format!("{plugin_name}: {e}")))?;
        Ok(buffers.outputs[0].clone())
    }

    /// Current processing latency in samples: a live query of
    /// `IAudioProcessor::getLatencySamples` (0 when the plugin reports
    /// none). Feeds [`LatencyMap`](super::latency::LatencyMap) via the
    /// worker's `GetLatency` op, like the CLAP backend's probe.
    pub fn latency_samples(&self) -> u32 {
        self.plugin.latency_samples()
    }

    pub fn state(&self) -> PluginState {
        PluginState::new(self.params.clone(), self.blob.clone())
    }

    /// Push worker-side truth into the backend. Known ids clamp into the
    /// plugin's ranges; unknown ids ride along host-side so snapshots
    /// round-trip exactly (the mock's rule). The blob goes back through
    /// `load_state` best-effort — the host-side copy is adopted regardless,
    /// so snapshot exactness never depends on the plugin accepting it.
    pub fn set_state(&mut self, state: &PluginState) {
        for (id, value) in &state.params {
            match find_vst3_slot(&self.slots, id) {
                Some(slot) => {
                    let clamped = value.clamp(slot.min, slot.max);
                    let _ = self.plugin.set_parameter(slot.id, clamped);
                    self.params.insert(slot.name.clone(), clamped);
                }
                None => {
                    self.params.insert(id.clone(), *value);
                }
            }
        }
        if !state.blob.is_empty() {
            let _ = self.plugin.load_state(&state.blob);
        }
        self.blob = state.blob.clone();
    }
}

impl Drop for Vst3Backend {
    fn drop(&mut self) {
        let _ = self.plugin.stop_processing();
    }
}

/// v1 accepts exactly one mono input bus and one mono output bus.
/// Anything else is a clean refusal, never silent misrouting.
fn check_mono_layout(plugin: &vst3_host::Plugin, path: &str) -> BackendResult<()> {
    let layout = plugin
        .audio_bus_layout()
        .map_err(|e| Vst3BackendError::PortLayout(format!("{path}: {e}")))?;
    let mono = |label: &str, buses: &[vst3_host::audio::AudioBusConfig]| -> BackendResult<()> {
        if buses.len() != 1 {
            return Err(Vst3BackendError::PortLayout(format!(
                "{path}: expected 1 {label} bus, found {}",
                buses.len()
            )));
        }
        if buses[0].channel_count != 1 {
            return Err(Vst3BackendError::PortLayout(format!(
                "{path}: {label} bus has {} channels, want 1",
                buses[0].channel_count
            )));
        }
        if !buses[0].active {
            return Err(Vst3BackendError::PortLayout(format!(
                "{path}: {label} bus is inactive"
            )));
        }
        Ok(())
    };
    mono("input", &layout.inputs)?;
    mono("output", &layout.outputs)?;
    Ok(())
}

/// Read the plugin's param list: canonical names, ids, normalized ranges,
/// and seed values (live value when the controller answers).
fn discover_vst3_params(
    plugin: &vst3_host::Plugin,
) -> std::result::Result<(Vec<Vst3ParamSlot>, BTreeMap<String, f64>), String> {
    let infos = plugin.get_parameters().map_err(|e| e.to_string())?;
    let mut slots = Vec::new();
    let mut values = BTreeMap::new();
    for info in infos {
        // VST3 values are normalized by spec; a plugin reporting an
        // inverted or empty range is a hostile descriptor — refuse it.
        if !(info.min < info.max) {
            return Err(format!("param `{}` has no range", info.name));
        }
        values.insert(
            info.name.clone(),
            info.value.clamp(info.min, info.max),
        );
        slots.push(Vst3ParamSlot {
            id: info.id,
            name: info.name,
            min: info.min,
            max: info.max,
        });
    }
    Ok((slots, values))
}

fn find_vst3_slot<'s>(slots: &'s [Vst3ParamSlot], id: &str) -> Option<&'s Vst3ParamSlot> {
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

    /// The staged `CcezGain.vst3` bundle path, built once per test process.
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();

    /// Build the `ccez-vst3-gain` cdylib fixture (when missing) and stage
    /// it as a real `.vst3` bundle dir. Panics with a clear message when
    /// the build fails — a missing fixture is a setup error, not a test
    /// failure. Mirrors [`super::super::worker::ensure_worker_built`] and
    /// [`super::super::clap::fixture::ensure_fixture_built`].
    pub(crate) fn ensure_fixture_built() -> PathBuf {
        BUNDLE
            .get_or_init(|| {
                let manifest =
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vst3-gain/Cargo.toml");
                let target =
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vst3-gain/target/debug");
                let lib = if cfg!(target_os = "macos") {
                    target.join("libccez_vst3_gain.dylib")
                } else if cfg!(target_os = "windows") {
                    target.join("ccez_vst3_gain.dll")
                } else {
                    target.join("libccez_vst3_gain.so")
                };
                if !lib.exists() {
                    let status = std::process::Command::new("cargo")
                        .args(["build", "--manifest-path"])
                        .arg(&manifest)
                        .status()
                        .expect("cargo build launches");
                    assert!(status.success() && lib.exists(), "vst3 fixture must build");
                }
                stage_bundle(&target, &lib)
            })
            .clone()
    }

    /// Stage the cdylib as a platform-real bundle dir. Only rewrites when
    /// stale, so parallel tests never race a half-written binary under a
    /// concurrent loader.
    fn stage_bundle(target: &PathBuf, lib: &PathBuf) -> PathBuf {
        let bundle = target.join("CcezGain.vst3");
        let binary = if cfg!(target_os = "macos") {
            bundle.join("Contents").join("MacOS").join("CcezGain")
        } else if cfg!(target_os = "windows") {
            bundle
                .join("Contents")
                .join("x86_64-win")
                .join("CcezGain.vst3")
        } else {
            bundle
                .join("Contents")
                .join("x86_64-linux")
                .join("CcezGain.so")
        };
        let bytes = std::fs::read(lib).expect("read fixture dylib");
        let stale = std::fs::read(&binary).map(|b| b != bytes).unwrap_or(true);
        if stale {
            let dir = binary.parent().expect("binary dir");
            std::fs::create_dir_all(dir).expect("bundle dir");
            let tmp = dir.join("CcezGain.bin.tmp");
            std::fs::write(&tmp, &bytes).expect("stage fixture binary");
            std::fs::rename(&tmp, &binary).expect("publish fixture binary");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mut perms = std::fs::metadata(&binary).expect("meta").permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&binary, perms).expect("chmod fixture binary");
            }
        }
        if cfg!(target_os = "macos") {
            let plist = bundle.join("Contents").join("Info.plist");
            if !plist.is_file() {
                std::fs::write(
                    &plist,
                    concat!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
                        "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
                        "<plist version=\"1.0\"><dict>\n",
                        "<key>CFBundleExecutable</key><string>CcezGain</string>\n",
                        "<key>CFBundleIdentifier</key><string>com.ccez.gain</string>\n",
                        "<key>CFBundleName</key><string>Ccez Gain</string>\n",
                        "<key>CFBundlePackageType</key><string>BNDL</string>\n",
                        "</dict></plist>\n",
                    ),
                )
                .expect("write Info.plist");
            }
        }
        bundle
    }

    #[test]
    fn fixture_bundle_loads_and_reports_latency() {
        let bundle = ensure_fixture_built();
        assert!(bundle.is_dir(), "a real bundle dir: {}", bundle.display());
        let backend =
            super::Vst3Backend::load(&bundle.display().to_string(), 44100.0).expect("load");
        assert!(!backend.plugin_id().is_empty());
        assert_eq!(backend.plugin_name(), "Ccez Gain");
        assert_eq!(backend.latency_samples(), super::fixture_latency());
    }

    #[test]
    fn fixture_gain_audio_is_sample_exact() {
        let bundle = ensure_fixture_built();
        let mut backend =
            super::Vst3Backend::load(&bundle.display().to_string(), 44100.0).expect("load");
        backend.set_param("gain", 0.5).expect("set_param");
        let out = backend.process(&[1.0, 0.5, -1.0]).expect("process");
        assert_eq!(out, vec![0.5, 0.25, -0.5]);
    }

    #[test]
    fn missing_bundle_is_a_clean_error() {
        match super::Vst3Backend::load("/nonexistent/ghost.vst3", 44100.0) {
            Err(super::Vst3BackendError::Load(_)) => {}
            other => panic!("expected Load error, got {}", other.is_ok()),
        }
    }

    #[test]
    fn bundle_enumerates_effects_and_controller_in_factory_order() {
        let bundle = ensure_fixture_built();
        let listed =
            super::Vst3Backend::available_classes(&bundle.display().to_string()).expect("list");
        let names: Vec<&str> = listed.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Ccez Gain",
                "Ccez Gain Two",
                "Ccez Stereo Gain",
                "Ccez Gain"
            ]
        );
        let effects: Vec<&str> = listed
            .iter()
            .filter(|c| c.category == super::AUDIO_EFFECT_CLASS)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(effects, vec!["Ccez Gain", "Ccez Gain Two", "Ccez Stereo Gain"]);
        for class in &listed {
            assert_eq!(class.uid.len(), 32, "uid is 32 hex chars: {}", class.uid);
            assert!(class.uid.chars().all(|ch| ch.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn second_effect_selects_by_class_id_and_renders() {
        let bundle = ensure_fixture_built();
        let path = bundle.display().to_string();
        let listed = super::Vst3Backend::available_classes(&path).expect("list");
        let two = listed
            .iter()
            .find(|c| c.name == "Ccez Gain Two")
            .expect("second effect");
        let mut backend =
            super::Vst3Backend::load_class(&path, 44100.0, &two.uid).expect("load_class");
        assert_eq!(backend.plugin_id(), two.uid);
        assert_eq!(backend.plugin_name(), "Ccez Gain Two");
        backend.set_param("gain", 0.5).expect("set_param");
        let out = backend.process(&[1.0, 0.5, -1.0]).expect("process");
        assert_eq!(out, vec![0.5, 0.25, -0.5]);
    }

    #[test]
    fn unknown_class_id_is_a_clean_load_error() {
        let bundle = ensure_fixture_built();
        match super::Vst3Backend::load_class(
            &bundle.display().to_string(),
            44100.0,
            "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
        ) {
            Err(super::Vst3BackendError::Load(_)) => {}
            other => panic!("expected Load error, got {}", other.is_ok()),
        }
    }

    #[test]
    fn stereo_effect_refuses_with_port_layout() {
        let bundle = ensure_fixture_built();
        let path = bundle.display().to_string();
        let listed = super::Vst3Backend::available_classes(&path).expect("list");
        let stereo = listed
            .iter()
            .find(|c| c.name == "Ccez Stereo Gain")
            .expect("stereo effect");
        match super::Vst3Backend::load_class(&path, 44100.0, &stereo.uid) {
            Err(super::Vst3BackendError::PortLayout(msg)) => {
                assert!(msg.contains("2 channels"), "got {msg}");
            }
            other => panic!("expected PortLayout, got {}", other.is_ok()),
        }
    }

    #[test]
    fn unknown_param_is_bad_param() {
        let bundle = ensure_fixture_built();
        let mut backend =
            super::Vst3Backend::load(&bundle.display().to_string(), 44100.0).expect("load");
        match backend.set_param("cutoff", 1.0) {
            Err(super::Vst3BackendError::BadParam(id)) => assert_eq!(id, "cutoff"),
            other => panic!("expected BadParam, got {other:?}"),
        }
    }
}

/// Re-exported for tests: the latency the fixture declares, so the
/// roundtrip test asserts reporting instead of hard-coding a constant.
#[cfg(test)]
pub(crate) fn fixture_latency() -> u32 {
    32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRS: AtomicU64 = AtomicU64::new(0);

    /// Unique scratch dir per test (no `tempfile` dep in core).
    fn scratch(name: &str) -> PathBuf {
        let n = TEST_DIRS.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "ccez-vst3-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn write_gain_bundle(dir: &Path, bundle: &str, gain: f64) -> PathBuf {
        let contents = dir.join(bundle).join("Contents");
        fs::create_dir_all(&contents).expect("bundle contents");
        let desc = serde_json::json!({
            "name": "TestGain",
            "vendor": "ccez",
            "version": "1",
            "inputs": 2,
            "outputs": 2,
            "params": [
                {"id": "gain", "label": "Gain", "value": gain,
                 "min": 0.0, "max": 4.0, "default": 1.0, "unit": ""}
            ],
        });
        fs::write(
            contents.join("descriptor.json"),
            serde_json::to_string_pretty(&desc).expect("json"),
        )
        .expect("descriptor");
        dir.join(bundle)
    }

    fn stereo_block(value: f32, frames: usize) -> (Vec<f32>, Vec<f32>) {
        (vec![value; frames], vec![value; frames])
    }

    #[test]
    fn one_vst3_plugin_loads() {
        let dir = scratch("load");
        let bundle_path = write_gain_bundle(&dir, "TestGain.vst3", 2.0);

        // Scan finds exactly the one bundle.
        let found = Vst3Host::scan(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, bundle_path);
        assert_eq!(found[0].name, "TestGain");

        // Load instantiates it through the full host path.
        let mut plugin = Vst3Host::load_bundle(&found[0]).expect("load");
        assert_eq!(plugin.info().name, "TestGain");
        assert!(plugin.info().is_effect());
        assert_eq!(plugin.param("gain"), Some(2.0));

        // Audio runs: 0.5 in × gain 2.0 = 1.0 out, sample-exact.
        let (left, right) = stereo_block(0.5, 64);
        let mut out_l = vec![0.0f32; 64];
        let mut out_r = vec![0.0f32; 64];
        plugin
            .process(
                &[left.as_slice(), right.as_slice()],
                &mut [out_l.as_mut_slice(), out_r.as_mut_slice()],
            )
            .expect("process");
        assert!(out_l.iter().all(|&s| s == 1.0));
        assert!(out_r.iter().all(|&s| s == 1.0));

        // Param changes take effect on the next block; reset restores.
        plugin.set_param("gain", 0.25).expect("set_param");
        plugin
            .process(
                &[left.as_slice(), right.as_slice()],
                &mut [out_l.as_mut_slice(), out_r.as_mut_slice()],
            )
            .expect("process");
        assert!(out_l.iter().all(|&s| s == 0.125));
        plugin.reset();
        assert_eq!(plugin.param("gain"), Some(1.0));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_ignores_non_bundles_and_sorts() {
        let dir = scratch("scan");
        fs::create_dir_all(dir.join("B.vst3").join("Contents")).expect("b");
        fs::create_dir_all(dir.join("A.vst3").join("Contents")).expect("a");
        fs::write(dir.join("notes.txt"), "not a plugin").expect("txt");
        fs::create_dir_all(dir.join("plain-dir")).expect("plain");
        let found = Vst3Host::scan(&dir);
        assert_eq!(
            found.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        // A missing dir is "no plugins", not an error.
        assert!(Vst3Host::scan(&dir.join("nope")).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_without_payload_is_no_module() {
        let dir = scratch("empty");
        let path = dir.join("Empty.vst3");
        fs::create_dir_all(&path).expect("bundle");
        let bundle = Vst3Bundle::new(path.clone());
        assert!(bundle.module_file().is_none());
        match Vst3Host::load_bundle(&bundle) {
            Err(Vst3Error::NoModule(p)) => assert_eq!(p, path),
            Err(e) => panic!("want NoModule, got {e}"),
            Ok(_) => panic!("want NoModule, bundle unexpectedly loaded"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_library_without_factory_is_no_factory() {
        // Genuine dlopen round-trip against a system library that is
        // definitely not a VST3 module: open succeeds, the
        // GetPluginFactory lookup fails as a structured error.
        let candidates: &[&str] = if cfg!(target_os = "macos") {
            &["/usr/lib/libSystem.B.dylib", "/usr/lib/libc.dylib"]
        } else if cfg!(target_os = "windows") {
            &["C:\\Windows\\System32\\kernel32.dll"]
        } else {
            &[
                "/lib/x86_64-linux-gnu/libc.so.6",
                "/usr/lib/x86_64-linux-gnu/libc.so.6",
                "/lib64/libc.so.6",
            ]
        };
        let path = candidates
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file());
        let Some(path) = path else { return };
        match Vst3Host::load_module(&path) {
            Err(Vst3Error::NoFactory(_)) => {}
            other => panic!("want NoFactory, got {other:?}"),
        }
    }

    #[test]
    fn params_clamp_and_unknown_is_bad_param() {
        let mut plugin = NullVst3Plugin::new("Null");
        plugin.set_param("gain", 99.0).expect("set");
        assert_eq!(plugin.param("gain"), Some(4.0));
        plugin.set_param("gain", -99.0).expect("set");
        assert_eq!(plugin.param("gain"), Some(0.0));
        match plugin.set_param("resonance", 1.0) {
            Err(Vst3Error::BadParam(id)) => assert_eq!(id, "resonance"),
            other => panic!("want BadParam, got {other:?}"),
        }
    }

    #[test]
    fn block_shape_violations_are_process_errors() {
        let mut plugin = NullVst3Plugin::new("Null");
        let ch = vec![0.0f32; 8];
        let mut out = vec![0.0f32; 8];
        assert!(plugin.process(&[], &mut [out.as_mut_slice()]).is_err());
        assert!(
            plugin
                .process(&[ch.as_slice()], &mut [out.as_mut_slice()])
                .is_ok()
        );
        let short = vec![0.0f32; 4];
        let mut out_b = vec![0.0f32; 8];
        assert!(
            plugin
                .process(
                    &[ch.as_slice(), short.as_slice()],
                    &mut [out.as_mut_slice(), out_b.as_mut_slice()]
                )
                .is_err()
        );
    }

    #[test]
    fn null_plugin_passes_silence() {
        let mut plugin = NullVst3Plugin::new("Null");
        let (left, right) = stereo_block(0.0, 32);
        let mut out_l = vec![9.0f32; 32];
        let mut out_r = vec![9.0f32; 32];
        plugin
            .process(
                &[left.as_slice(), right.as_slice()],
                &mut [out_l.as_mut_slice(), out_r.as_mut_slice()],
            )
            .expect("process");
        assert!(out_l.iter().all(|&s| s == 0.0));
        assert!(out_r.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn device_node_mapping_uses_frozen_device_kind() {
        let plugin = NullVst3Plugin::new("Null");
        let node = plugin.as_device_node("dev_vst3_null");
        assert_eq!(node.kind, NodeKind::Device);
        assert_eq!(node.id, "dev_vst3_null");
        assert_eq!(node.params, plugin.params());
        // Round-trips through the frozen project JSON untouched.
        let json = serde_json::to_string(&node).expect("json");
        let back: Node = serde_json::from_str(&json).expect("back");
        assert_eq!(back.kind, NodeKind::Device);
    }
}
