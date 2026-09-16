//! Track C (agent 3): Audio Unit plugin support — macOS only, behind a flag.
//!
//! New to Audio Units? An AU plugin is a macOS system component (a
//! `.component` bundle) that a host finds through the `AudioComponent`
//! registry and drives with `AudioUnit` calls. Every component is named by
//! three FourCC codes packed as `u32`s: type (`aufx` = effect, `aumu` =
//! instrument), subtype (the specific plugin), manufacturer (`appl` =
//! Apple). This module owns that vocabulary plus the two operations a host
//! needs first:
//!
//! - [`scan_system`]: list installed `.component` bundles. Pure [`std`],
//!   so it compiles and runs everywhere; off macOS the search paths are
//!   empty and it returns no candidates.
//! - [`instantiate`]: turn an [`AuComponentDesc`] into a live [`AuInstance`]
//!   via `AudioComponentFindNext` + `AudioComponentInstanceNew`. macOS
//!   only, and only when the caller passes `enabled = true` —
//!   [`instantiate_if_flag`] wires that to the `CCEZ_ENABLE_AU`
//!   environment flag (off by default). Anything else gets
//!   [`AuError::DisabledByFlag`].
//! - [`AuBackend::load`]: the tested render path — parameter bridging
//!   (list/get/set on the global scope) plus `AudioUnitRender` behind
//!   the sandboxed worker's additive `LoadAu` op. Needs no flag: loading
//!   here is the explicit opt-in. Untested component types refuse with
//!   [`AuError::UnsupportedType`] before anything loads.
//!
//! The hosting-crate evaluation that led to zero new dependencies lives in
//! `docs/notes/track-c.md` (§ AU hosting-crate evaluation).
//!
//! Reads no frozen types and adds no IPC or project-schema surface, so the
//! typegen drift gate is unaffected.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Pack four ASCII bytes into an `OSType` component code (`aufx`).
pub const fn fourcc(bytes: [u8; 4]) -> u32 {
    u32::from_be_bytes(bytes)
}

/// Unpack an `OSType` back into its four ASCII bytes.
pub const fn fourcc_bytes(code: u32) -> [u8; 4] {
    code.to_be_bytes()
}

/// Lossy display form of a FourCC code (`0x61756678` → `"aufx"`).
/// Non-printable bytes render as `?` so logs never emit control codes.
pub fn fourcc_string(code: u32) -> String {
    fourcc_bytes(code)
        .iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// `kAudioUnitType_Output` (`auou`): hardware I/O units.
pub const AU_TYPE_OUTPUT: u32 = fourcc([b'a', b'u', b'o', b'u']);
/// `kAudioUnitType_MusicDevice` (`aumu`): software instruments.
pub const AU_TYPE_MUSIC_DEVICE: u32 = fourcc([b'a', b'u', b'm', b'u']);
/// `kAudioUnitType_Effect` (`aufx`): filters, dynamics, delays, …
pub const AU_TYPE_EFFECT: u32 = fourcc([b'a', b'u', b'f', b'x']);
/// `kAudioUnitType_Mixer` (`aumx`): mixers and splitters.
pub const AU_TYPE_MIXER: u32 = fourcc([b'a', b'u', b'm', b'x']);
/// `kAudioUnitType_Panner` (`aupn`): panners and spatializers.
pub const AU_TYPE_PANNER: u32 = fourcc([b'a', b'u', b'p', b'n']);
/// `kAudioUnitType_Generator` (`augn`): signal sources without input.
pub const AU_TYPE_GENERATOR: u32 = fourcc([b'a', b'u', b'g', b'n']);
/// `kAudioUnitType_FormatConverter` (`aufc`): sample-rate/format bridges.
pub const AU_TYPE_FORMAT_CONVERTER: u32 = fourcc([b'a', b'u', b'f', b'c']);
/// Apple-as-manufacturer (`appl`): the units macOS ships with.
pub const AU_MANUFACTURER_APPLE: u32 = fourcc([b'a', b'p', b'p', b'l']);

/// Environment flag that opts into AU instantiation. Any of
/// `1` / `true` / `yes` / `on` (case-insensitive); unset or anything else
/// means deferred. Named after the feature, not the format version, so
/// AUv2 and AUv3 share one gate.
pub const AU_ENABLE_ENV: &str = "CCEZ_ENABLE_AU";

/// The three FourCC codes that name one Audio Unit component. A zero field
/// is a wildcard for `AudioComponentFindNext`; [`AuComponentDesc::any`]
/// wildcards all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuComponentDesc {
    /// Component type (`aufx`, `aumu`, …) or 0 for any.
    pub component_type: u32,
    /// Component subtype (the specific plugin) or 0 for any.
    pub component_subtype: u32,
    /// Component manufacturer (`appl`, …) or 0 for any.
    pub manufacturer: u32,
}

impl AuComponentDesc {
    /// New descriptor from explicit codes.
    pub const fn new(component_type: u32, component_subtype: u32, manufacturer: u32) -> Self {
        Self {
            component_type,
            component_subtype,
            manufacturer,
        }
    }

    /// Match the first registered component of any kind.
    pub const fn any() -> Self {
        Self::new(0, 0, 0)
    }

    /// One of Apple's bundled units, e.g. `apple(AU_TYPE_EFFECT, subtype)`.
    pub const fn apple(component_type: u32, component_subtype: u32) -> Self {
        Self::new(component_type, component_subtype, AU_MANUFACTURER_APPLE)
    }
}

impl fmt::Display for AuComponentDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            fourcc_string(self.component_type),
            fourcc_string(self.component_subtype),
            fourcc_string(self.manufacturer)
        )
    }
}

/// One installed `.component` bundle found by [`scan_system`] / [`scan_dir`].
/// `desc` is `None` until load time: bundle filenames don't carry FourCC
/// codes, so full identity resolves through the `AudioComponent` registry
/// in [`instantiate`], not from the directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuPluginInfo {
    /// Display name (`CFBundleName` from `Info.plist`, else bundle stem).
    pub name: String,
    /// Path to the `.component` bundle directory.
    pub bundle_path: PathBuf,
    /// Resolved FourCC identity; `None` until instantiated.
    pub desc: Option<AuComponentDesc>,
}

/// What can go wrong finding, loading, or driving an Audio Unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuError {
    /// Called on an OS with no `AudioComponent` registry.
    UnsupportedPlatform,
    /// The AU flag is off: v1 defers AU loading by design, not by failure.
    DisabledByFlag { flag: &'static str },
    /// No registered component matches the descriptor.
    NotFound { desc: AuComponentDesc },
    /// `AudioComponentInstanceNew` refused (`status` is the `OSStatus`).
    InstantiateFailed { status: i32 },
    /// A bundle directory couldn't be read while scanning.
    Io(String),
    /// The component type is outside the v1 render scope (only effects,
    /// instruments, and generators render; output/mixer/panner/converter
    /// topologies need bus negotiation v1 does not do). A refusal, never
    /// silent misrouting. Checked before touching the registry.
    UnsupportedType { component_type: u32 },
    /// Unknown parameter id on `set_param` / `get_param` (never sent).
    BadParam(String),
    /// `AudioUnitInitialize` refused (`status` is the `OSStatus`).
    InitFailed { status: i32 },
    /// Stream-format negotiation refused (`status` is the `OSStatus`).
    FormatFailed { status: i32 },
    /// A parameter get/set call refused (`status` is the `OSStatus`).
    ParamFailed { status: i32 },
    /// `AudioUnitRender` refused (`status` is the `OSStatus`).
    RenderFailed { status: i32 },
}

impl fmt::Display for AuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                write!(f, "Audio Units exist only on macOS")
            }
            Self::DisabledByFlag { flag } => write!(
                f,
                "AU loading is deferred behind the {flag} flag (CLAP/VST3 ship first)"
            ),
            Self::NotFound { desc } => write!(f, "no Audio Unit matches {desc}"),
            Self::InstantiateFailed { status } => {
                write!(f, "AudioComponentInstanceNew failed (OSStatus {status})")
            }
            Self::Io(msg) => write!(f, "AU scan I/O: {msg}"),
            Self::UnsupportedType { component_type } => write!(
                f,
                "AU type `{}` is outside the v1 render scope (effects, instruments, generators)",
                fourcc_string(*component_type)
            ),
            Self::BadParam(id) => write!(f, "unknown AU param `{id}`"),
            Self::InitFailed { status } => {
                write!(f, "AudioUnitInitialize failed (OSStatus {status})")
            }
            Self::FormatFailed { status } => {
                write!(f, "AU stream format not accepted (OSStatus {status})")
            }
            Self::ParamFailed { status } => {
                write!(f, "AU parameter call failed (OSStatus {status})")
            }
            Self::RenderFailed { status } => {
                write!(f, "AudioUnitRender failed (OSStatus {status})")
            }
        }
    }
}

impl std::error::Error for AuError {}

/// Parse the `CCEZ_ENABLE_AU` flag value. Split out for tests so no test
/// has to mutate the shared process environment.
fn flag_value(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Whether AU instantiation is opted in on this process. Always `false`
/// off macOS; on macOS it follows [`AU_ENABLE_ENV`] (default off).
pub fn au_enabled() -> bool {
    cfg!(target_os = "macos")
        && std::env::var(AU_ENABLE_ENV)
            .map(|v| flag_value(&v))
            .unwrap_or(false)
}

/// Directories searched by [`scan_system`]: user, local, then system. The
/// system domain moved across macOS releases, so all three are listed and
/// missing ones are skipped, not errors.
pub fn component_search_paths() -> Vec<PathBuf> {
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
    #[cfg(target_os = "macos")]
    {
        let mut paths = vec![
            PathBuf::from("/Library/Audio/Plug-Ins/Components"),
            PathBuf::from("/System/Library/Components"),
        ];
        if let Ok(home) = std::env::var("HOME") {
            paths.insert(
                0,
                PathBuf::from(home).join("Library/Audio/Plug-Ins/Components"),
            );
        }
        paths
    }
}

/// List the `.component` bundles in one directory. Unreadable directories
/// yield an empty list (a DAW scan must survive one bad folder); entries
/// are sorted by name so scans are deterministic.
pub fn scan_dir(dir: &Path) -> Vec<AuPluginInfo> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut found: Vec<AuPluginInfo> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("component"))
                && path.is_dir()
        })
        .map(|bundle_path| {
            let name = bundle_display_name(&bundle_path).unwrap_or_else(|| {
                bundle_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("unknown component")
                    .to_owned()
            });
            AuPluginInfo {
                name,
                bundle_path,
                desc: None,
            }
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// List installed AU bundles across [`component_search_paths`].
pub fn scan_system() -> Vec<AuPluginInfo> {
    component_search_paths()
        .iter()
        .flat_map(|dir| scan_dir(dir))
        .collect()
}

/// Best-effort display name from `<bundle>/Contents/Info.plist`:
/// `CFBundleDisplayName`, else `CFBundleName`. This is a targeted
/// key/string extractor, not a plist parser — it only understands the
/// `<key>NAME</key><string>VALUE</string>` shape Apple writes, and bails
/// (`None`) on anything else so the caller falls back to the file stem.
fn bundle_display_name(bundle: &Path) -> Option<String> {
    const MAX_PLIST_BYTES: u64 = 1 << 20;
    let plist_path = bundle.join("Contents/Info.plist");
    let meta = std::fs::metadata(&plist_path).ok()?;
    if meta.len() > MAX_PLIST_BYTES || !meta.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&plist_path).ok()?;
    plist_string_for_key(&text, "CFBundleDisplayName")
        .or_else(|| plist_string_for_key(&text, "CFBundleName"))
}

/// Extract `<string>…</string>` following `<key>{key}</key>` in plist XML.
fn plist_string_for_key(plist: &str, key: &str) -> Option<String> {
    let open_key = format!("<key>{key}</key>");
    let at = plist.find(&open_key)? + open_key.len();
    let after = &plist[at..];
    let value_at = after.find("<string>")? + "<string>".len();
    let end = after[value_at..].find("</string>")?;
    let value = after[value_at..value_at + end].trim();
    if value.is_empty() || value.contains('<') {
        return None;
    }
    Some(
        value
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'"),
    )
}

/// Minimal `AudioComponent` FFI: find + create + destroy, plus the
/// parameter/render calls the bridging layer needs. Deliberately *not* a
/// new crate dependency (see the evaluation in `track-c.md`): one
/// `#[link]` to the `AudioToolbox` framework the OS always ships covers
/// the whole seam (plus `CoreFoundation` for parameter display names).
#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::{c_char, c_void};

    /// Mirrors `AudioComponentDescription` field-for-field (`UInt32` × 5).
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct AudioComponentDescription {
        pub component_type: u32,
        pub component_subtype: u32,
        pub component_manufacturer: u32,
        pub component_flags: u32,
        pub component_flags_mask: u32,
    }

    pub type AudioComponent = *mut c_void;
    pub type AudioComponentInstance = *mut c_void;

    /// Mirrors `AudioStreamBasicDescription`: `Float64` + `UInt32` × 8
    /// (40 bytes). Only the canonical mono `Float32` shape below is ever
    /// written; anything read back is inspected, never trusted.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct AudioStreamBasicDescription {
        pub sample_rate: f64,
        pub format_id: u32,
        pub format_flags: u32,
        pub bytes_per_packet: u32,
        pub frames_per_packet: u32,
        pub bytes_per_frame: u32,
        pub channels_per_frame: u32,
        pub bits_per_channel: u32,
        pub reserved: u32,
    }

    /// Mirrors `AudioBuffer`: two `UInt32`s + one pointer.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct AudioBuffer {
        pub channels: u32,
        pub byte_size: u32,
        pub data: *mut c_void,
    }

    /// The mono case of the variable-length `AudioBufferList`: one count
    /// + one inline `AudioBuffer` (24 bytes). v1 renders mono only, like
    /// the CLAP/VST3 backends.
    #[repr(C)]
    #[derive(Debug)]
    pub struct AudioBufferListMono {
        pub num_buffers: u32,
        pub buffer: AudioBuffer,
    }

    /// Mirrors `SMPTETime` (24 bytes); only carried, never interpreted.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct SmpteTime {
        pub subframes: i16,
        pub subframe_divisor: i16,
        pub counter: u32,
        pub typ: u32,
        pub flags: u32,
        pub hours: i16,
        pub minutes: i16,
        pub seconds: i16,
        pub frames: i16,
    }

    /// Mirrors `AudioTimeStamp` (64 bytes). Renders stamp
    /// `mSampleTime` only (`kAudioTimeStampSampleTimeValid`); every other
    /// representation stays zero.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct AudioTimeStamp {
        pub sample_time: f64,
        pub host_time: u64,
        pub rate_scalar: f64,
        pub word_clock: u64,
        pub smpte: SmpteTime,
        pub flags: u32,
        pub reserved: u32,
    }

    /// Mirrors `AudioUnitParameterInfo` (104 bytes with alignment).
    /// `name` is UNUSED by contract (Apple fills `cf_name` instead);
    /// cf-strings that arrive with `CFNameRelease` set are released
    /// after reading.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct AudioUnitParameterInfoRaw {
        pub name: [c_char; 52],
        pub unit_name: *const c_void,
        pub clump_id: u32,
        pub cf_name: *const c_void,
        pub unit: u32,
        pub min_value: f32,
        pub max_value: f32,
        pub default_value: f32,
        pub flags: u32,
    }

    /// Mirrors `AURenderCallbackStruct`: the input-pull callback effects
    /// use to fetch their source audio during `AudioUnitRender`.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct AuRenderCallback {
        pub input_proc: Option<
            unsafe extern "C" fn(
                *mut c_void,
                *mut u32,
                *const AudioTimeStamp,
                u32,
                u32,
                *mut AudioBufferListMono,
            ) -> i32,
        >,
        pub input_proc_ref_con: *mut c_void,
    }

    #[link(name = "AudioToolbox", kind = "framework")]
    extern "C" {
        pub fn AudioComponentFindNext(
            in_component: AudioComponent,
            desc: *const AudioComponentDescription,
        ) -> AudioComponent;
        pub fn AudioComponentInstanceNew(
            in_component: AudioComponent,
            out_instance: *mut AudioComponentInstance,
        ) -> i32;
        pub fn AudioComponentInstanceDispose(in_instance: AudioComponentInstance) -> i32;
        pub fn AudioUnitInitialize(in_unit: AudioComponentInstance) -> i32;
        pub fn AudioUnitUninitialize(in_unit: AudioComponentInstance) -> i32;
        pub fn AudioUnitGetPropertyInfo(
            in_unit: AudioComponentInstance,
            in_id: u32,
            in_scope: u32,
            in_element: u32,
            out_data_size: *mut u32,
            out_writable: *mut u8,
        ) -> i32;
        pub fn AudioUnitGetProperty(
            in_unit: AudioComponentInstance,
            in_id: u32,
            in_scope: u32,
            in_element: u32,
            out_data: *mut c_void,
            io_data_size: *mut u32,
        ) -> i32;
        pub fn AudioUnitSetProperty(
            in_unit: AudioComponentInstance,
            in_id: u32,
            in_scope: u32,
            in_element: u32,
            in_data: *const c_void,
            in_data_size: u32,
        ) -> i32;
        pub fn AudioUnitGetParameter(
            in_unit: AudioComponentInstance,
            in_id: u32,
            in_scope: u32,
            in_element: u32,
            out_value: *mut f32,
        ) -> i32;
        pub fn AudioUnitSetParameter(
            in_unit: AudioComponentInstance,
            in_id: u32,
            in_scope: u32,
            in_element: u32,
            in_value: f32,
            in_buffer_offset: u32,
        ) -> i32;
        pub fn AudioUnitRender(
            in_unit: AudioComponentInstance,
            io_action_flags: *mut u32,
            in_timestamp: *const AudioTimeStamp,
            in_output_bus: u32,
            in_frames: u32,
            io_data: *mut AudioBufferListMono,
        ) -> i32;
    }

    // `CoreFoundation` string reads for parameter display names
    // (`CFStringRef` -> UTF-8). The framework ships with every macOS.
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFStringGetLength(s: *const c_void) -> isize;
        pub fn CFStringGetCString(
            s: *const c_void,
            buf: *mut c_char,
            buf_len: isize,
            encoding: u32,
        ) -> u8;
        pub fn CFRelease(s: *const c_void);
    }
}

#[cfg(target_os = "macos")]
const _: () = assert!(std::mem::size_of::<ffi::AudioStreamBasicDescription>() == 40);
#[cfg(target_os = "macos")]
const _: () = assert!(std::mem::size_of::<ffi::AudioBufferListMono>() == 24);
#[cfg(target_os = "macos")]
const _: () = assert!(std::mem::size_of::<ffi::AudioTimeStamp>() == 64);
#[cfg(target_os = "macos")]
const _: () = assert!(std::mem::size_of::<ffi::AudioUnitParameterInfoRaw>() == 104);
#[cfg(target_os = "macos")]
const _: () = assert!(std::mem::size_of::<ffi::AuRenderCallback>() == 16);

/// A live Audio Unit instance. Owns its `AudioComponentInstance` and
/// disposes it in [`Drop`]; audio-path calls (`AudioUnitRender`,
/// parameter get/set) are the phase-2 follow-up on this handle.
#[derive(Debug)]
pub struct AuInstance {
    #[cfg(target_os = "macos")]
    handle: *mut std::ffi::c_void,
    desc: AuComponentDesc,
}

// SAFETY: the handle is opaque, owned by this value, used only through
// `&self`, and disposed exactly once in `Drop` — safe to hand across
// threads (the audio thread will own it), but not to share (`!Sync`).
#[cfg(target_os = "macos")]
unsafe impl Send for AuInstance {}

impl AuInstance {
    /// The descriptor this instance was opened from.
    pub fn desc(&self) -> AuComponentDesc {
        self.desc
    }
}

#[cfg(target_os = "macos")]
impl Drop for AuInstance {
    fn drop(&mut self) {
        // SAFETY: `handle` came from a successful `AudioComponentInstanceNew`
        // and `Drop` runs once, so this disposes exactly what we own. The
        // status is unrecoverable during teardown by construction.
        let _ = unsafe { ffi::AudioComponentInstanceDispose(self.handle) };
    }
}

/// Whether any registered component matches `desc`. Pure existence probe:
/// no instance is created. Always `false` off macOS.
pub fn component_exists(desc: &AuComponentDesc) -> bool {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = desc;
        false
    }
    #[cfg(target_os = "macos")]
    {
        use ffi::AudioComponentFindNext;
        use std::ptr::null_mut;
        let raw = ffi::AudioComponentDescription {
            component_type: desc.component_type,
            component_subtype: desc.component_subtype,
            component_manufacturer: desc.manufacturer,
            component_flags: 0,
            component_flags_mask: 0,
        };
        // SAFETY: `FindNext` only reads the descriptor snapshot and walks
        // the registry; null cursor means "start from the first match".
        let found = unsafe { AudioComponentFindNext(null_mut(), &raw) };
        !found.is_null()
    }
}

/// Open the first component matching `desc`. Pass `enabled = true` only
/// behind the [`AU_ENABLE_ENV`] flag (see [`instantiate_if_flag`]); with
/// `enabled = false` this returns [`AuError::DisabledByFlag`] without
/// touching the registry, which is how v1 ships AU as deferred.
pub fn instantiate(desc: &AuComponentDesc, enabled: bool) -> Result<AuInstance, AuError> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (desc, enabled);
        Err(AuError::UnsupportedPlatform)
    }
    #[cfg(target_os = "macos")]
    {
        if !enabled {
            return Err(AuError::DisabledByFlag {
                flag: AU_ENABLE_ENV,
            });
        }
        use ffi::{AudioComponentFindNext, AudioComponentInstanceNew};
        use std::ptr::null_mut;
        let raw = ffi::AudioComponentDescription {
            component_type: desc.component_type,
            component_subtype: desc.component_subtype,
            component_manufacturer: desc.manufacturer,
            component_flags: 0,
            component_flags_mask: 0,
        };
        // SAFETY: as in `component_exists` — registry walk, no writes.
        let component = unsafe { AudioComponentFindNext(null_mut(), &raw) };
        if component.is_null() {
            return Err(AuError::NotFound { desc: *desc });
        }
        let mut instance: ffi::AudioComponentInstance = null_mut();
        // SAFETY: `component` is non-null (checked above) and `instance`
        // points to valid writable stack memory for the out-param.
        let status = unsafe { AudioComponentInstanceNew(component, &mut instance) };
        if status != 0 || instance.is_null() {
            return Err(AuError::InstantiateFailed { status });
        }
        Ok(AuInstance {
            handle: instance,
            desc: *desc,
        })
    }
}

/// [`instantiate`] gated on the live [`AU_ENABLE_ENV`] flag: the one call
/// the future audio path uses, so AU can never load unless the user opted
/// in with `CCEZ_ENABLE_AU=1`.
pub fn instantiate_if_flag(desc: &AuComponentDesc) -> Result<AuInstance, AuError> {
    instantiate(desc, au_enabled())
}

// ---------------------------------------------------------------------------
// Parameter + render bridging: the sandboxed-worker AU backend.
// ---------------------------------------------------------------------------
//
// Teaching note: an Audio Unit renders *pulled*, not pushed. The host
// calls `AudioUnitRender` for N output frames; an effect unit then pulls
// its input through a callback the host registered
// (`kAudioUnitProperty_SetRenderCallback`), while a generator/source
// unit just fills the output. `AuBackend` owns that whole contract —
// find, open, negotiate a mono `Float32` stream format at the session
// rate, initialize, bridge parameters, render mono blocks, report
// latency — so the worker process speaks the same
// init/load/process/params/latency/state shape as the CLAP/VST3
// backends, behind one additive `LoadAu` op.
//
// The render path deliberately does NOT consult [`AU_ENABLE_ENV`]: the
// flag gated v1 *discovery* while render was untested; now that
// parameter + render bridging is integrated and covered, loading through
// [`AuBackend::load`] is the opt-in. The flag still gates the raw
// [`instantiate_if_flag`] seam, whose deferred-by-design contract (and
// tests) are untouched.
//
// v1 scope, honestly documented (mirrors the CLAP/VST3 v1 limits):
//
// - Mono only: one `Float32` channel at the session rate on bus 0. A unit
//   that refuses the canonical format fails [`AuBackend::load`] with
//   [`AuError::FormatFailed`] instead of rendering wrong audio.
// - Only effects (`aufx`), instruments (`aumu`), and generators (`augn`)
//   load. Output, mixer, panner, converter, and music-effect topologies
//   fail fast with [`AuError::UnsupportedType`] — checked before the
//   registry is touched, on every platform.
// - Params travel as immediate `AudioUnitSetParameter` calls on the
//   global scope (worker ids match unit names case-insensitively, or a
//   bare parameter number; values clamp to the unit's `[min, max]`, the
//   same rule the engine's `ParamSet` applies). Unknown ids are kept in
//   the host-side snapshot but never sent — snapshots stay exact.
// - The opaque `blob` is host-side passthrough, exactly like the mock:
//   AU class-info/preset serialization is the follow-up, not this track.
// - Latency reads the unit's `kAudioUnitProperty_Latency` seconds live
//   (0 when the unit exposes none) into [`LatencyMap`](super::latency::LatencyMap).

use std::collections::BTreeMap;

use super::host::PluginState;

/// `true` when `desc` names a component type the v1 render path drives.
/// Pure FourCC check: no registry, no flag, same answer everywhere.
pub fn au_render_supported(desc: &AuComponentDesc) -> bool {
    matches!(
        desc.component_type,
        AU_TYPE_EFFECT | AU_TYPE_MUSIC_DEVICE | AU_TYPE_GENERATOR
    )
}

/// One bridged parameter: the stable numeric AU id plus the worker-side
/// name, range, and default the host clamps values into.
#[derive(Debug, Clone)]
pub struct AuParamSlot {
    /// `AudioUnitParameterID` on the global scope, element 0.
    pub id: u32,
    /// Display name (`cfNameString`, else `param{id}`); worker id.
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

/// A live Audio Unit running inside the worker process.
///
/// Owns its [`AuInstance`] (disposed on drop, after
/// `AudioUnitUninitialize`). `Send` so the worker loop can own it like
/// every other backend; never shared (`!Sync`, like the handle inside).
#[derive(Debug)]
pub struct AuBackend {
    desc: AuComponentDesc,
    sample_rate: f64,
    /// Canonical worker-side values by slot name, plus any unknown ids
    /// the worker was told (kept for snapshot exactness, never sent).
    params: BTreeMap<String, f64>,
    blob: Vec<u8>,
    slots: Vec<AuParamSlot>,
    #[cfg(target_os = "macos")]
    unit: AuInstance,
    /// Next render's `mSampleTime`; the stamp must advance monotonically
    /// or the unit infers a timeline discontinuity.
    #[cfg(target_os = "macos")]
    sample_time: f64,
    /// Effects pull input through the render callback; sources render
    /// without one.
    #[cfg(target_os = "macos")]
    needs_input: bool,
}

#[cfg(target_os = "macos")]
// SAFETY: same ownership story as `AuInstance` — the unit is owned by
// this value, driven only through `&mut self` on the worker thread, and
// torn down exactly once in `Drop`.
unsafe impl Send for AuBackend {}

/// Largest single `AudioUnitRender` call. Bigger worker blocks are
/// chunked (and must fit `MaximumFramesPerSlice`, set at load);
/// smaller ones pass through untouched.
const AU_MAX_BLOCK_FRAMES: usize = 1024;

// AudioToolbox constants, pinned to the SDK values inspected in
// `AudioUnitProperties.h` / `CoreAudioBaseTypes.h` (macOS 26 SDK).
#[cfg(target_os = "macos")]
mod au_const {
    pub const PROP_PARAMETER_LIST: u32 = 3;
    pub const PROP_PARAMETER_INFO: u32 = 4;
    pub const PROP_STREAM_FORMAT: u32 = 8;
    pub const PROP_LATENCY: u32 = 12;
    pub const PROP_MAX_FRAMES_PER_SLICE: u32 = 14;
    pub const PROP_SET_RENDER_CALLBACK: u32 = 23;
    pub const SCOPE_GLOBAL: u32 = 0;
    pub const SCOPE_INPUT: u32 = 1;
    pub const SCOPE_OUTPUT: u32 = 2;
    /// `'lpcm'`.
    pub const FORMAT_LINEAR_PCM: u32 = 0x6c70_636d;
    /// `Float | NativeEndian | Packed | NonInterleaved` for mono `F32`.
    pub const FORMAT_FLAGS_MONO_F32: u32 = 0x2b;
    pub const TIMESTAMP_SAMPLE_VALID: u32 = 1;
    /// `kCFStringEncodingUTF8`.
    pub const CF_UTF8: u32 = 0x0800_0100;
    /// `kAudioUnitParameterFlag_CFNameRelease` (bit 4).
    pub const PARAM_FLAG_CF_RELEASE: u32 = 1 << 4;
}

#[cfg(target_os = "macos")]
fn canonical_mono_asbd(sample_rate: f64) -> ffi::AudioStreamBasicDescription {
    ffi::AudioStreamBasicDescription {
        sample_rate,
        format_id: au_const::FORMAT_LINEAR_PCM,
        format_flags: au_const::FORMAT_FLAGS_MONO_F32,
        bytes_per_packet: 4,
        frames_per_packet: 1,
        bytes_per_frame: 4,
        channels_per_frame: 1,
        bits_per_channel: 32,
        reserved: 0,
    }
}

/// Negotiate the v1 mono `Float32` format on one bus: patch the unit's
/// own current format (rate + mono channel counts, keeping its flags —
/// the flag combination a unit accepts is its business) and fall back
/// to the canonical shape only when the unit reports no format at all.
/// Anything the unit refuses is [`AuError::FormatFailed`], never silent
/// misrouting.
#[cfg(target_os = "macos")]
fn negotiate_mono_format(
    handle: *mut std::ffi::c_void,
    scope: u32,
    sample_rate: f64,
) -> Result<(), AuError> {
    if let Ok(current) = au_get::<ffi::AudioStreamBasicDescription>(
        handle,
        au_const::PROP_STREAM_FORMAT,
        scope,
        0,
    ) {
        let mut patched = current;
        patched.sample_rate = sample_rate;
        patched.channels_per_frame = 1;
        patched.frames_per_packet = 1;
        patched.bits_per_channel = 32;
        patched.bytes_per_frame = 4;
        patched.bytes_per_packet = 4;
        if au_set(handle, au_const::PROP_STREAM_FORMAT, scope, 0, &patched).is_ok() {
            return Ok(());
        }
    }
    let canonical = canonical_mono_asbd(sample_rate);
    au_set(handle, au_const::PROP_STREAM_FORMAT, scope, 0, &canonical)
}

#[cfg(target_os = "macos")]
fn au_set_bytes(
    handle: *mut std::ffi::c_void,
    id: u32,
    scope: u32,
    element: u32,
    value: &[u8],
) -> Result<(), AuError> {
    // SAFETY: `handle` is a live initialized-or-configuring unit owned by
    // the backend; `value` borrows valid bytes for the call.
    let status = unsafe {
        ffi::AudioUnitSetProperty(
            handle,
            id,
            scope,
            element,
            value.as_ptr() as *const std::ffi::c_void,
            value.len() as u32,
        )
    };
    if status != 0 {
        return Err(AuError::FormatFailed { status });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn au_set<T>(handle: *mut std::ffi::c_void, id: u32, scope: u32, element: u32, value: &T) -> Result<(), AuError> {
    // SAFETY: `T` is always a plain-data FFI struct; its bytes are the
    // property payload by CoreAudio contract.
    let bytes = unsafe {
        std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>())
    };
    au_set_bytes(handle, id, scope, element, bytes)
}

#[cfg(target_os = "macos")]
fn au_get<T: Default>(
    handle: *mut std::ffi::c_void,
    id: u32,
    scope: u32,
    element: u32,
) -> Result<T, i32> {
    let mut value = T::default();
    // Default-derived zeros are the honest empty payload: every `T`
    // used here is plain data (floats, ints, FFI structs).
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: as in `au_set` — `handle` is live, `value` is valid
    // writable memory of exactly `size` bytes.
    let status = unsafe {
        ffi::AudioUnitGetProperty(
            handle,
            id,
            scope,
            element,
            &mut value as *mut T as *mut std::ffi::c_void,
            &mut size,
        )
    };
    if status != 0 || size as usize != std::mem::size_of::<T>() {
        return Err(if status != 0 { status } else { -1 });
    }
    Ok(value)
}

/// Read a `CFStringRef` as UTF-8 without ever emitting a control code.
/// Anything unreadable (or absurdly long) is `None` so the caller falls
/// back to the stable `param{id}` worker id.
#[cfg(target_os = "macos")]
fn cf_to_string(ptr: *const std::ffi::c_void) -> Option<String> {
    use std::ffi::c_char;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `ptr` came from the unit's parameter info on this thread.
    let units = unsafe { ffi::CFStringGetLength(ptr) };
    if units <= 0 || units > 256 {
        return None;
    }
    let cap = (units as usize + 1) * 4;
    let mut buf = vec![0 as c_char; cap];
    // SAFETY: `buf` is valid writable memory of `cap` bytes.
    let ok = unsafe { ffi::CFStringGetCString(ptr, buf.as_mut_ptr(), cap as isize, au_const::CF_UTF8) };
    if ok == 0 {
        return None;
    }
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as u8)
        .collect();
    String::from_utf8(bytes).ok().filter(|s| !s.is_empty())
}

/// The input-pull callback behind effect renders: copies the current
/// chunk into the unit's fetch buffer, zero-padding a short tail. Runs
/// synchronously inside `AudioUnitRender` on the worker thread; the
/// context borrows the caller's chunk and never escapes the call.
#[cfg(target_os = "macos")]
struct RenderInput<'a> {
    frames: &'a [f32],
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn au_input_proc(
    ref_con: *mut std::ffi::c_void,
    _action_flags: *mut u32,
    _timestamp: *const ffi::AudioTimeStamp,
    _bus: u32,
    n_frames: u32,
    io_data: *mut ffi::AudioBufferListMono,
) -> i32 {
    if ref_con.is_null() || io_data.is_null() {
        return -50;
    }
    // SAFETY: `ref_con` is the `RenderInput` the enclosing `process`
    // call registered one frame up the stack; `io_data` is the unit's
    // valid fetch list for this synchronous call.
    let src = unsafe { &*(ref_con as *const RenderInput<'_>) };
    let dst = unsafe { &mut *io_data };
    if dst.buffer.data.is_null() {
        return -50;
    }
    let want = n_frames as usize;
    let capacity = dst.buffer.byte_size as usize / 4;
    let n = want.min(capacity);
    let out = unsafe { std::slice::from_raw_parts_mut(dst.buffer.data as *mut f32, n) };
    let take = src.frames.len().min(n);
    out[..take].copy_from_slice(&src.frames[..take]);
    for s in &mut out[take..] {
        *s = 0.0;
    }
    dst.buffer.byte_size = (n * 4) as u32;
    0
}

impl AuBackend {
    /// Open `desc`, negotiate mono `Float32` at `sample_rate`, discover
    /// params, and initialize. The render path's opt-in: no
    /// [`AU_ENABLE_ENV`] consult — the integration is tested, so loading
    /// here is explicit, while the raw flag seam keeps its deferred
    /// contract. Untested component types refuse before anything loads.
    pub fn load(desc: &AuComponentDesc, sample_rate: f64) -> Result<Self, AuError> {
        if !au_render_supported(desc) {
            return Err(AuError::UnsupportedType {
                component_type: desc.component_type,
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = sample_rate;
            Err(AuError::UnsupportedPlatform)
        }
        #[cfg(target_os = "macos")]
        {
            Self::load_macos(desc, sample_rate)
        }
    }

    #[cfg(target_os = "macos")]
    fn load_macos(desc: &AuComponentDesc, sample_rate: f64) -> Result<Self, AuError> {
        // `enabled = true`: this IS the tested render path opting in, not
        // the deferred raw seam (`instantiate_if_flag` keeps the flag).
        let unit = instantiate(desc, true)?;
        let handle = unit.handle;
        let needs_input = desc.component_type == AU_TYPE_EFFECT;

        let max_frames: u32 = AU_MAX_BLOCK_FRAMES as u32;
        au_set(
            handle,
            au_const::PROP_MAX_FRAMES_PER_SLICE,
            au_const::SCOPE_GLOBAL,
            0,
            &max_frames,
        )?;

        negotiate_mono_format(handle, au_const::SCOPE_OUTPUT, sample_rate)?;
        if needs_input {
            negotiate_mono_format(handle, au_const::SCOPE_INPUT, sample_rate)?;
        }

        // SAFETY: configured, owned, and not yet initialized — exactly
        // what `AudioUnitInitialize` expects.
        let status = unsafe { ffi::AudioUnitInitialize(handle) };
        if status != 0 {
            return Err(AuError::InitFailed { status });
        }

        let mut backend = Self {
            desc: *desc,
            sample_rate,
            params: BTreeMap::new(),
            blob: Vec::new(),
            slots: Vec::new(),
            unit,
            sample_time: 0.0,
            needs_input,
        };
        backend.discover_params();
        Ok(backend)
    }

    /// Read the unit's parameter list into [`AuParamSlot`]s, seeding
    /// worker-side values from live reads (default on refusal). A unit
    /// with no list simply has no automatable params — not an error.
    #[cfg(target_os = "macos")]
    fn discover_params(&mut self) {
        use au_const::{PROP_PARAMETER_INFO, PROP_PARAMETER_LIST, SCOPE_GLOBAL};
        let handle = self.unit.handle;
        // SAFETY: `GetPropertyInfo` only writes the size out-param.
        let mut size: u32 = 0;
        let status = unsafe {
            ffi::AudioUnitGetPropertyInfo(handle, PROP_PARAMETER_LIST, SCOPE_GLOBAL, 0, &mut size, std::ptr::null_mut())
        };
        if status != 0 || size == 0 || size % 4 != 0 {
            return;
        }
        let count = (size / 4).min(1024) as usize;
        let mut ids = vec![0u32; count];
        let mut bytes = (count * 4) as u32;
        // SAFETY: `ids` is valid writable memory of `bytes` bytes.
        let status = unsafe {
            ffi::AudioUnitGetProperty(
                handle,
                PROP_PARAMETER_LIST,
                SCOPE_GLOBAL,
                0,
                ids.as_mut_ptr() as *mut std::ffi::c_void,
                &mut bytes,
            )
        };
        if status != 0 {
            return;
        }
        ids.truncate(bytes as usize / 4);
        for id in ids {
            let mut info_size = std::mem::size_of::<ffi::AudioUnitParameterInfoRaw>() as u32;
            let mut info: ffi::AudioUnitParameterInfoRaw = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is a valid zeroed FFI struct of `info_size`.
            let status = unsafe {
                ffi::AudioUnitGetProperty(
                    handle,
                    PROP_PARAMETER_INFO,
                    SCOPE_GLOBAL,
                    id,
                    &mut info as *mut _ as *mut std::ffi::c_void,
                    &mut info_size,
                )
            };
            if status != 0 {
                continue;
            }
            let name = cf_to_string(info.cf_name)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("param{id}"));
            // A hostile descriptor (empty/inverted range) is skipped, the
            // way the VST3 backend refuses it — never clamped into NaN.
            if !(info.min_value < info.max_value) || !info.default_value.is_finite() {
                Self::release_param_strings(&info);
                continue;
            }
            let mut live: f32 = 0.0;
            // SAFETY: `live` is valid writable `f32` memory.
            let seed = if unsafe { ffi::AudioUnitGetParameter(handle, id, SCOPE_GLOBAL, 0, &mut live) } == 0
                && live.is_finite()
            {
                live as f64
            } else {
                info.default_value as f64
            };
            self.params.insert(
                name.clone(),
                seed.clamp(info.min_value as f64, info.max_value as f64),
            );
            self.slots.push(AuParamSlot {
                id,
                name,
                min: info.min_value as f64,
                max: info.max_value as f64,
                default: info.default_value as f64,
            });
            Self::release_param_strings(&info);
        }
        self.slots.sort_by(|a, b| a.id.cmp(&b.id));
    }

    #[cfg(target_os = "macos")]
    fn release_param_strings(info: &ffi::AudioUnitParameterInfoRaw) {
        // SAFETY: release exactly the +1 refs the unit handed us (the
        // flag governs both strings by contract); nulls are skipped.
        if info.flags & au_const::PARAM_FLAG_CF_RELEASE != 0 {
            unsafe {
                if !info.cf_name.is_null() {
                    ffi::CFRelease(info.cf_name);
                }
                if !info.unit_name.is_null() {
                    ffi::CFRelease(info.unit_name);
                }
            }
        }
    }

    pub fn desc(&self) -> AuComponentDesc {
        self.desc
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Bridged parameter table (stable id order). Empty on units with no
    /// automatable params — and off macOS, where loading never succeeds.
    pub fn param_list(&self) -> &[AuParamSlot] {
        &self.slots
    }

    /// Set one param by worker id (matches unit names case-insensitively,
    /// or a bare parameter number; unknown ids are an error, never sent).
    pub fn set_param(&mut self, id: &str, value: f64) -> Result<(), AuError> {
        let slot = find_au_slot(&self.slots, id).ok_or_else(|| AuError::BadParam(id.to_string()))?;
        let clamped = value.clamp(slot.min, slot.max);
        #[cfg(target_os = "macos")]
        {
            // SAFETY: owned live unit; immediate set on the global scope.
            let status = unsafe {
                ffi::AudioUnitSetParameter(
                    self.unit.handle,
                    slot.id,
                    au_const::SCOPE_GLOBAL,
                    0,
                    clamped as f32,
                    0,
                )
            };
            if status != 0 {
                return Err(AuError::ParamFailed { status });
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Err(AuError::UnsupportedPlatform);
        }
        #[cfg(target_os = "macos")]
        {
            self.params.insert(slot.name.clone(), clamped);
            Ok(())
        }
    }

    /// Read one param back from the unit (worker-id rules as
    /// [`AuBackend::set_param`]).
    pub fn get_param(&self, id: &str) -> Result<f64, AuError> {
        let slot = find_au_slot(&self.slots, id).ok_or_else(|| AuError::BadParam(id.to_string()))?;
        #[cfg(not(target_os = "macos"))]
        {
            let _ = slot;
            Err(AuError::UnsupportedPlatform)
        }
        #[cfg(target_os = "macos")]
        {
            let mut live: f32 = 0.0;
            // SAFETY: `live` is valid writable `f32` memory.
            let status = unsafe {
                ffi::AudioUnitGetParameter(self.unit.handle, slot.id, au_const::SCOPE_GLOBAL, 0, &mut live)
            };
            if status != 0 {
                return Err(AuError::ParamFailed { status });
            }
            Ok(live as f64)
        }
    }

    /// Render one mono block. Effects pull `input` through the
    /// input callback; sources ignore it. Empty input renders empty
    /// (a zero-frame `AudioUnitRender` is a refusal, not silence).
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, AuError> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = input;
            Err(AuError::UnsupportedPlatform)
        }
        #[cfg(target_os = "macos")]
        {
            let mut out = Vec::with_capacity(input.len());
            for chunk in input.chunks(AU_MAX_BLOCK_FRAMES) {
                if chunk.is_empty() {
                    continue;
                }
                out.extend(self.render_chunk(chunk)?);
            }
            Ok(out)
        }
    }

    #[cfg(target_os = "macos")]
    fn render_chunk(&mut self, chunk: &[f32]) -> Result<Vec<f32>, AuError> {
        let frames = chunk.len() as u32;
        if self.needs_input {
            let ctx = RenderInput { frames: chunk };
            let callback = ffi::AuRenderCallback {
                input_proc: Some(au_input_proc),
                // SAFETY: `ctx` outlives the synchronous render below;
                // the unit never retains it (re-registered every chunk).
                input_proc_ref_con: &ctx as *const RenderInput<'_> as *mut std::ffi::c_void,
            };
            // A stale callback must never survive the chunk whose stack
            // it borrows — registration is per-chunk by construction.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    &callback as *const ffi::AuRenderCallback as *const u8,
                    std::mem::size_of::<ffi::AuRenderCallback>(),
                )
            };
            // SAFETY: `handle` is live and initialized; `bytes` borrows
            // the valid callback struct for the call.
            let status = unsafe {
                ffi::AudioUnitSetProperty(
                    self.unit.handle,
                    au_const::PROP_SET_RENDER_CALLBACK,
                    au_const::SCOPE_INPUT,
                    0,
                    bytes.as_ptr() as *const std::ffi::c_void,
                    bytes.len() as u32,
                )
            };
            if status != 0 {
                return Err(AuError::RenderFailed { status });
            }
        }
        let mut output = vec![0.0f32; chunk.len()];
        let mut list = ffi::AudioBufferListMono {
            num_buffers: 1,
            buffer: ffi::AudioBuffer {
                channels: 1,
                byte_size: (chunk.len() * 4) as u32,
                data: output.as_mut_ptr() as *mut std::ffi::c_void,
            },
        };
        let stamp = ffi::AudioTimeStamp {
            sample_time: self.sample_time,
            host_time: 0,
            rate_scalar: 0.0,
            word_clock: 0,
            smpte: ffi::SmpteTime::default(),
            flags: au_const::TIMESTAMP_SAMPLE_VALID,
            reserved: 0,
        };
        let mut action_flags: u32 = 0;
        // SAFETY: `handle` is live and initialized; `list` owns a valid
        // `f32` buffer of `frames`; `stamp` is a valid monotonic stamp.
        // `output` is borrowed by the list only for this call.
        let status = unsafe {
            ffi::AudioUnitRender(
                self.unit.handle,
                &mut action_flags,
                &stamp,
                0,
                frames,
                &mut list,
            )
        };
        if status != 0 {
            return Err(AuError::RenderFailed { status });
        }
        self.sample_time += chunk.len() as f64;
        // The unit reports what it wrote via `byte_size`; trust but
        // verify — never hand back more than we allocated.
        let written = (list.buffer.byte_size as usize / 4).min(output.len());
        output.truncate(written);
        Ok(output)
    }

    /// Current processing latency in samples: a live read of the unit's
    /// `kAudioUnitProperty_Latency` seconds at the session rate (0 when
    /// the unit exposes none). Feeds
    /// [`LatencyMap`](super::latency::LatencyMap) via the worker's
    /// `GetLatency` op, like the sibling backends' probes.
    pub fn latency_samples(&self) -> u32 {
        #[cfg(not(target_os = "macos"))]
        {
            0
        }
        #[cfg(target_os = "macos")]
        {
            match au_get::<f64>(
                self.unit.handle,
                au_const::PROP_LATENCY,
                au_const::SCOPE_GLOBAL,
                0,
            ) {
                Ok(secs) if secs.is_finite() && secs > 0.0 => {
                    (secs * self.sample_rate).round().clamp(0.0, u32::MAX as f64) as u32
                }
                _ => 0,
            }
        }
    }

    pub fn state(&self) -> PluginState {
        PluginState::new(self.params.clone(), self.blob.clone())
    }

    /// Push worker-side truth into the backend. Known ids clamp into the
    /// unit's ranges and are sent immediately (AU params persist on the
    /// unit, so no per-block re-send is needed); unknown ids ride along
    /// host-side so snapshots round-trip exactly (the mock's rule). The
    /// host-side copy is adopted regardless, so snapshot exactness never
    /// depends on the unit accepting a value.
    pub fn set_state(&mut self, state: &PluginState) {
        #[cfg(target_os = "macos")]
        {
            for (id, value) in &state.params {
                match find_au_slot(&self.slots, id) {
                    Some(slot) => {
                        let clamped = value.clamp(slot.min, slot.max);
                        let _ = unsafe {
                            ffi::AudioUnitSetParameter(
                                self.unit.handle,
                                slot.id,
                                au_const::SCOPE_GLOBAL,
                                0,
                                clamped as f32,
                                0,
                            )
                        };
                        self.params.insert(slot.name.clone(), clamped);
                    }
                    None => {
                        self.params.insert(id.clone(), *value);
                    }
                }
            }
            self.blob = state.blob.clone();
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.params = state.params.clone();
            self.blob = state.blob.clone();
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for AuBackend {
    fn drop(&mut self) {
        // Uninitialize before the inner `AuInstance` disposes the
        // handle; teardown status is unrecoverable by construction.
        // SAFETY: owned live handle, dropped exactly once.
        let _ = unsafe { ffi::AudioUnitUninitialize(self.unit.handle) };
    }
}

fn find_au_slot<'s>(slots: &'s [AuParamSlot], id: &str) -> Option<&'s AuParamSlot> {
    if let Some(slot) = slots.iter().find(|s| s.name == id) {
        return Some(slot);
    }
    if let Some(slot) = slots
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(id.trim()))
    {
        return Some(slot);
    }
    // Bare parameter numbers (`"2"` → id 2) stay drivable when a unit
    // reports no display name.
    if let Ok(numeric) = id.trim().parse::<u32>() {
        return slots.iter().find(|s| s.id == numeric);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn fourcc_packs_ascii_big_endian() {
        assert_eq!(fourcc([b'a', b'u', b'f', b'x']), 0x6175_6678);
        assert_eq!(fourcc_bytes(AU_TYPE_EFFECT), [b'a', b'u', b'f', b'x']);
        assert_eq!(fourcc_string(AU_TYPE_MUSIC_DEVICE), "aumu");
        assert_eq!(fourcc_string(AU_MANUFACTURER_APPLE), "appl");
        assert_eq!(fourcc_string(0x0000_0000), "????");
    }

    #[test]
    fn desc_display_names_codes() {
        let desc = AuComponentDesc::apple(AU_TYPE_EFFECT, fourcc([b'l', b'm', b't', b'r']));
        assert_eq!(desc.to_string(), "aufx:lmtr:appl");
        assert_eq!(AuComponentDesc::any().to_string(), "????:????:????");
    }

    #[test]
    fn flag_value_truth_table() {
        for yes in ["1", "true", "TRUE", " yes ", "on", "On"] {
            assert!(flag_value(yes), "{yes:?} should enable AU");
        }
        for no in ["", "0", "false", "no", "off", "2", "enable"] {
            assert!(!flag_value(no), "{no:?} should not enable AU");
        }
    }

    #[test]
    fn load_without_flag_is_deferred_not_failed() {
        // The v1 contract: AU behind the flag is *deferred*, never an error
        // the caller has to debug. No registry is touched on this path.
        match instantiate(&AuComponentDesc::any(), false) {
            Err(AuError::DisabledByFlag { flag }) => assert_eq!(flag, AU_ENABLE_ENV),
            #[cfg(not(target_os = "macos"))]
            Err(AuError::UnsupportedPlatform) => {}
            other => panic!("expected deferred load, got {other:?}"),
        }
    }

    #[test]
    fn unknown_component_is_not_found() {
        // 0x3f3f3f3f ('????') is not a real type code on any system, so this
        // probes the NotFound arm without depending on installed plugins.
        // Skipped when the flag is off: deferral must win over lookup.
        if !au_enabled() && cfg!(target_os = "macos") {
            return;
        }
        match instantiate(
            &AuComponentDesc::new(0x3f3f_3f3f, 0x3f3f_3f3f, 0x3f3f_3f3f),
            true,
        ) {
            Err(AuError::NotFound { .. }) => {}
            #[cfg(not(target_os = "macos"))]
            Err(AuError::UnsupportedPlatform) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn plist_key_extraction_reads_bundle_names() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleName</key><string>My &amp; Mighty Synth</string>
<key>CFBundleVersion</key><integer>3</integer>
</dict></plist>"#;
        assert_eq!(
            plist_string_for_key(plist, "CFBundleName"),
            Some("My & Mighty Synth".to_owned())
        );
        assert_eq!(plist_string_for_key(plist, "CFBundleVersion"), None);
        assert_eq!(plist_string_for_key(plist, "Missing"), None);
        // Nested markup is not a plain string: bail so the caller falls
        // back to the bundle stem instead of returning garbage.
        let nested = "<dict><key>K</key><dict><key>Q</key></dict></dict>";
        assert_eq!(plist_string_for_key(nested, "K"), None);
    }

    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

    fn fixture_root() -> PathBuf {
        let id = FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("ccez-au-scan-{}-{id}", std::process::id()))
    }

    fn write_bundle(root: &Path, file_name: &str, plist: Option<&str>) -> PathBuf {
        let bundle = root.join(file_name);
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        if let Some(text) = plist {
            std::fs::write(bundle.join("Contents/Info.plist"), text).unwrap();
        }
        bundle
    }

    #[test]
    fn scan_dir_lists_bundles_sorted_with_names() {
        let root = fixture_root();
        std::fs::create_dir_all(&root).unwrap();
        write_bundle(
            &root,
            "Zed.component",
            Some("<plist><dict><key>CFBundleName</key><string>Zed Synth</string></dict></plist>"),
        );
        write_bundle(&root, "Alpha.component", None);
        write_bundle(&root, "Ignored.vst3", None);
        std::fs::write(root.join("Stray.component"), "not a dir").unwrap();

        let found = scan_dir(&root);
        assert_eq!(found.len(), 2, "only .component dirs: {found:?}");
        assert_eq!(found[0].name, "Alpha"); // stem fallback, sorted first
        assert_eq!(found[1].name, "Zed Synth"); // plist name wins
        assert!(found.iter().all(|info| info.desc.is_none()));

        // A scan must survive one bad folder, not raise it.
        assert!(scan_dir(&root.join("does-not-exist")).is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn error_display_never_leaks_paths_or_handles() {
        let msg = AuError::DisabledByFlag {
            flag: AU_ENABLE_ENV,
        }
        .to_string();
        assert!(msg.contains(AU_ENABLE_ENV));
        let msg = AuError::NotFound {
            desc: AuComponentDesc::any(),
        }
        .to_string();
        assert!(msg.contains("no Audio Unit"));
        // Bridging errors name codes and statuses only — never paths
        // or handles.
        let msg = AuError::UnsupportedType {
            component_type: AU_TYPE_OUTPUT,
        }
        .to_string();
        assert!(msg.contains("auou"));
        assert!(AuError::BadParam("cutoff".to_string())
            .to_string()
            .contains("cutoff"));
        assert!(AuError::RenderFailed { status: -10878 }
            .to_string()
            .contains("-10878"));
    }

    /// Validation: one real AU plugin loads on this Mac. Uses the wildcard
    /// descriptor (first registered component — always an Apple unit on a
    /// stock system), opens it explicitly *past* the flag, and drops it.
    /// Gated to macOS; other targets prove the deferred path instead.
    #[cfg(target_os = "macos")]
    #[test]
    fn first_registered_au_instantiates() {
        assert!(
            component_exists(&AuComponentDesc::any()),
            "no Audio Units registered on this Mac"
        );
        let instance = instantiate(&AuComponentDesc::any(), true)
            .expect("first registered AU should instantiate in-process");
        assert_eq!(instance.desc(), AuComponentDesc::any());
        // `Drop` disposes the instance; completion without crashing is the
        // assertion — a leaked or double-freed AU would fault here.
    }

    fn apple_delay() -> AuComponentDesc {
        AuComponentDesc::apple(AU_TYPE_EFFECT, fourcc([b'd', b'e', b'l', b'y']))
    }

    #[test]
    fn render_scope_gate_refuses_untested_types_before_loading() {
        // Output/mixer/panner/converter/music-effect topologies are
        // outside v1: the refusal fires before the registry is touched,
        // so it holds on every platform (no flag, no hardware needed).
        for component_type in [
            AU_TYPE_OUTPUT,
            AU_TYPE_MIXER,
            AU_TYPE_PANNER,
            AU_TYPE_FORMAT_CONVERTER,
            fourcc([b'a', b'u', b'o', b'l']),
            fourcc([b'x', b'x', b'x', b'x']),
        ] {
            let desc = AuComponentDesc::apple(component_type, fourcc([b'd', b'e', b'l', b'y']));
            assert!(!au_render_supported(&desc));
            match super::AuBackend::load(&desc, 44100.0) {
                Err(AuError::UnsupportedType { .. }) => {}
                other => panic!("expected UnsupportedType, got {other:?}"),
            }
        }
        // The driven topologies pass the gate (loading itself still
        // needs macOS + a registered unit).
        for component_type in [AU_TYPE_EFFECT, AU_TYPE_MUSIC_DEVICE, AU_TYPE_GENERATOR] {
            assert!(au_render_supported(&AuComponentDesc::apple(
                component_type,
                fourcc([b'd', b'e', b'l', b'y'])
            )));
        }
    }

    /// Validation: Apple's AUDelay bridges its parameter list (observed
    /// on this Mac: ids 0–3, `Dry/Wet Mix` / `Delay Time` / `Feedback` /
    /// `Lowpass Cutoff Frequency`). Set/get round-trips live on the
    /// unit; out-of-range values clamp; unknown ids refuse.
    #[cfg(target_os = "macos")]
    #[test]
    fn apple_delay_param_bridging() {
        let mut backend = super::AuBackend::load(&apple_delay(), 44100.0).expect("load AUDelay");
        assert_eq!(backend.sample_rate(), 44100.0);
        let names: Vec<(u32, String)> = backend
            .param_list()
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                (0, "Dry/Wet Mix".to_string()),
                (1, "Delay Time".to_string()),
                (2, "Feedback".to_string()),
                (3, "Lowpass Cutoff Frequency".to_string()),
            ]
        );
        // Live seed: AUDelay's Delay Time defaults to 1.0s.
        let seed = backend.get_param("Delay Time").expect("seed read");
        assert!((seed - 1.0).abs() < 1e-6, "seed was {seed}");
        // By name, case-insensitively, and by bare number — all alias
        // the same unit parameter.
        backend.set_param("Delay Time", 0.5).expect("set by name");
        assert!((backend.get_param("delay time").expect("read") - 0.5).abs() < 1e-6);
        assert!((backend.get_param("1").expect("read") - 0.5).abs() < 1e-6);
        // Clamp, not refuse: Delay Time maxes at 2.0s.
        backend.set_param("Delay Time", 99.0).expect("clamped set");
        assert!((backend.get_param("1").expect("read") - 2.0).abs() < 1e-6);
        backend.set_param("Delay Time", 1.0).expect("restore");
        // Unknown ids never reach the unit.
        match backend.get_param("cutoff-x") {
            Err(AuError::BadParam(id)) => assert_eq!(id, "cutoff-x"),
            other => panic!("expected BadParam, got {other:?}"),
        }
        match backend.set_param("cutoff-x", 1.0) {
            Err(AuError::BadParam(_)) => {}
            other => panic!("expected BadParam, got {other:?}"),
        }
    }

    /// Validation: silence through Apple's AUDelay renders exact silence
    /// (empty delay lines + zero input leave nothing to output), and a
    /// tone passes through the effect's input-pull callback audibly.
    #[cfg(target_os = "macos")]
    #[test]
    fn apple_delay_renders_silence_and_tone() {
        let mut backend = super::AuBackend::load(&apple_delay(), 44100.0).expect("load AUDelay");
        let silence = vec![0.0f32; 256];
        let out = backend.process(&silence).expect("render silence");
        assert_eq!(out.len(), 256);
        assert!(out.iter().all(|&s| s == 0.0), "silence must stay bit-exact");
        // A tone through a fresh 1s delay: the dry path renders
        // immediately (only the first sample is still zero).
        let tone: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin()).collect();
        let wet = backend.process(&tone).expect("render tone");
        assert_eq!(wet.len(), 256);
        assert!(
            wet.iter().filter(|&&s| s != 0.0).count() > 200,
            "dry path must pass the tone through"
        );
        // Empty input renders empty: a zero-frame AudioUnitRender is a
        // refusal, not silence, so the backend short-circuits.
        assert!(backend.process(&[]).expect("empty").is_empty());
        // Blocks past the slice limit chunk without changing shape.
        let long = backend.process(&vec![0.0f32; 3000]).expect("long block");
        assert_eq!(long.len(), 3000);
        assert!(long.iter().all(|&s| s == 0.0));
    }

    /// Validation: AUDelay honestly reports zero latency samples, and the
    /// reading feeds the delay-compensation map like every backend.
    #[cfg(target_os = "macos")]
    #[test]
    fn apple_delay_latency_feeds_compensation_map() {
        use crate::plugins::latency::LatencyMap;
        let backend = super::AuBackend::load(&apple_delay(), 44100.0).expect("load AUDelay");
        assert_eq!(backend.latency_samples(), 0);
        let mut map = LatencyMap::new();
        map.report("au-delay", backend.latency_samples()).expect("report");
        assert_eq!(map.chain_latency(&["au-delay".to_string()]), 0);
    }

    /// Validation: backend state round-trips worker-side truth (params
    /// restore onto the unit; unknown ids ride along host-side).
    #[cfg(target_os = "macos")]
    #[test]
    fn apple_delay_state_restores_onto_unit() {
        let mut backend = super::AuBackend::load(&apple_delay(), 44100.0).expect("load AUDelay");
        backend.set_param("Feedback", 25.0).expect("set");
        let state = backend.state();
        assert_eq!(state.params.get("Feedback"), Some(&25.0));
        backend.set_param("Feedback", 75.0).expect("retune");
        let mut restored = state.clone();
        restored.params.insert("future-param".to_string(), 3.0);
        backend.set_state(&restored);
        assert!((backend.get_param("Feedback").expect("read") - 25.0).abs() < 1e-6);
        assert_eq!(backend.state(), restored);
    }

    /// Validation: the system scan finds the units macOS ships with.
    #[cfg(target_os = "macos")]
    #[test]
    fn system_scan_finds_bundled_units() {
        let found = scan_system();
        assert!(
            !found.is_empty(),
            "expected ≥1 .component bundle under the AU search paths"
        );
    }
}
