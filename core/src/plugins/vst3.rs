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
//!    [`Vst3Instance`]. v1 loads the `descriptor.json` dev/test format
//!    in-process; native binaries go through [`Vst3Host::load_module`],
//!    which really `dlopen`s the module and enumerates its factory classes.
//! 3. **Running** ([`Vst3Instance`]): the host-side plugin contract —
//!    params as frozen [`Param`](crate::model::Param)s, block processing,
//!    reset. [`DescriptorPlugin`] and [`NullVst3Plugin`] implement it
//!    in-process; real binary instances are the phase-2 follow-up.
//!
//! Frozen-contract rule: plugin params reuse the frozen `Param` shape and
//! [`Vst3Instance::as_device_node`] maps a plugin to a frozen
//! `NodeKind::Device` node, so this track adds no project-schema or IPC
//! surface and the typegen drift gate is unaffected. (`Vst3Descriptor` is
//! deliberately *not* registered in `emit.rs`: it is an unstable dev/test
//! format, never written to project files.)
//!
//! Sandbox seam (shared with the sibling CLAP track): out-of-process
//! isolation is owned by the CLAP agent in `plugins/sandbox.rs` —
//! read-only for this track. [`Vst3Instance`] is `Send` so a boxed instance
//! can move behind that sandbox interface later without changing this
//! file's public behavior; the adapter belongs in `vst3.rs` when the
//! sandbox lands.

use std::ffi::c_void;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
