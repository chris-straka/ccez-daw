//! Plugin scan-list model: one sorted, de-duplicated view over every
//! discovered plugin, whatever format found it.
//!
//! Teaching note: discovery (walking folders for `*.vst3`, `*.component`,
//! `*.clap`) already lives next to each format — [`Vst3Bundle::scan_dir`]
//! for VST3, [`scan_dir`](crate::plugins::au::scan_dir) for AU. What was
//! missing is the *browser model*: one list the UI shows, the engine
//! instantiates from, and a scan cache serializes. That is this module:
//!
//! - [`ScannedPlugin`]: one entry — stable [`id`](ScannedPlugin::id),
//!   display [`name`](ScannedPlugin::name), [`format`](ScannedPlugin::format),
//!   optional bundle [`path`](ScannedPlugin::path), vendor/version.
//! - [`ScanList`]: the owned list. [`insert`](ScanList::insert) de-dupes
//!   on `(format, path-or-id)` (first wins — rescans never reorder the
//!   browser), [`sorted`](ScanList::sorted) returns the display order
//!   (format, then name), [`merge`](ScanList::merge) folds in another
//!   scan, and JSON round-trips it for the scan cache.
//!
//! The id contract: `<format-prefix>:<stable-tail>` where the tail is the
//! bundle path for file formats (`vst3:/lib/Comp.vst3`) and the
//! descriptor id for registry formats (`au:aufx:lmtr:appl`, `mock:gain`).
//! Two entries with the same id are the same plugin by construction.
//!
//! Reads no frozen types and adds no IPC or project-schema surface, so the
//! typegen drift gate (`bun run typegen -- --check`) is unaffected.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::plugins::au::AuPluginInfo;
use crate::plugins::vst3::Vst3Bundle;

/// Which plugin API found the entry. Serializes lowercase (`"vst3"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    Clap,
    Vst3,
    #[serde(rename = "au")]
    AudioUnit,
    Mock,
}

impl std::fmt::Display for PluginFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clap => write!(f, "clap"),
            Self::Vst3 => write!(f, "vst3"),
            Self::AudioUnit => write!(f, "au"),
            Self::Mock => write!(f, "mock"),
        }
    }
}

impl std::str::FromStr for PluginFormat {
    type Err = ScanError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "clap" => Ok(Self::Clap),
            "vst3" => Ok(Self::Vst3),
            "au" | "audiounit" | "audio-unit" => Ok(Self::AudioUnit),
            "mock" => Ok(Self::Mock),
            other => Err(ScanError::BadEntry(format!("unknown plugin format `{other}`"))),
        }
    }
}

/// One discovered plugin: identity plus browser display fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedPlugin {
    /// Stable id (`vst3:/lib/Comp.vst3`, `au:aufx:lmtr:appl`, `mock:gain`).
    pub id: String,
    pub name: String,
    pub format: PluginFormat,
    /// Bundle/library path for file formats; `None` for registry entries.
    pub path: Option<String>,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub version: String,
}

impl ScannedPlugin {
    /// New entry; empty ids and empty names are rejected (a browser row
    /// with no identity is a crash report waiting for a user).
    pub fn new(
        id: &str,
        name: &str,
        format: PluginFormat,
        path: Option<String>,
    ) -> Result<Self, ScanError> {
        if id.trim().is_empty() {
            return Err(ScanError::BadEntry("plugin id must be non-empty".to_string()));
        }
        if name.trim().is_empty() {
            return Err(ScanError::BadEntry(format!(
                "plugin `{id}` needs a display name"
            )));
        }
        Ok(Self {
            id: id.to_string(),
            name: name.to_string(),
            format,
            path,
            vendor: String::new(),
            version: String::new(),
        })
    }

    pub fn with_vendor(mut self, vendor: &str) -> Self {
        self.vendor = vendor.to_string();
        self
    }

    pub fn with_version(mut self, version: &str) -> Self {
        self.version = version.to_string();
        self
    }

    /// Built-in / test entry with no bundle on disk.
    pub fn mock(id: &str, name: &str) -> Result<Self, ScanError> {
        Self::new(&format!("mock:{id}"), name, PluginFormat::Mock, None)
    }

    /// CLAP bundle entry (loadable once the clack-host seam lands).
    /// Identity is path-derived (`clap:<path>`); the display name stays free.
    pub fn clap(name: &str, path: &Path) -> Result<Self, ScanError> {
        Self::new(
            &format!("clap:{}", path.display()),
            name,
            PluginFormat::Clap,
            Some(path.display().to_string()),
        )
    }

    /// VST3 bundle entry (loadable through the sandboxed `Vst3Backend`).
    /// Identity is path-derived (`vst3:<path>`); the display name stays free.
    pub fn vst3(name: &str, path: &Path) -> Result<Self, ScanError> {
        Self::new(
            &format!("vst3:{}", path.display()),
            name,
            PluginFormat::Vst3,
            Some(path.display().to_string()),
        )
    }

    /// Key for de-duplication: `(format, path-or-id)`.
    fn dedup_key(&self) -> (PluginFormat, &str) {
        match self.path.as_deref() {
            Some(p) => (self.format, p),
            None => (self.format, self.id.as_str()),
        }
    }
}

impl std::fmt::Display for ScannedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} ({})", self.format, self.name, self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    BadEntry(String),
    BadJson(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadEntry(m) => write!(f, "bad scan entry: {m}"),
            Self::BadJson(m) => write!(f, "bad scan-list json: {m}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// The browser/scan-cache list: owned entries plus per-path issues that
/// did not fail the scan (a DAW scan survives one bad folder).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanList {
    pub plugins: Vec<ScannedPlugin>,
    #[serde(default)]
    pub issues: Vec<String>,
}

impl ScanList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Insert one entry. A duplicate `(format, path-or-id)` is ignored
    /// (first wins) and reported as `false`; a fresh entry returns `true`.
    pub fn insert(&mut self, plugin: ScannedPlugin) -> bool {
        let key = plugin.dedup_key();
        let dup = self.plugins.iter().any(|p| p.dedup_key() == key);
        if dup {
            return false;
        }
        self.plugins.push(plugin);
        true
    }

    /// Record a non-fatal scan issue (unreadable dir, bad descriptor).
    pub fn note_issue(&mut self, issue: impl Into<String>) {
        self.issues.push(issue.into());
    }

    /// Remove by stable id, returning the entry. Missing ids are `None`
    /// (uninstalls are idempotent).
    pub fn remove(&mut self, id: &str) -> Option<ScannedPlugin> {
        let at = self.plugins.iter().position(|p| p.id == id)?;
        Some(self.plugins.remove(at))
    }

    pub fn find(&self, id: &str) -> Option<&ScannedPlugin> {
        self.plugins.iter().find(|p| p.id == id)
    }

    /// All entries of one format, in insertion order.
    pub fn of_format(&self, format: PluginFormat) -> Vec<&ScannedPlugin> {
        self.plugins.iter().filter(|p| p.format == format).collect()
    }

    /// Display order: format first (clap, vst3, au, mock sort by the
    /// enum), then name. The stored order is insertion order — rescans
    /// merge without reshuffling what the user already sees.
    pub fn sorted(&self) -> Vec<&ScannedPlugin> {
        let mut out: Vec<&ScannedPlugin> = self.plugins.iter().collect();
        out.sort_by(|a, b| (a.format, &a.name).cmp(&(b.format, &b.name)));
        out
    }

    /// Fold another scan in (re-scan path): fresh entries append, dupes
    /// keep their original row, issues accumulate.
    pub fn merge(&mut self, other: ScanList) {
        for p in other.plugins {
            self.insert(p);
        }
        self.issues.extend(other.issues);
    }

    /// Serialize the list (the scan-cache file shape).
    pub fn to_json(&self) -> Result<String, ScanError> {
        serde_json::to_string(self).map_err(|e| ScanError::BadJson(e.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self, ScanError> {
        serde_json::from_str(json).map_err(|e| ScanError::BadJson(e.to_string()))
    }

    /// Build from VST3 bundles found by [`Vst3Bundle::scan_dir`].
    pub fn from_vst3_bundles(bundles: &[Vst3Bundle]) -> Self {
        let mut list = Self::new();
        for b in bundles {
            let path = b.path.display().to_string();
            match ScannedPlugin::new(
                &format!("vst3:{path}"),
                &b.name,
                PluginFormat::Vst3,
                Some(path),
            ) {
                Ok(p) => {
                    list.insert(p);
                }
                Err(e) => list.note_issue(e.to_string()),
            }
        }
        list
    }

    /// Build from AU bundles found by the AU [`scan_dir`](crate::plugins::au::scan_dir).
    pub fn from_au_infos(infos: &[AuPluginInfo]) -> Self {
        let mut list = Self::new();
        for info in infos {
            let path = info.bundle_path.display().to_string();
            let id = match &info.desc {
                Some(desc) => format!("au:{desc}"),
                None => format!("au:{path}"),
            };
            match ScannedPlugin::new(&id, &info.name, PluginFormat::AudioUnit, Some(path)) {
                Ok(p) => {
                    list.insert(p);
                }
                Err(e) => list.note_issue(e.to_string()),
            }
        }
        list
    }
}

/// Scan one directory for VST3 bundles into a [`ScanList`]. Missing or
/// unreadable dirs yield an empty list with an issue note — never an
/// error (standard plugin locations often do not exist yet).
pub fn scan_vst3_dir(dir: &Path) -> ScanList {
    if !dir.is_dir() {
        let mut list = ScanList::new();
        list.note_issue(format!("vst3 scan: {} is not a directory", dir.display()));
        return list;
    }
    ScanList::from_vst3_bundles(&Vst3Bundle::scan_dir(dir))
}

/// Scan one directory for AU bundles into a [`ScanList`]. Same
/// never-fails-loud contract as [`scan_vst3_dir`].
pub fn scan_au_dir(dir: &Path) -> ScanList {
    if !dir.is_dir() {
        let mut list = ScanList::new();
        list.note_issue(format!("au scan: {} is not a directory", dir.display()));
        return list;
    }
    ScanList::from_au_infos(&crate::plugins::au::scan_dir(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn scratch(name: &str) -> std::path::PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ccez-scan-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn format_names_round_trip() {
        for (f, s) in [
            (PluginFormat::Clap, "clap"),
            (PluginFormat::Vst3, "vst3"),
            (PluginFormat::AudioUnit, "au"),
            (PluginFormat::Mock, "mock"),
        ] {
            assert_eq!(f.to_string(), s);
            assert_eq!(s.parse::<PluginFormat>().expect("parse"), f);
        }
        assert!("dxi".parse::<PluginFormat>().is_err());
        // Aliases parse; JSON uses the short form.
        assert_eq!(
            "AudioUnit".parse::<PluginFormat>().expect("alias"),
            PluginFormat::AudioUnit
        );
        let json = serde_json::to_string(&PluginFormat::AudioUnit).expect("json");
        assert_eq!(json, "\"au\"");
    }

    #[test]
    fn entries_reject_identity_gaps() {
        assert!(ScannedPlugin::new("", "Name", PluginFormat::Mock, None).is_err());
        assert!(ScannedPlugin::new("mock:x", "", PluginFormat::Mock, None).is_err());
        let m = ScannedPlugin::mock("gain", "Gain").expect("mock");
        assert_eq!(m.id, "mock:gain");
        assert_eq!(m.path, None);
        let c = ScannedPlugin::clap("Comp", Path::new("/lib/comp.clap")).expect("clap");
        assert_eq!(c.id, "clap:/lib/comp.clap");
    }

    #[test]
    fn insert_dedupes_first_wins_remove_is_idempotent() {
        let mut list = ScanList::new();
        assert!(list.is_empty());
        let a = ScannedPlugin::mock("gain", "Gain").expect("a");
        assert!(list.insert(a.clone()));
        // Same path-or-id: ignored, original row kept.
        let renamed = ScannedPlugin {
            name: "Renamed".to_string(),
            ..a.clone()
        };
        assert!(!list.insert(renamed));
        assert_eq!(list.find("mock:gain").expect("find").name, "Gain");
        // Same id text but a different format is a different plugin.
        let mut vst3_twin = a.clone();
        vst3_twin.format = PluginFormat::Vst3;
        assert!(list.insert(vst3_twin));
        assert_eq!(list.len(), 2);
        assert!(list.remove("mock:nope").is_none());
        assert!(list.remove("mock:gain").is_some());
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn sorted_orders_by_format_then_name_and_merge_keeps_rows() {
        let mut list = ScanList::new();
        list.insert(ScannedPlugin::mock("z", "Zed").expect("z"));
        list.insert(
            ScannedPlugin::clap("Comp", Path::new("/lib/c.clap")).expect("c"),
        );
        list.insert(ScannedPlugin::mock("a", "Alpha").expect("a"));
        let names: Vec<&str> = list.sorted().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Comp", "Alpha", "Zed"]);
        // Stored order is untouched by the view.
        assert_eq!(list.plugins[0].name, "Zed");

        let mut rescan = ScanList::new();
        rescan.insert(
            ScannedPlugin::clap("Comp Renamed", Path::new("/lib/c.clap")).expect("dupe"),
        );
        rescan.insert(ScannedPlugin::mock("new", "Newbie").expect("new"));
        rescan.note_issue("one bad folder");
        list.merge(rescan);
        // Dupe kept its original row; the genuinely new plugin appended.
        assert_eq!(list.find("clap:/lib/c.clap").expect("kept").name, "Comp");
        assert!(list.find("mock:new").is_some());
        assert_eq!(list.issues, vec!["one bad folder".to_string()]);
        assert_eq!(list.of_format(PluginFormat::Mock).len(), 3);
    }

    #[test]
    fn json_round_trip_is_exact_and_rejects_garbage() {
        let mut list = ScanList::new();
        list.insert(ScannedPlugin::mock("gain", "Gain").expect("m"));
        list.note_issue("scratch");
        let json = list.to_json().expect("to json");
        assert_eq!(ScanList::from_json(&json).expect("from json"), list);
        assert!(ScanList::from_json("{bad").is_err());
    }

    #[test]
    fn vst3_and_au_dir_scans_share_one_model() {
        let root = scratch("mixed");
        // One descriptor-less VST3 bundle dir + one AU component dir.
        let vst3 = root.join("Comp.vst3");
        std::fs::create_dir_all(vst3.join("Contents")).expect("vst3 bundle");
        let au = root.join("Delay.component");
        std::fs::create_dir_all(au.join("Contents")).expect("au bundle");
        std::fs::write(root.join("Stray.component"), "not a dir").expect("stray");

        let mut list = scan_vst3_dir(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list.plugins[0].format, PluginFormat::Vst3);
        // AU component without a plist falls back to the bundle stem.
        let au_list = scan_au_dir(&root);
        list.merge(au_list);
        assert_eq!(list.len(), 2);
        assert!(list.find("au:Delay").is_none()); // id carries the path tail
        assert_eq!(list.of_format(PluginFormat::AudioUnit).len(), 1);

        // Missing dirs are notes, not errors.
        let missing = scan_vst3_dir(&root.join("nope"));
        assert!(missing.is_empty() && !missing.issues.is_empty());
        let missing_au = scan_au_dir(&root.join("nope"));
        assert!(missing_au.is_empty() && !missing_au.issues.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn from_au_infos_prefers_registry_identity() {
        let with_desc = AuPluginInfo {
            name: "Limiter".to_string(),
            bundle_path: std::path::PathBuf::from("/Lib/Limit.component"),
            desc: Some(crate::plugins::au::AuComponentDesc::apple(
                crate::plugins::au::AU_TYPE_EFFECT,
                crate::plugins::au::fourcc([b'l', b'm', b't', b'r']),
            )),
        };
        let list = ScanList::from_au_infos(&[with_desc]);
        assert_eq!(list.len(), 1);
        assert_eq!(list.plugins[0].id, "au:aufx:lmtr:appl");
    }
}
