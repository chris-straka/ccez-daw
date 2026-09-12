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
}

/// Why [`PluginKind::Clap`] is a documented seam rather than live code.
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
/// v1 ships no new cargo dependencies (offline-friendly, no supply-chain
/// surface) and proves the sandbox + snapshot + recovery machinery with
/// the mock backend. A future `ClapBackend` (clack-host + libloading)
/// runs *inside the worker process* — untrusted C-ABI code must live
/// behind the process boundary anyway — and speaks the existing
/// [`crate::plugins::worker`] protocol, so the host and recovery paths
/// below do not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClapNote;

/// One loadable plugin: identity plus where its DSP comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// Stable id (matches the device [`Node`](crate::model::Node) id when
    /// the plugin backs a chain insert).
    pub id: String,
    pub name: String,
    pub kind: PluginKind,
    /// Filesystem path to the `.clap` bundle. Required for
    /// [`PluginKind::Clap`], ignored for [`PluginKind::Mock`].
    pub path: Option<String>,
}

impl PluginDescriptor {
    pub fn mock(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Mock,
            path: None,
        }
    }

    pub fn clap(id: &str, name: &str, path: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: PluginKind::Clap,
            path: Some(path.to_string()),
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
    /// Real `.clap` loading is the phase-2 seam (see [`ClapNote`]).
    /// Mirrors Track B's `RemoteUnimplemented`: mark, refuse, never branch.
    ClapUnimplemented(String),
    BadState(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPlugin(id) => write!(f, "unknown plugin `{id}`"),
            Self::DuplicatePlugin(id) => write!(f, "plugin `{id}` already loaded"),
            Self::Sandbox(e) => write!(f, "plugin sandbox: {e}"),
            Self::ClapUnimplemented(p) => write!(
                f,
                "CLAP bundle `{p}` cannot load yet (clack-host seam; use Mock)"
            ),
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
    /// snapshot (the baseline `recover()` restores to).
    pub fn load(&mut self, descriptor: PluginDescriptor) -> Result<()> {
        if self.entries.contains_key(&descriptor.id) {
            return Err(HostError::DuplicatePlugin(descriptor.id.clone()));
        }
        if descriptor.kind == PluginKind::Clap {
            return Err(HostError::ClapUnimplemented(
                descriptor.path.clone().unwrap_or_default(),
            ));
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

    pub fn restore(&mut self, id: &str, state: &PluginState) -> Result<()> {
        Ok(self.entry_mut(id)?.plugin.restore(state)?)
    }

    /// `true` when the plugin's worker process is up.
    pub fn alive(&mut self, id: &str) -> Result<bool> {
        Ok(self.entry_mut(id)?.plugin.alive())
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
    fn clap_bundle_refuses_at_the_seam() {
        let mut host = host();
        let err = host
            .load(PluginDescriptor::clap("vst", "Synth", "/lib/synth.clap"))
            .expect_err("clap must refuse");
        assert!(matches!(err, HostError::ClapUnimplemented(_)));
        // The refusal leaves no half-loaded entry behind.
        assert!(host.ids().is_empty());
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
