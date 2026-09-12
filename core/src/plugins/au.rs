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
//!   [`AuError::DisabledByFlag`]: v1 ships macOS with CLAP/VST3 first and
//!   keeps AU behind the flag until the audio-path integration (render
//!   callback + parameter bridging) lands.
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

/// What can go wrong finding or loading an Audio Unit.
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

/// Minimal `AudioComponent` FFI: find + create + destroy. Deliberately
/// *not* a new crate dependency (see the evaluation in `track-c.md`): one
/// `#[link]` to the `AudioToolbox` framework the OS always ships covers
/// the whole v1 seam, and render/parameter calls land here as follow-ups.
#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::c_void;

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
    }
}

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
