//! Track C: device-chain helpers (oversampling flag, wet/dry mix, snapshots).
//!
//! Teaching note: an insert chain is just an ordered list — the frozen
//! [`Track`](crate::model::Track) already carries it as `device_ids`, and
//! each device is a frozen [`Node`](crate::model::Node) with a `params`
//! list. This module adds *conventions* on top of those shapes, never new
//! shapes:
//!
//! - [`OVERSAMPLE_PARAM`] (`"oversampling"`): 0.0 = off, 1.0 = on.
//! - [`WET_PARAM`] / [`DRY_PARAM`] (`"wet"` / `"dry"`): per-device mix
//!   gains in 0.0..=1.0. A bypassed-feeling device is `wet = 0, dry = 1`.
//! - [`ChainSnapshot`]: the chain's order plus every device's full params,
//!   as plain JSON. Save before a destructive tweak, restore to undo it —
//!   a poor-man's preset scoped to one track's chain.
//!
//! Because everything lives inside the frozen `Param` shape, the typegen
//! drift gate (`bun run typegen -- --check`) is unaffected: this module
//! adds no IPC or project-schema surface.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Node, NodeKind, Param, Project};

/// Device param flag for 2x oversampling. Stored 0.0 (off) / 1.0 (on);
/// missing means off.
pub const OVERSAMPLE_PARAM: &str = "oversampling";
/// Device param: wet (processed) gain, 0.0..=1.0. Missing means 1.0.
pub const WET_PARAM: &str = "wet";
/// Device param: dry (unprocessed) gain, 0.0..=1.0. Missing means 0.0.
pub const DRY_PARAM: &str = "dry";
/// Oversampling factor when the flag is on (2x: the cheap, common case).
pub const OVERSAMPLE_FACTOR: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    UnknownTrack(String),
    UnknownDevice(String),
    BadSnapshot(String),
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTrack(t) => write!(f, "unknown chain track `{t}`"),
            Self::UnknownDevice(d) => write!(f, "unknown chain device `{d}`"),
            Self::BadSnapshot(m) => write!(f, "bad chain snapshot: {m}"),
        }
    }
}

impl std::error::Error for ChainError {}

pub type Result<T> = std::result::Result<T, ChainError>;

/// One device's saved state: identity plus its full params (values *and*
/// ranges, so restore is exact).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceState {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub params: Vec<Param>,
}

/// A track's whole insert chain, frozen to JSON: device order plus every
/// device's state. Restoring re-applies both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainSnapshot {
    pub track_id: String,
    pub order: Vec<String>,
    pub devices: Vec<DeviceState>,
}

/// Device ids of `track_id` in chain order.
pub fn chain_order(project: &Project, track_id: &str) -> Result<Vec<String>> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| ChainError::UnknownTrack(track_id.to_string()))?;
    Ok(track.device_ids.clone())
}

/// Chain devices of `track_id` in chain order. Ids with no matching device
/// node are skipped (the graph treats them the same way: endpoints without
/// state render as pass-through).
pub fn chain_devices<'a>(project: &'a Project, track_id: &str) -> Result<Vec<&'a Node>> {
    let order = chain_order(project, track_id)?;
    let by_id: BTreeMap<&str, &Node> = project
        .devices
        .iter()
        .map(|d| (d.id.as_str(), d))
        .collect();
    Ok(order
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).copied())
        .collect())
}

/// `true` when the device's oversampling flag reads >= 0.5 (missing = off).
pub fn is_oversampled(device: &Node) -> bool {
    param_value(device, OVERSAMPLE_PARAM).unwrap_or(0.0) >= 0.5
}

/// Effective render rate factor for the device: 2x when flagged, else 1x.
pub fn oversample_factor(device: &Node) -> u32 {
    if is_oversampled(device) {
        OVERSAMPLE_FACTOR
    } else {
        1
    }
}

/// Set the oversampling flag, inserting the param (0.0..=1.0) when absent.
pub fn set_oversampled(device: &mut Node, on: bool) {
    upsert_param(device, OVERSAMPLE_PARAM, "Oversampling", if on { 1.0 } else { 0.0 }, 0.0, 1.0);
}

/// `(wet, dry)` mix gains for the device (defaults 1.0 / 0.0 when absent).
pub fn wet_dry(device: &Node) -> (f32, f32) {
    let wet = param_value(device, WET_PARAM).unwrap_or(1.0) as f32;
    let dry = param_value(device, DRY_PARAM).unwrap_or(0.0) as f32;
    (wet, dry)
}

/// Set the wet/dry mix, clamped to 0.0..=1.0, inserting params when absent.
pub fn set_wet_dry(device: &mut Node, wet: f32, dry: f32) {
    upsert_param(
        device,
        WET_PARAM,
        "Wet",
        wet.clamp(0.0, 1.0) as f64,
        0.0,
        1.0,
    );
    upsert_param(
        device,
        DRY_PARAM,
        "Dry",
        dry.clamp(0.0, 1.0) as f64,
        0.0,
        1.0,
    );
}

/// Mix one sample: `dry * dry_in + wet * wet_in`. Pure function of its
/// inputs — the same math the realtime insert applies per frame, so tests
/// prove the DSP contract without hardware.
pub fn apply_wet_dry(dry_in: f32, wet_in: f32, wet: f32, dry: f32) -> f32 {
    dry_in * dry + wet_in * wet
}

/// Save `track_id`'s chain: its order plus every listed device's state.
pub fn save_snapshot(project: &Project, track_id: &str) -> Result<ChainSnapshot> {
    let order = chain_order(project, track_id)?;
    let by_id: BTreeMap<&str, &Node> = project
        .devices
        .iter()
        .map(|d| (d.id.as_str(), d))
        .collect();
    let mut devices = Vec::new();
    for id in &order {
        let dev = by_id
            .get(id.as_str())
            .ok_or_else(|| ChainError::UnknownDevice(id.clone()))?;
        devices.push(DeviceState {
            id: dev.id.clone(),
            name: dev.name.clone(),
            kind: dev.kind.clone(),
            params: dev.params.clone(),
        });
    }
    Ok(ChainSnapshot {
        track_id: track_id.to_string(),
        order,
        devices,
    })
}

/// Restore a snapshot: device params/names are replaced wholesale (missing
/// devices are re-created as [`NodeKind::Device`]), then the track's order
/// is reset to the snapshot's. Order entries with no state are an error —
/// restoring must never silently drop an insert.
pub fn restore_snapshot(project: &mut Project, snapshot: &ChainSnapshot) -> Result<()> {
    if snapshot.order.len() != snapshot.devices.len()
        || !snapshot
            .order
            .iter()
            .zip(snapshot.devices.iter())
            .all(|(o, d)| o == &d.id)
    {
        return Err(ChainError::BadSnapshot(
            "order and devices must agree element-wise".to_string(),
        ));
    }
    for state in &snapshot.devices {
        match project.devices.iter_mut().find(|d| d.id == state.id) {
            Some(dev) => {
                dev.name = state.name.clone();
                dev.kind = state.kind.clone();
                dev.params = state.params.clone();
            }
            None => project.devices.push(Node {
                id: state.id.clone(),
                kind: state.kind.clone(),
                name: state.name.clone(),
                params: state.params.clone(),
            }),
        }
    }
    let track = project
        .tracks
        .iter_mut()
        .find(|t| t.id == snapshot.track_id)
        .ok_or_else(|| ChainError::UnknownTrack(snapshot.track_id.clone()))?;
    track.device_ids = snapshot.order.clone();
    Ok(())
}

/// Serialize a snapshot to JSON (what a preset file / op payload carries).
pub fn snapshot_to_json(snapshot: &ChainSnapshot) -> Result<String> {
    serde_json::to_string(snapshot).map_err(|e| ChainError::BadSnapshot(e.to_string()))
}

/// Parse a snapshot back from JSON.
pub fn snapshot_from_json(json: &str) -> Result<ChainSnapshot> {
    serde_json::from_str(json).map_err(|e| ChainError::BadSnapshot(e.to_string()))
}

// -- internals ---------------------------------------------------------------

fn param_value(device: &Node, id: &str) -> Option<f64> {
    device.params.iter().find(|p| p.id == id).map(|p| p.value)
}

fn upsert_param(device: &mut Node, id: &str, label: &str, value: f64, min: f64, max: f64) {
    match device.params.iter_mut().find(|p| p.id == id) {
        Some(p) => {
            p.value = value.clamp(p.min, p.max);
        }
        None => device.params.push(Param {
            id: id.to_string(),
            label: label.to_string(),
            value: value.clamp(min, max),
            min,
            max,
            default: value.clamp(min, max),
            unit: String::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    fn device(id: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Device,
            name: id.to_string(),
            params: vec![Param {
                id: "gain".to_string(),
                label: "Gain".to_string(),
                value: 0.8,
                min: 0.0,
                max: 1.5,
                default: 1.0,
                unit: String::new(),
            }],
        }
    }

    fn project() -> Project {
        let mut p = Project::new("p", "Chain");
        p.tracks.push(Track {
            id: "trk".to_string(),
            name: "Lead".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec!["dev_a".to_string(), "dev_b".to_string()],
        });
        p.devices.push(device("dev_a"));
        p.devices.push(device("dev_b"));
        p
    }

    #[test]
    fn oversampling_flag_defaults_off_and_toggles() {
        let mut dev = device("d");
        assert!(!is_oversampled(&dev));
        assert_eq!(oversample_factor(&dev), 1);
        set_oversampled(&mut dev, true);
        assert!(is_oversampled(&dev));
        assert_eq!(oversample_factor(&dev), OVERSAMPLE_FACTOR);
        // Survives the frozen model JSON round-trip (it is just a Param).
        let json = serde_json::to_string(&dev).expect("serialize");
        let back: Node = serde_json::from_str(&json).expect("deserialize");
        assert!(is_oversampled(&back));
        set_oversampled(&mut dev, false);
        assert!(!is_oversampled(&dev));
    }

    #[test]
    fn wet_dry_mix_math_and_clamping() {
        let mut dev = device("d");
        // Defaults: fully wet insert.
        assert_eq!(wet_dry(&dev), (1.0, 0.0));
        assert_eq!(apply_wet_dry(0.5, 1.0, 1.0, 0.0), 1.0);
        // Equal mix averages the two paths.
        set_wet_dry(&mut dev, 0.5, 0.5);
        assert_eq!(apply_wet_dry(1.0, 1.0, 0.5, 0.5), 1.0);
        assert_eq!(apply_wet_dry(0.0, 1.0, 0.5, 0.5), 0.5);
        // Bypass feel: wet shut, dry open.
        set_wet_dry(&mut dev, 0.0, 1.0);
        assert_eq!(apply_wet_dry(0.7, 0.1, 0.0, 1.0), 0.7);
        // Out-of-range inputs clamp instead of erroring.
        set_wet_dry(&mut dev, 99.0, -99.0);
        assert_eq!(wet_dry(&dev), (1.0, 0.0));
    }

    #[test]
    fn chain_snapshot_save_restore_round_trip() {
        let mut p = project();
        // Season the chain: flags, mixes, custom gains.
        set_oversampled(&mut p.devices[0], true);
        set_wet_dry(&mut p.devices[0], 0.7, 0.3);
        set_wet_dry(&mut p.devices[1], 0.0, 1.0);
        p.devices[1].params[0].value = 0.25;

        let snap = save_snapshot(&p, "trk").expect("save");
        assert_eq!(snap.order, vec!["dev_a".to_string(), "dev_b".to_string()]);
        // JSON is the preset-file shape: it must round-trip exactly.
        let json = snapshot_to_json(&snap).expect("to json");
        assert_eq!(snapshot_from_json(&json).expect("from json"), snap);

        // Destructive tweak: reorder, retune, drop a device from the track.
        p.tracks[0].device_ids = vec!["dev_b".to_string()];
        set_oversampled(&mut p.devices[0], false);
        set_wet_dry(&mut p.devices[1], 1.0, 0.0);
        p.devices.remove(0);
        assert_ne!(save_snapshot(&p, "trk").expect("resave").order, snap.order);

        // Restore: order, membership, and every param value come back.
        restore_snapshot(&mut p, &snap).expect("restore");
        assert_eq!(p.tracks[0].device_ids, snap.order);
        let devs = chain_devices(&p, "trk").expect("devices");
        assert_eq!(devs.len(), 2);
        assert!(is_oversampled(devs[0]));
        assert_eq!(wet_dry(devs[0]), (0.7, 0.3));
        assert_eq!(wet_dry(devs[1]), (0.0, 1.0));
        assert_eq!(save_snapshot(&p, "trk").expect("resave"), snap);
    }

    #[test]
    fn snapshot_errors_are_clean() {
        let p = project();
        assert!(matches!(
            save_snapshot(&p, "nope"),
            Err(ChainError::UnknownTrack(_))
        ));
        assert!(matches!(
            snapshot_from_json("{bad"),
            Err(ChainError::BadSnapshot(_))
        ));
        let mut bad = save_snapshot(&p, "trk").expect("save");
        bad.order.push("ghost".to_string());
        let mut q = project();
        assert!(matches!(
            restore_snapshot(&mut q, &bad),
            Err(ChainError::BadSnapshot(_))
        ));
    }
}
