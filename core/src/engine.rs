//! Track A: project engine + history.
//!
//! The project document is event-sourced: an append-only op log plus
//! snapshots (`contracts/op-log-format.md`). This module owns that machinery:
//!
//! - [`Engine::apply`] appends one [`Op`](crate::model::Op) and returns its
//!   sequence number (`seq` is assigned here, monotonically per project).
//! - [`Engine::undo`] / [`Engine::redo`] give infinite cross-session
//!   undo. Undo appends an `UndoMarker` op naming the undone `seq`; redo
//!   appends a second marker that cancels it. Because both directions are
//!   just more log entries, history survives restart and redoable branches
//!   stay addressable at op granularity.
//! - Snapshots bound replay: [`Engine::snapshot`] stores the full project at
//!   the current tip, and rebuilds replay only the ops after it.
//! - Instant autosave: every mutation appends to `ops.jsonl` (fsynced) before
//!   it returns, so `kill -9` loses nothing. Recovery replays snapshot + log
//!   and ignores a torn trailing line.
//! - Lazy load: heavy blobs (audio, MIDI, presets, frozen plugin states)
//!   live as separate files under `assets/` and are read only on demand via
//!   [`Engine::load_asset`]. Opening a project never touches them.
//! - Portable bundle: [`Engine::export_bundle`] / [`import_bundle`] copy the
//!   whole package (project + log + snapshot + manifest + assets) as one
//!   directory.
//!
//! On-disk layout (`<dir>/`):
//!
//! ```text
//! project.json    last materialized state (convenience; NOT the truth)
//! ops.jsonl       the truth: one JSON op per line, append-only
//! snapshot.json   { seq, project } bounding replay (optional)
//! manifest.json   asset index { key, kind, size }
//! assets/<key>    opaque blobs referenced by clip `source` fields
//! ```
//!
//! Recovery never trusts `project.json`: it rebuilds from snapshot + log, so
//! a crash torn mid-autosave still replays exactly.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{Clip, Op, OpKind, Project, Track};

/// How often (in appended ops) an automatic snapshot is taken. Bounds replay.
const DEFAULT_SNAPSHOT_EVERY: usize = 64;

/// Conventional asset kinds carried in a portable bundle. Stored as an
/// opaque string so new kinds never break the format; these four are the
/// ones the bundle contract promises to carry.
pub const ASSET_KIND_AUDIO: &str = "audio";
pub const ASSET_KIND_MIDI: &str = "midi";
pub const ASSET_KIND_PRESET: &str = "preset";
pub const ASSET_KIND_PLUGIN: &str = "plugin";

#[derive(Debug)]
pub enum EngineError {
    Io(std::io::Error),
    Json(serde_json::Error),
    BadActor(String),
    BadPayload(String),
    UnknownTarget(String),
    DuplicateTarget(String),
    NothingToUndo,
    NothingToRedo,
    NotAProject(PathBuf),
    CorruptLog(String),
    CorruptBundle(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "engine io: {e}"),
            Self::Json(e) => write!(f, "engine json: {e}"),
            Self::BadActor(a) => write!(f, "bad actor `{a}` (want ui, mcp, ai:<x>, script:<x>)"),
            Self::BadPayload(m) => write!(f, "bad op payload: {m}"),
            Self::UnknownTarget(t) => write!(f, "unknown op target `{t}`"),
            Self::DuplicateTarget(t) => write!(f, "duplicate op target `{t}`"),
            Self::NothingToUndo => write!(f, "nothing to undo"),
            Self::NothingToRedo => write!(f, "nothing to redo"),
            Self::NotAProject(p) => write!(f, "not a project dir: {}", p.display()),
            Self::CorruptLog(m) => write!(f, "corrupt op log: {m}"),
            Self::CorruptBundle(m) => write!(f, "corrupt bundle: {m}"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// `true` for the frozen actor set: `ui`, `mcp`, `ai:<sidecar>`,
/// `script:<name>`. All AI output lands as ordinary `ai:*` ops so it stays
/// undoable by design.
pub fn valid_actor(actor: &str) -> bool {
    actor == "ui"
        || actor == "mcp"
        || actor.strip_prefix("ai:").is_some_and(|s| !s.is_empty())
        || actor
            .strip_prefix("script:")
            .is_some_and(|s| !s.is_empty())
}

/// Snapshot bounding log replay: the full project as of `seq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Last log `seq` covered by `project` (0 when the log is empty).
    pub seq: u64,
    pub project: Project,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetEntry {
    pub key: String,
    pub kind: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub assets: Vec<AssetEntry>,
}

/// Apply one op's effect to a project. Pure (no I/O): used both for live
/// apply and for replay during recovery. `UndoMarker` is a no-op here;
/// markers take effect in [`rebuild`] via the undone set.
pub fn apply_op_to_project(project: &mut Project, op: &Op) -> Result<()> {
    match op.kind {
        OpKind::TrackAdded => {
            let track: Track = serde_json::from_str(&op.value_json).map_err(|_| {
                EngineError::BadPayload(format!("TrackAdded needs a Track JSON: {}", op.value_json))
            })?;
            if project.tracks.iter().any(|t| t.id == track.id) {
                return Err(EngineError::DuplicateTarget(track.id));
            }
            project.tracks.push(track);
        }
        OpKind::ClipAdded => {
            let clip: Clip = serde_json::from_str(&op.value_json).map_err(|_| {
                EngineError::BadPayload(format!("ClipAdded needs a Clip JSON: {}", op.value_json))
            })?;
            if project.clips.iter().any(|c| c.id == clip.id) {
                return Err(EngineError::DuplicateTarget(clip.id));
            }
            let track = project
                .tracks
                .iter_mut()
                .find(|t| t.id == clip.track_id)
                .ok_or_else(|| EngineError::UnknownTarget(clip.track_id.clone()))?;
            if !track.clip_ids.contains(&clip.id) {
                track.clip_ids.push(clip.id.clone());
            }
            project.clips.push(clip);
        }
        OpKind::ClipMoved => {
            let clip = project
                .clips
                .iter_mut()
                .find(|c| c.id == op.target)
                .ok_or_else(|| EngineError::UnknownTarget(op.target.clone()))?;
            clip.start_beats = parse_beats(&op.value_json)?;
        }
        OpKind::ParamSet => {
            let (node, param) = op
                .target
                .split_once(':')
                .ok_or_else(|| EngineError::UnknownTarget(op.target.clone()))?;
            let value = parse_number(&op.value_json, "ParamSet")?;
            apply_param(project, node, param, value)?;
        }
        OpKind::TempoSet => {
            project.tempo = parse_number(&op.value_json, "TempoSet")?;
        }
        OpKind::AutomationPointSet => {
            apply_automation_point(project, op)?;
        }
        OpKind::UndoMarker => {}
    }
    Ok(())
}

/// `AutomationPointSet` payloads: `{"beat": <n>, "value": <n>}`, plus
/// `"node"`/`"param"` when the target lane does not exist yet (the lane
/// is created). Upsert: an exact-beat point is replaced, otherwise the
/// point inserts keeping strictly-ascending beat order.
fn apply_automation_point(project: &mut Project, op: &Op) -> Result<()> {
    let v: serde_json::Value =
        serde_json::from_str(&op.value_json).map_err(|_| {
            EngineError::BadPayload(format!(
                "AutomationPointSet needs {{\"beat\": <n>, \"value\": <n>}}: {}",
                op.value_json
            ))
        })?;
    let o = v.as_object().ok_or_else(|| {
        EngineError::BadPayload(format!(
            "AutomationPointSet needs {{\"beat\": <n>, \"value\": <n>}}: {}",
            op.value_json
        ))
    })?;
    let beat = o
        .get("beat")
        .and_then(|n| n.as_f64())
        .ok_or_else(|| {
            EngineError::BadPayload(format!(
                "AutomationPointSet needs a finite \"beat\": {}",
                op.value_json
            ))
        })?;
    let value = o
        .get("value")
        .and_then(|n| n.as_f64())
        .ok_or_else(|| {
            EngineError::BadPayload(format!(
                "AutomationPointSet needs a finite \"value\": {}",
                op.value_json
            ))
        })?;
    if !beat.is_finite() || beat < 0.0 || !value.is_finite() {
        return Err(EngineError::BadPayload(format!(
            "AutomationPointSet beat must be finite and >= 0, value finite: {}",
            op.value_json
        )));
    }
    let lane = match project.automation.iter_mut().find(|l| l.id == op.target) {
        Some(l) => l,
        None => {
            let node = o
                .get("node")
                .and_then(|n| n.as_str())
                .ok_or_else(|| EngineError::UnknownTarget(op.target.clone()))?;
            let param = o
                .get("param")
                .and_then(|n| n.as_str())
                .ok_or_else(|| EngineError::UnknownTarget(op.target.clone()))?;
            if node.is_empty() || param.is_empty() {
                return Err(EngineError::UnknownTarget(op.target.clone()));
            }
            project.automation.push(crate::model::AutomationLane {
                id: op.target.clone(),
                target: crate::model::ParamAddress {
                    node: node.to_string(),
                    param: param.to_string(),
                },
                points: Vec::new(),
            });
            project.automation.last_mut().expect("just pushed")
        }
    };
    match lane.points.iter_mut().find(|p| p.beat == beat) {
        Some(p) => p.value = value,
        None => {
            let pos = lane
                .points
                .iter()
                .position(|p| p.beat > beat)
                .unwrap_or(lane.points.len());
            lane.points.insert(
                pos,
                crate::model::AutomationPoint { beat, value },
            );
        }
    }
    Ok(())
}

/// `ClipMoved` payloads: `{"startBeats": 8}` per the frozen contract,
/// `{"start_beats": 8}`, or a bare number.
fn parse_beats(value_json: &str) -> Result<f64> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(value_json) {
        if let Some(n) = v.as_f64() {
            return Ok(n);
        }
        if let Some(o) = v.as_object() {
            for key in ["startBeats", "start_beats"] {
                if let Some(n) = o.get(key).and_then(|n| n.as_f64()) {
                    return Ok(n);
                }
            }
        }
    }
    Err(EngineError::BadPayload(format!(
        "ClipMoved needs {{\"startBeats\": <n>}} or a number: {value_json}"
    )))
}

/// Bare number payloads (`"0.5"`), also accepting `{"value": x}` /
/// `{"tempo": x}` objects defensively.
fn parse_number(value_json: &str, what: &str) -> Result<f64> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(value_json) {
        if let Some(n) = v.as_f64() {
            return Ok(n);
        }
        if let Some(o) = v.as_object() {
            for key in ["value", "tempo"] {
                if let Some(n) = o.get(key).and_then(|n| n.as_f64()) {
                    return Ok(n);
                }
            }
        }
    }
    Err(EngineError::BadPayload(format!(
        "{what} needs a numeric JSON payload: {value_json}"
    )))
}

/// Universal `node:param` addressing over tracks (volume/pan/muted/solo)
/// and device-node params (clamped to range).
fn apply_param(project: &mut Project, node: &str, param: &str, value: f64) -> Result<()> {
    if let Some(track) = project.tracks.iter_mut().find(|t| t.id == node) {
        match param {
            "volume" | "pan" => {
                let v = value.clamp(0.0, 1.5);
                if param == "volume" {
                    track.volume = v;
                } else {
                    track.pan = value.clamp(-1.0, 1.0);
                }
                return Ok(());
            }
            "muted" => {
                track.muted = value != 0.0;
                return Ok(());
            }
            "solo" => {
                track.solo = value != 0.0;
                return Ok(());
            }
            _ => {}
        }
    }
    if let Some(dev) = project.devices.iter_mut().find(|d| d.id == node) {
        if let Some(p) = dev.params.iter_mut().find(|p| p.id == param) {
            p.value = value.clamp(p.min, p.max);
            return Ok(());
        }
    }
    Err(EngineError::UnknownTarget(format!("{node}:{param}")))
}

/// Fold the marker stream into the set of undone seqs. Markers toggle:
/// a plain `UndoMarker{target: S}` inserts S; a marker whose payload is
/// `{"redo": true}` removes S again. Markers are processed in seq order so
/// undo-then-redo-then-undo replays exactly.
fn fold_undone(log: &[Op]) -> BTreeSet<u64> {
    let mut undone = BTreeSet::new();
    for op in log {
        if op.kind != OpKind::UndoMarker {
            continue;
        }
        let Ok(seq) = op.target.parse::<u64>() else {
            continue;
        };
        if is_redo_marker(op) {
            undone.remove(&seq);
        } else {
            undone.insert(seq);
        }
    }
    undone
}

fn is_redo_marker(op: &Op) -> bool {
    serde_json::from_str::<serde_json::Value>(&op.value_json)
        .ok()
        .and_then(|v| v.get("redo").and_then(|r| r.as_bool()))
        .unwrap_or(false)
}

/// Rebuild state from a base project plus the live (non-undone,
/// non-marker) ops after `base_seq`, in seq order.
fn rebuild(base: &Project, base_seq: u64, log: &[Op], undone: &BTreeSet<u64>) -> Result<Project> {
    let mut project = base.clone();
    for op in log.iter().filter(|op| op.seq > base_seq) {
        if op.kind == OpKind::UndoMarker || undone.contains(&op.seq) {
            continue;
        }
        apply_op_to_project(&mut project, op).map_err(|e| {
            EngineError::CorruptLog(format!("replay of seq {} failed: {e}", op.seq))
        })?;
    }
    Ok(project)
}

fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join("snapshot.json")
}
fn ops_path(dir: &Path) -> PathBuf {
    dir.join("ops.jsonl")
}
fn project_path(dir: &Path) -> PathBuf {
    dir.join("project.json")
}
fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}
fn assets_dir(dir: &Path) -> PathBuf {
    dir.join("assets")
}

/// Read the append-only log, tolerating a torn `kill -9` tail: blank lines
/// are skipped, and a malformed *final* line is dropped (a crash mid-append
/// can leave half a line). A malformed line anywhere else is corruption.
fn read_log(path: &Path) -> Result<Vec<Op>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let lines: Vec<String> = BufReader::new(file).lines().collect::<std::io::Result<_>>()?;
    let mut ops = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let last = i + 1 == lines.len();
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Op>(line) {
            Ok(op) => ops.push(op),
            Err(e) => {
                if last {
                    break; // torn tail from a crash mid-append: replay what survived
                }
                return Err(EngineError::CorruptLog(format!("line {}: {e}", i + 1)));
            }
        }
    }
    // Monotonic seqs are the replay invariant; enforce on load, not just trust.
    let mut prev = 0u64;
    let mut first = true;
    for op in &ops {
        if !first && op.seq <= prev {
            return Err(EngineError::CorruptLog(format!(
                "seq {} out of order after {prev}",
                op.seq
            )));
        }
        first = false;
        prev = op.seq;
    }
    Ok(ops)
}

/// Atomic write (tmp + rename) so a crash never leaves half a JSON doc.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

fn read_manifest(dir: &Path) -> Manifest {
    let path = manifest_path(dir);
    if !path.exists() {
        return Manifest {
            version: 0,
            assets: Vec::new(),
        };
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Manifest {
            version: 0,
            assets: Vec::new(),
        })
}

/// Asset keys are bare filenames: no slashes, no `..`, nothing absolute.
fn check_asset_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key == "."
        || key == ".."
        || key.contains('/')
        || key.contains('\\')
        || key.contains('\0')
        || Path::new(key).is_absolute()
        || key.contains("..")
    {
        return Err(EngineError::BadPayload(format!("bad asset key `{key}`")));
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// The project engine: op log + snapshots + autosave + assets + bundles.
///
/// There is deliberately no `close()`: every mutation is durable before it
/// returns, so dropping the engine *is* a clean shutdown and `kill -9` is
/// just a fast drop.
pub struct Engine {
    dir: PathBuf,
    project: Project,
    /// Replay base: creation state, or the latest snapshot. Never mutated by
    /// apply/undo/redo except through [`Engine::snapshot`].
    base: Project,
    base_seq: u64,
    log: Vec<Op>,
    undone: BTreeSet<u64>,
    next_seq: u64,
    snapshot: Option<Snapshot>,
    snapshot_log_len: usize,
    snapshot_every: usize,
}

impl Engine {
    /// Create a fresh project dir holding `project` (empty log).
    pub fn create(dir: &Path, project: Project) -> Result<Self> {
        fs::create_dir_all(dir)?;
        fs::create_dir_all(assets_dir(dir))?;
        write_atomic(
            &manifest_path(dir),
            serde_json::to_string_pretty(&Manifest {
                version: 0,
                assets: Vec::new(),
            })
            .expect("manifest serializes")
            .as_bytes(),
        )?;
        File::create(ops_path(dir))?;
        write_atomic(
            &project_path(dir),
            serde_json::to_string_pretty(&project)?.as_bytes(),
        )?;
        Ok(Self {
            dir: dir.to_path_buf(),
            project: project.clone(),
            base: project,
            base_seq: 0,
            log: Vec::new(),
            undone: BTreeSet::new(),
            next_seq: 1,
            snapshot: None,
            snapshot_log_len: 0,
            snapshot_every: DEFAULT_SNAPSHOT_EVERY,
        })
    }

    /// Open a project dir. Reads the snapshot (if any) and replays the tail
    /// of the log; assets are never touched (lazy load). Recovery ignores
    /// `project.json` and rebuilds from snapshot + log.
    pub fn open(dir: &Path) -> Result<Self> {
        let snapshot: Option<Snapshot> = fs::read_to_string(snapshot_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let log = read_log(&ops_path(dir))?;
        let next_seq = log.iter().map(|op| op.seq).max().unwrap_or(0) + 1;
        let undone = fold_undone(&log);
        let (base_seq, base): (u64, Project) = match &snapshot {
            Some(s) => (s.seq, s.project.clone()),
            None => {
                let raw = fs::read_to_string(project_path(dir))
                    .map_err(|_| EngineError::NotAProject(dir.to_path_buf()))?;
                let p: Project = serde_json::from_str(&raw)
                    .map_err(|_| EngineError::NotAProject(dir.to_path_buf()))?;
                (0, p)
            }
        };
        let project = rebuild(&base, base_seq, &log, &undone)?;
        let snapshot_log_len = log.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            project,
            base,
            base_seq,
            log,
            undone,
            next_seq,
            snapshot,
            snapshot_log_len,
            snapshot_every: DEFAULT_SNAPSHOT_EVERY,
        })
    }

    /// Number of appended ops after which [`Engine::snapshot`] runs
    /// automatically. `0` disables auto-snapshots (manual only).
    pub fn set_snapshot_every(&mut self, n: usize) {
        self.snapshot_every = n;
    }

    pub fn project(&self) -> &Project {
        &self.project
    }
    pub fn log(&self) -> &[Op] {
        &self.log
    }
    pub fn undone_seqs(&self) -> Vec<u64> {
        self.undone.iter().copied().collect()
    }
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }
    pub fn snapshot_seq(&self) -> Option<u64> {
        self.snapshot.as_ref().map(|s| s.seq)
    }

    /// Live ops in seq order (markers resolved out, undone ops excluded):
    /// the granularity branches compare/merge at.
    pub fn live_ops(&self) -> Vec<&Op> {
        self.log
            .iter()
            .filter(|op| op.kind != OpKind::UndoMarker && !self.undone.contains(&op.seq))
            .collect()
    }

    /// Live ops with `seq` greater than `seq`, for branch compare/merge.
    pub fn ops_since(&self, seq: u64) -> Vec<&Op> {
        self.live_ops()
            .into_iter()
            .filter(|op| op.seq > seq)
            .collect()
    }

    /// Append one op and return its sequence number. The op is validated and
    /// applied first (a bad op never enters the log), then appended to
    /// `ops.jsonl` + fsync (instant autosave), then materialized.
    pub fn apply(
        &mut self,
        actor: &str,
        kind: OpKind,
        target: &str,
        value_json: &str,
    ) -> Result<u64> {
        if !valid_actor(actor) {
            return Err(EngineError::BadActor(actor.to_string()));
        }
        let op = Op {
            seq: self.next_seq,
            actor: actor.to_string(),
            kind,
            target: target.to_string(),
            value_json: value_json.to_string(),
        };
        // Validate before the log is touched: no poison entries, ever.
        let mut staged = self.project.clone();
        apply_op_to_project(&mut staged, &op)?;
        self.append_durable(&op)?;
        self.project = staged;
        self.log.push(op);
        self.next_seq += 1;
        self.after_mutation()?;
        Ok(self.next_seq - 1)
    }

    /// Undo the latest live op; returns its seq. Appends an `UndoMarker`
    /// naming it, so the undo itself is durable and redoable after restart.
    pub fn undo(&mut self) -> Result<u64> {
        let seq = self
            .log
            .iter()
            .rev()
            .find(|op| op.kind != OpKind::UndoMarker && !self.undone.contains(&op.seq))
            .map(|op| op.seq)
            .ok_or(EngineError::NothingToUndo)?;
        let marker = Op {
            seq: self.next_seq,
            actor: "ui".to_string(),
            kind: OpKind::UndoMarker,
            target: seq.to_string(),
            value_json: String::new(),
        };
        self.append_durable(&marker)?;
        self.log.push(marker);
        self.next_seq += 1;
        self.undone.insert(seq);
        self.refresh()?;
        Ok(seq)
    }

    /// Redo the most recently undone op; returns the new marker seq.
    /// Appends a `{"redo": true}` marker that cancels the undo marker.
    pub fn redo(&mut self) -> Result<u64> {
        let seq = self.undone.iter().next_back().copied().ok_or(EngineError::NothingToRedo)?;
        let marker = Op {
            seq: self.next_seq,
            actor: "ui".to_string(),
            kind: OpKind::UndoMarker,
            target: seq.to_string(),
            value_json: "{\"redo\": true}".to_string(),
        };
        self.append_durable(&marker)?;
        self.log.push(marker);
        self.next_seq += 1;
        self.undone.remove(&seq);
        self.refresh()?;
        Ok(self.next_seq - 1)
    }

    /// Take a snapshot at the current tip. Rebuilds replay only later ops.
    pub fn snapshot(&mut self) -> Result<()> {
        let snap = Snapshot {
            seq: self.next_seq - 1,
            project: self.project.clone(),
        };
        write_atomic(
            &snapshot_path(&self.dir),
            serde_json::to_string_pretty(&snap)?.as_bytes(),
        )?;
        self.base = self.project.clone();
        self.base_seq = self.next_seq - 1;
        self.snapshot = Some(snap);
        self.snapshot_log_len = self.log.len();
        Ok(())
    }

    // -- assets (lazy load) -------------------------------------------------

    /// Store an opaque blob (audio, MIDI, preset, frozen plugin state).
    /// Clips reference it through their `source` field; bytes stay on disk
    /// until [`Engine::load_asset`] is called.
    pub fn store_asset(&mut self, key: &str, kind: &str, bytes: &[u8]) -> Result<()> {
        check_asset_key(key)?;
        if kind.is_empty() {
            return Err(EngineError::BadPayload("asset kind is empty".to_string()));
        }
        let dir = assets_dir(&self.dir);
        fs::create_dir_all(&dir)?;
        let path = dir.join(key);
        write_atomic(&path, bytes)?;
        let mut manifest = read_manifest(&self.dir);
        manifest.assets.retain(|e| e.key != key);
        manifest.assets.push(AssetEntry {
            key: key.to_string(),
            kind: kind.to_string(),
            size: bytes.len() as u64,
        });
        manifest.assets.sort_by(|a, b| a.key.cmp(&b.key));
        write_atomic(
            &manifest_path(&self.dir),
            serde_json::to_string_pretty(&manifest)?.as_bytes(),
        )?;
        Ok(())
    }

    /// Read one blob on demand. Opening the project never calls this.
    pub fn load_asset(&self, key: &str) -> Result<Vec<u8>> {
        check_asset_key(key)?;
        fs::read(assets_dir(&self.dir).join(key)).map_err(EngineError::Io)
    }

    /// Asset index without reading any bytes (the lazy-load listing).
    pub fn asset_keys(&self) -> Vec<AssetEntry> {
        read_manifest(&self.dir).assets
    }

    /// Decode every decodable `audio`-kind asset into a render-ready
    /// [`SampleBank`](crate::bounce::SampleBank) for `asset:` clips.
    /// Undecodable blobs are skipped (dangling keys render silence, never
    /// errors); an engine with no audio assets yields an empty bank and
    /// byte-identical procedural renders.
    pub fn sample_bank(&self) -> crate::bounce::SampleBank {
        let mut bank = crate::bounce::SampleBank::empty();
        for entry in read_manifest(&self.dir).assets {
            if entry.kind != ASSET_KIND_AUDIO {
                continue;
            }
            let bytes = match self.load_asset(&entry.key) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if bank.insert_wav(&entry.key, &bytes).is_err() {
                eprintln!("sample_bank: skipping undecodable audio asset {}", entry.key);
            }
        }
        bank
    }

    // -- portable bundle ----------------------------------------------------

    /// Copy this project dir into `dest` as one portable package: project +
    /// log + snapshot + manifest + every asset (audio, presets, MIDI,
    /// automation via the project doc, frozen plugin states as blobs).
    pub fn export_bundle(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            return Err(EngineError::BadPayload(format!(
                "bundle dest exists: {}",
                dest.display()
            )));
        }
        copy_dir_recursive(&self.dir, dest)
    }

    /// Copy a bundle into `dest`, verify the manifest (every entry present,
    /// sizes match), and open it.
    pub fn import_bundle(src: &Path, dest: &Path) -> Result<Self> {
        if dest.exists() {
            return Err(EngineError::BadPayload(format!(
                "import dest exists: {}",
                dest.display()
            )));
        }
        copy_dir_recursive(src, dest)?;
        let engine = Self::open(dest)?;
        let manifest = read_manifest(dest);
        let mut index: BTreeMap<&str, u64> = BTreeMap::new();
        for e in &manifest.assets {
            index.insert(e.key.as_str(), e.size);
        }
        for (key, size) in &index {
            let bytes = fs::read(assets_dir(dest).join(key)).map_err(|_| {
                EngineError::CorruptBundle(format!("bundle asset missing: {key}"))
            })?;
            if bytes.len() as u64 != *size {
                return Err(EngineError::CorruptBundle(format!(
                    "bundle asset size mismatch: {key}"
                )));
            }
        }
        let _ = engine;
        Self::open(dest)
    }

    // -- internals ----------------------------------------------------------

    /// Append one line to the log and fsync: the instant-autosave path.
    /// Afterwards the op survives `kill -9`; `project.json` is best-effort.
    fn append_durable(&self, op: &Op) -> Result<()> {
        let mut f = OpenOptions::new().append(true).open(ops_path(&self.dir))?;
        writeln!(f, "{}", serde_json::to_string(op)?)?;
        f.sync_all()?;
        drop(f);
        // Materialized convenience copy; recovery never depends on it.
        let _ = write_atomic(
            &project_path(&self.dir),
            serde_json::to_string_pretty(&self.project)?.as_bytes(),
        );
        Ok(())
    }

    fn after_mutation(&mut self) -> Result<()> {
        // project.json should reflect the mutation we just applied.
        let _ = write_atomic(
            &project_path(&self.dir),
            serde_json::to_string_pretty(&self.project)?.as_bytes(),
        );
        if self.snapshot_every > 0 && self.log.len() - self.snapshot_log_len >= self.snapshot_every
        {
            self.snapshot()?;
        }
        Ok(())
    }

    /// Re-derive live state from base + log (undo/redo path).
    fn refresh(&mut self) -> Result<()> {
        let base = self.base.clone();
        self.project = rebuild(&base, self.base_seq, &self.log, &self.undone)?;
        self.after_mutation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Unique scratch dir per test (no external crates).
    fn scratch(name: &str) -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "ccez-engine-{}-{}-{}",
            name,
            std::process::id(),
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn track_json(id: &str, name: &str) -> String {
        serde_json::to_string(&Track {
            id: id.to_string(),
            name: name.to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        })
        .expect("track serializes")
    }

    fn clip_json(id: &str, track_id: &str, start: f64) -> String {
        serde_json::to_string(&Clip {
            id: id.to_string(),
            track_id: track_id.to_string(),
            name: id.to_string(),
            start_beats: start,
            length_beats: 4.0,
            kind: crate::model::ClipKind::Midi,
            source: "take:1".to_string(),
        })
        .expect("clip serializes")
    }

    #[test]
    fn apply_assigns_monotonic_seqs_and_mutates() {
        let dir = scratch("seq");
        let mut e = Engine::create(&dir, Project::sample()).expect("create");
        let s1 = e
            .apply("ui", OpKind::TempoSet, "tempo", "128.0")
            .expect("tempo");
        let s2 = e
            .apply("mcp", OpKind::TrackAdded, "trk_x", &track_json("trk_x", "X"))
            .expect("track");
        assert_eq!((s1, s2), (1, 2));
        assert_eq!(e.project().tempo, 128.0);
        assert!(e.project().tracks.iter().any(|t| t.id == "trk_x"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_ops_never_enter_the_log() {
        let dir = scratch("bad");
        let mut e = Engine::create(&dir, Project::sample()).expect("create");
        assert!(matches!(
            e.apply("human", OpKind::TempoSet, "tempo", "1.0"),
            Err(EngineError::BadActor(_))
        ));
        assert!(matches!(
            e.apply("ai:", OpKind::TempoSet, "tempo", "1.0"),
            Err(EngineError::BadActor(_))
        ));
        assert!(matches!(
            e.apply("ui", OpKind::TempoSet, "tempo", " allegro "),
            Err(EngineError::BadPayload(_))
        ));
        assert!(matches!(
            e.apply("ui", OpKind::ParamSet, "nope:volume", "0.5"),
            Err(EngineError::UnknownTarget(_))
        ));
        assert!(e.log().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_op_kind_applies() {
        let dir = scratch("kinds");
        let mut e = Engine::create(&dir, Project::new("p", "P")).expect("create");
        e.apply("ui", OpKind::TrackAdded, "t1", &track_json("t1", "T"))
            .expect("track");
        e.apply("script:seed", OpKind::ClipAdded, "c1", &clip_json("c1", "t1", 0.0))
            .expect("clip");
        e.apply("ai:jam", OpKind::ClipMoved, "c1", "{\"startBeats\": 8}")
            .expect("move");
        e.apply("ui", OpKind::ParamSet, "t1:volume", "0.5")
            .expect("param");
        e.apply("ui", OpKind::TempoSet, "tempo", "100")
            .expect("tempo");
        let p = e.project();
        assert_eq!(p.clips.iter().find(|c| c.id == "c1").expect("clip").start_beats, 8.0);
        assert_eq!(p.tracks.iter().find(|t| t.id == "t1").expect("track").volume, 0.5);
        assert_eq!(p.tempo, 100.0);
        assert_eq!(e.live_ops().len(), 5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn automation_point_set_creates_upserts_and_orders() {
        let dir = scratch("auto");
        let mut e = Engine::create(&dir, Project::new("p", "P")).expect("create");
        // Missing lane without node/param refuses.
        assert!(matches!(
            e.apply("ui", OpKind::AutomationPointSet, "lane_a", "{\"beat\": 0, \"value\": 0.5}"),
            Err(EngineError::UnknownTarget(_))
        ));
        // Missing lane with address creates it.
        e.apply(
            "ui",
            OpKind::AutomationPointSet,
            "lane_a",
            "{\"beat\": 4, \"value\": 0.5, \"node\": \"t1\", \"param\": \"volume\"}",
        )
        .expect("create lane");
        // Out-of-order insert keeps ascending beats; exact beat replaces.
        e.apply("ui", OpKind::AutomationPointSet, "lane_a", "{\"beat\": 0, \"value\": 0.0}")
            .expect("insert");
        e.apply("ui", OpKind::AutomationPointSet, "lane_a", "{\"beat\": 4, \"value\": 0.9}")
            .expect("replace");
        let lane = e.project().automation.iter().find(|l| l.id == "lane_a").expect("lane");
        assert_eq!(lane.target.node, "t1");
        assert_eq!(lane.target.param, "volume");
        let beats: Vec<f64> = lane.points.iter().map(|p| p.beat).collect();
        assert_eq!(beats, vec![0.0, 4.0]);
        assert_eq!(lane.points[1].value, 0.9);
        // Bad payloads refuse: junk, missing value, negative beat.
        assert!(matches!(
            e.apply("ui", OpKind::AutomationPointSet, "lane_a", "forte"),
            Err(EngineError::BadPayload(_))
        ));
        assert!(matches!(
            e.apply("ui", OpKind::AutomationPointSet, "lane_a", "{\"beat\": 8}"),
            Err(EngineError::BadPayload(_))
        ));
        assert!(matches!(
            e.apply("ui", OpKind::AutomationPointSet, "lane_a", "{\"beat\": -1, \"value\": 0.5}"),
            Err(EngineError::BadPayload(_))
        ));
        assert_eq!(e.live_ops().len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_redo_round_trip_in_memory() {
        let dir = scratch("undoredo");
        let mut e = Engine::create(&dir, Project::sample()).expect("create");
        e.apply("ui", OpKind::TempoSet, "tempo", "140.0").expect("t");
        assert_eq!(e.project().tempo, 140.0);
        assert_eq!(e.undo().expect("undo"), 1);
        assert_eq!(e.project().tempo, 120.0);
        e.redo().expect("redo");
        assert_eq!(e.project().tempo, 140.0);
        // Redo twice must not duplicate: nothing left to redo.
        assert!(matches!(e.redo(), Err(EngineError::NothingToRedo)));
        // Undo past the bottom errors instead of corrupting.
        e.undo().expect("undo again");
        assert!(matches!(e.undo(), Err(EngineError::NothingToUndo)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_history_survives_restart() {
        let dir = scratch("xsession");
        let before = {
            let mut e = Engine::create(&dir, Project::sample()).expect("create");
            e.apply("ui", OpKind::TempoSet, "tempo", "140.0").expect("t");
            e.apply("ui", OpKind::ParamSet, "trk_music:volume", "0.1").expect("p");
            e.undo().expect("undo param");
            e.project().clone()
        };
        // Reopen = post-crash process. The UndoMarker persisted, so the
        // param stays undone while tempo stays applied — and redo works.
        let mut e = Engine::open(&dir).expect("reopen");
        assert_eq!(e.project(), &before);
        assert_eq!(e.project().tempo, 140.0);
        assert_eq!(e.undone_seqs().len(), 1);
        e.redo().expect("cross-session redo");
        assert_eq!(
            e.project().tracks.iter().find(|t| t.id == "trk_music").expect("track").volume,
            0.1
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kill_9_loses_nothing() {
        let dir = scratch("kill9");
        let expected = {
            let mut e = Engine::create(&dir, Project::sample()).expect("create");
            e.apply("ui", OpKind::TempoSet, "tempo", "132.0").expect("t");
            e.apply("ui", OpKind::TrackAdded, "trk_k", &track_json("trk_k", "K"))
                .expect("track");
            e.apply("ui", OpKind::ClipAdded, "clip_k", &clip_json("clip_k", "trk_k", 2.0))
                .expect("clip");
            e.snapshot().expect("snap");
            e.apply("ui", OpKind::ClipMoved, "clip_k", "6.0").expect("move");
            e.project().clone()
            // No close(), no shutdown handshake: `drop` here IS the kill -9.
        };
        // Simulate a crash torn mid-autosave: the convenience copy is gone.
        fs::remove_file(project_path(&dir)).expect("delete project.json");
        let e = Engine::open(&dir).expect("recover");
        assert_eq!(e.project(), &expected);
        assert_eq!(e.log().len(), 4); // snapshot is not a log entry
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn torn_tail_line_is_ignored_on_recovery() {
        let dir = scratch("torn");
        let mut e = Engine::create(&dir, Project::sample()).expect("create");
        e.apply("ui", OpKind::TempoSet, "tempo", "111.0").expect("t");
        drop(e);
        // Half a line as left by a crash mid-append.
        let mut f = OpenOptions::new().append(true).open(ops_path(&dir)).expect("open log");
        f.write_all(b"{\"seq\": 999, \"actor\": \"ui\", \"kind\": \"Tempo").expect("torn write");
        drop(f);
        let e = Engine::open(&dir).expect("recover past torn tail");
        assert_eq!(e.project().tempo, 111.0);
        assert_eq!(e.log().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_bounds_replay() {
        let dir = scratch("snap");
        let mut e = Engine::create(&dir, Project::new("p", "P")).expect("create");
        e.set_snapshot_every(0); // manual snapshots for this test
        for i in 0..10 {
            e.apply("ui", OpKind::TempoSet, "tempo", &format!("{}", 100 + i))
                .expect("t");
        }
        e.snapshot().expect("snap");
        assert_eq!(e.snapshot_seq(), Some(10));
        for i in 10..13 {
            e.apply("ui", OpKind::TempoSet, "tempo", &format!("{}", 100 + i))
                .expect("t");
        }
        // Reopen replays only the 3 post-snapshot ops onto the snapshot.
        let e2 = Engine::open(&dir).expect("reopen");
        assert_eq!(e2.project().tempo, 112.0);
        assert_eq!(e2.snapshot_seq(), Some(10));
        assert_eq!(e.project(), e2.project());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn assets_are_lazy_and_bundles_round_trip() {
        let dir = scratch("bundle");
        let mut e = Engine::create(&dir, Project::sample()).expect("create");
        let drums = vec![0u8, 1, 2, 3, 4, 5];
        let state = vec![9u8; 32];
        e.store_asset("drums.wav", ASSET_KIND_AUDIO, &drums).expect("audio");
        e.store_asset("take1.mid", ASSET_KIND_MIDI, &[1, 2, 3]).expect("midi");
        e.store_asset("keys.preset", ASSET_KIND_PRESET, &[7]).expect("preset");
        e.store_asset("verb.frozen", ASSET_KIND_PLUGIN, &state).expect("plugin");
        e.apply("ui", OpKind::TempoSet, "tempo", "125.0").expect("t");
        assert_eq!(e.asset_keys().len(), 4);
        // Frozen means verbatim: what was stored is what comes back.
        assert_eq!(e.load_asset("verb.frozen").expect("load"), state);
        assert!(e.load_asset("../escape").is_err());
        assert!(e.load_asset("missing.wav").is_err());

        let dest = scratch("bundle-out");
        e.export_bundle(&dest).expect("export");
        // Tamper check happens on import; here the pristine copy must open
        // identical, then serve assets on demand.
        let e2 = Engine::import_bundle(&dest, &scratch("bundle-in")).expect("import");
        assert_eq!(e2.project(), e.project());
        assert_eq!(e2.load_asset("drums.wav").expect("load"), drums);
        assert_eq!(e2.asset_keys().len(), 4);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn corrupt_bundle_is_rejected() {
        let dir = scratch("badsrc");
        let mut e = Engine::create(&dir, Project::sample()).expect("create");
        e.store_asset("a.wav", ASSET_KIND_AUDIO, &[1, 2, 3]).expect("store");
        let dest = scratch("baddest");
        e.export_bundle(&dest).expect("export");
        fs::remove_file(dest.join("assets").join("a.wav")).expect("drop asset");
        assert!(matches!(
            Engine::import_bundle(&dest, &scratch("badin")),
            Err(EngineError::CorruptBundle(_))
        ));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn actor_rules_match_the_frozen_contract() {
        for good in ["ui", "mcp", "ai:sidecar", "ai:x:y", "script:seed"] {
            assert!(valid_actor(good), "{good}");
        }
        for bad in ["", "human", "ai:", "script:", "UI", "ai"] {
            assert!(!valid_actor(bad), "{bad}");
        }
    }
}
