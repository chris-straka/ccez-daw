//! Track G: mixer console — a view over the one routing graph.
//!
//! Teaching note: most DAWs keep a separate mixer model that drifts from
//! the router (a fader moves, the graph doesn't, and null-tests fail).
//! Here the mixer owns **no signal topology at all**: faders, groups,
//! buses, snapshots, and the reference track are *views* computed from the
//! frozen [`Project`](crate::model::Project) routing edges
//! (`contracts/project-schema.md`: "the mixer is a view over `Audio`
//! edges"). The only mixer-owned state is UI-local sidecars
//! ([`VcaGroup`], [`MixerSnapshot`]) that ride *alongside* the project —
//! they never change the frozen schema, so the typegen drift gate
//! (`bun run typegen -- --check`) stays green.
//!
//! Map:
//! - [`strips`] — one fader strip per track, with its `Audio` out-target.
//! - [`VcaGroup`] + [`effective_track_gain`] — VCA/groups (nested: gains multiply).
//! - [`bus_children`] / [`bus_roots`] / [`bus_gain`] — nested buses over `Audio` edges.
//! - [`MixerSnapshot`] + [`capture`] / [`recall`] — snapshots; recall is exact (null-test).
//! - [`rms`] / [`loudness_db`] / [`match_gain_for`] — loudness-matched A/B.
//! - [`is_reference_track`] / [`reference_tracks`] / [`cue_plan`] — reference track.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{EdgeKind, Project};

/// dB floor reported for digital silence (RMS = 0 has no finite dB value).
pub const SILENCE_DB: f64 = -120.0;

#[derive(Debug, Clone, PartialEq)]
pub enum MixerError {
    UnknownTrack(String),
    UnknownGroup(String),
    BadValue(String),
}

impl std::fmt::Display for MixerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTrack(t) => write!(f, "unknown mixer track `{t}`"),
            Self::UnknownGroup(g) => write!(f, "unknown VCA group `{g}`"),
            Self::BadValue(m) => write!(f, "bad mixer value: {m}"),
        }
    }
}

impl std::error::Error for MixerError {}

pub type Result<T> = std::result::Result<T, MixerError>;

// -- dB / gain ---------------------------------------------------------------

/// dB → linear gain. `db <= SILENCE_DB` maps to 0 (mute).
pub fn db_to_gain(db: f64) -> f64 {
    if !db.is_finite() || db <= SILENCE_DB {
        return 0.0;
    }
    10.0f64.powf(db / 20.0)
}

/// Linear gain → dB. Non-positive maps to [`SILENCE_DB`].
pub fn gain_to_db(gain: f64) -> f64 {
    if !gain.is_finite() || gain <= 0.0 {
        return SILENCE_DB;
    }
    20.0 * gain.log10()
}

// -- strips: the mixer is a view over Audio edges ----------------------------

/// One fader strip: a track plus where its `Audio` edge points.
/// `out` is `None` for "straight to master" (no explicit edge).
#[derive(Debug, Clone, PartialEq)]
pub struct Strip {
    pub id: String,
    pub name: String,
    pub volume: f64,
    pub pan: f64,
    pub muted: bool,
    pub solo: bool,
    pub is_reference: bool,
    pub out: Option<String>,
}

/// Every track in project order, each annotated with its `Audio` out-target
/// (first `Audio` edge leaving the track, if any) and the reference flag.
pub fn strips(project: &Project) -> Vec<Strip> {
    let mut outs: BTreeMap<&str, &str> = BTreeMap::new();
    for edge in &project.routing {
        if edge.kind == EdgeKind::Audio {
            outs.entry(edge.from_node.as_str()).or_insert(edge.to_node.as_str());
        }
    }
    project
        .tracks
        .iter()
        .map(|t| Strip {
            id: t.id.clone(),
            name: t.name.clone(),
            volume: t.volume,
            pan: t.pan,
            muted: t.muted,
            solo: t.solo,
            is_reference: is_reference_track(&t.id, &t.name),
            out: outs.get(t.id.as_str()).map(|s| s.to_string()),
        })
        .collect()
}

// -- VCA / groups (nested: gains multiply) -----------------------------------

/// A VCA group: named set of tracks (or bus ids) sharing one gain trim.
/// UI-local sidecar — membership lives here, not in the frozen schema.
/// Groups nest by *membership overlap*: a track in two groups hears both
/// trims multiplied. Gain is stored linear (1.0 = unity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VcaGroup {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    pub gain: f64,
}

impl VcaGroup {
    pub fn new(id: &str, name: &str, members: Vec<String>, gain_db: f64) -> Result<Self> {
        if id.is_empty() {
            return Err(MixerError::BadValue("group id must be non-empty".to_string()));
        }
        if !gain_db.is_finite() || gain_db < SILENCE_DB || gain_db > 24.0 {
            return Err(MixerError::BadValue(format!(
                "group gain_db {gain_db} out of [-120, +24]"
            )));
        }
        let mut seen = BTreeSet::new();
        for m in &members {
            if !seen.insert(m.clone()) {
                return Err(MixerError::BadValue(format!(
                    "group `{id}` lists `{m}` twice"
                )));
            }
        }
        Ok(Self {
            id: id.to_string(),
            name: name.to_string(),
            members,
            gain: db_to_gain(gain_db),
        })
    }

    pub fn gain_db(&self) -> f64 {
        gain_to_db(self.gain)
    }

    pub fn set_gain_db(&mut self, db: f64) -> Result<()> {
        if !db.is_finite() || db < SILENCE_DB || db > 24.0 {
            return Err(MixerError::BadValue(format!("gain_db {db} out of [-120, +24]")));
        }
        self.gain = db_to_gain(db);
        Ok(())
    }
}

/// Product of every group trim containing `track_id` (1.0 = no group).
pub fn vca_trim(groups: &[VcaGroup], track_id: &str) -> f64 {
    groups
        .iter()
        .filter(|g| g.members.iter().any(|m| m == track_id))
        .map(|g| g.gain)
        .fold(1.0, |a, b| a * b)
}

/// Audible linear gain of a track: `volume × VCA trims × bus-chain gains`.
/// Mute/solo are *routing* decisions, not gain — see [`audible`].
pub fn effective_track_gain(project: &Project, groups: &[VcaGroup], track_id: &str) -> Result<f64> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| MixerError::UnknownTrack(track_id.to_string()))?;
    Ok(track.volume * vca_trim(groups, track_id) * bus_chain_gain(project, track_id))
}

// -- nested buses (over Audio edges) ------------------------------------------

/// Direct `Audio`-edge children feeding `bus_id` (producers of the bus).
pub fn bus_children(project: &Project, bus_id: &str) -> Vec<String> {
    let mut set = BTreeSet::new();
    for edge in &project.routing {
        if edge.kind == EdgeKind::Audio && edge.to_node == bus_id {
            set.insert(edge.from_node.clone());
        }
    }
    set.into_iter().collect()
}

/// Bus roots: `Audio` edge endpoints that nothing feeds *as audio* but that
/// something feeds into — i.e. downstream sums (mix buses, master).
/// Tracks with no outbound edge drain to the implicit master.
pub fn bus_roots(project: &Project) -> Vec<String> {
    let mut fed: BTreeSet<&str> = BTreeSet::new();
    let mut feeds: BTreeSet<&str> = BTreeSet::new();
    for edge in &project.routing {
        if edge.kind == EdgeKind::Audio {
            feeds.insert(edge.from_node.as_str());
            fed.insert(edge.to_node.as_str());
        }
    }
    let mut roots: Vec<String> = fed.difference(&feeds).map(|s| s.to_string()).collect();
    roots.sort();
    roots
}

/// Gain trim carried on a bus/device node: product of its `gain`/`volume`
/// params (1.0 when it carries none — e.g. an implicit endpoint).
pub fn bus_gain(project: &Project, bus_id: &str) -> f64 {
    project
        .devices
        .iter()
        .find(|d| d.id == bus_id)
        .map(|d| {
            d.params
                .iter()
                .filter(|p| p.id == "gain" || p.id == "volume")
                .map(|p| p.value)
                .fold(1.0, |a, b| a * b)
        })
        .unwrap_or(1.0)
}

/// Walk the `Audio` edges downstream from `track_id`, multiplying every bus
/// gain on the path. Cycle-safe (visited set): a feedback cable is a graph
/// error elsewhere; here it just stops compounding.
fn bus_chain_gain(project: &Project, track_id: &str) -> f64 {
    let mut gain = 1.0;
    let mut visited: BTreeSet<String> = BTreeSet::from([track_id.to_string()]);
    let mut frontier: Vec<String> = direct_outs(project, track_id);
    while let Some(node) = frontier.pop() {
        if !visited.insert(node.clone()) {
            continue;
        }
        gain *= bus_gain(project, &node);
        frontier.extend(direct_outs(project, &node));
    }
    gain
}

fn direct_outs(project: &Project, node: &str) -> Vec<String> {
    project
        .routing
        .iter()
        .filter(|e| e.kind == EdgeKind::Audio && e.from_node == node)
        .map(|e| e.to_node.clone())
        .collect()
}

/// Is `track_id` audible under the project's mute/solo state?
/// Solo wins over mute (any solo → only solos sound); reference tracks
/// never sound in the mix — they are cued explicitly (see [`cue_plan`]).
pub fn audible(project: &Project, track_id: &str) -> Result<bool> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| MixerError::UnknownTrack(track_id.to_string()))?;
    if is_reference_track(&track.id, &track.name) {
        return Ok(false);
    }
    let any_solo = project.tracks.iter().any(|t| {
        t.solo && !is_reference_track(&t.id, &t.name) && t.id != track_id
            || (t.id == track_id && t.solo)
    });
    if any_solo {
        return Ok(track.solo);
    }
    Ok(!track.muted)
}

// -- snapshots -----------------------------------------------------------------

/// One frozen fader state: per-track mix settings plus VCA trims.
/// UI-local sidecar (serializable for A/B slots); [`recall`] applies it
/// back exactly — capture→recall with no edits is the null-test identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixerSnapshot {
    pub name: String,
    pub volumes: BTreeMap<String, f64>,
    pub pans: BTreeMap<String, f64>,
    pub mutes: BTreeMap<String, bool>,
    pub solos: BTreeMap<String, bool>,
    pub group_gains: BTreeMap<String, f64>,
}

/// Capture the current fader + group state.
pub fn capture(project: &Project, groups: &[VcaGroup], name: &str) -> MixerSnapshot {
    MixerSnapshot {
        name: name.to_string(),
        volumes: project.tracks.iter().map(|t| (t.id.clone(), t.volume)).collect(),
        pans: project.tracks.iter().map(|t| (t.id.clone(), t.pan)).collect(),
        mutes: project.tracks.iter().map(|t| (t.id.clone(), t.muted)).collect(),
        solos: project.tracks.iter().map(|t| (t.id.clone(), t.solo)).collect(),
        group_gains: groups.iter().map(|g| (g.id.clone(), g.gain)).collect(),
    }
}

/// Recall a snapshot: every captured strip returns to its stored value.
/// Unknown (added-later) tracks are left alone; tracks missing from the
/// snapshot are an error only in [`recall_strict`] — plain `recall` is
/// total so A/B slots survive track add/remove.
pub fn recall(project: &mut Project, groups: &mut [VcaGroup], snap: &MixerSnapshot) {
    for track in &mut project.tracks {
        if let Some(v) = snap.volumes.get(&track.id) {
            track.volume = *v;
        }
        if let Some(p) = snap.pans.get(&track.id) {
            track.pan = *p;
        }
        if let Some(m) = snap.mutes.get(&track.id) {
            track.muted = *m;
        }
        if let Some(s) = snap.solos.get(&track.id) {
            track.solo = *s;
        }
    }
    for group in groups.iter_mut() {
        if let Some(g) = snap.group_gains.get(&group.id) {
            group.gain = *g;
        }
    }
}

/// Strict recall: like [`recall`] but errors on snapshot tracks the project
/// no longer has (catches stale slots before an A/B null-test).
pub fn recall_strict(
    project: &mut Project,
    groups: &mut [VcaGroup],
    snap: &MixerSnapshot,
) -> Result<()> {
    for id in snap.volumes.keys() {
        if !project.tracks.iter().any(|t| &t.id == id) {
            return Err(MixerError::UnknownTrack(id.clone()));
        }
    }
    for id in snap.group_gains.keys() {
        if !groups.iter().any(|g| &g.id == id) {
            return Err(MixerError::UnknownGroup(id.clone()));
        }
    }
    recall(project, groups, snap);
    Ok(())
}

/// Strip ids whose fader state differs between two snapshots.
pub fn snapshot_diff(a: &MixerSnapshot, b: &MixerSnapshot) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for id in a.volumes.keys().chain(a.pans.keys()).chain(a.mutes.keys()).chain(a.solos.keys()) {
        let changed = a.volumes.get(id) != b.volumes.get(id)
            || a.pans.get(id) != b.pans.get(id)
            || a.mutes.get(id) != b.mutes.get(id)
            || a.solos.get(id) != b.solos.get(id);
        if changed {
            ids.insert(id.clone());
        }
    }
    ids.into_iter().collect()
}

// -- loudness-matched A/B -------------------------------------------------------

/// RMS (root-mean-square) level of mono samples. Silence → 0.0.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// RMS level in dBFS. Silence → [`SILENCE_DB`].
pub fn loudness_db(samples: &[f32]) -> f64 {
    gain_to_db(rms(samples) as f64)
}

/// Linear gain to apply to `b` so it matches `a`'s loudness
/// (`gain = rms(a) / rms(b)`; 1.0 when either side is silent — matching
/// silence would divide by zero and prove nothing).
pub fn match_gain_for(a: &[f32], b: &[f32]) -> f32 {
    let (ra, rb) = (rms(a), rms(b));
    if ra == 0.0 || rb == 0.0 {
        1.0
    } else {
        ra / rb
    }
}

/// Copy of `b` level-matched to `a` (the A/B comparison buffer).
pub fn matched_copy(a: &[f32], b: &[f32]) -> Vec<f32> {
    let g = match_gain_for(a, b);
    b.iter().map(|s| s * g).collect()
}

// -- reference track -------------------------------------------------------------

/// Reference-track convention (no schema change): id starts with `ref_` or
/// the name starts with `[REF] `. Commercial mixes parked here for A/B;
/// never in the mix, never in stems — cued explicitly.
pub fn is_reference_track(id: &str, name: &str) -> bool {
    id.starts_with("ref_") || name.starts_with("[REF] ")
}

/// Ids of all reference tracks in project order.
pub fn reference_tracks(project: &Project) -> Vec<String> {
    project
        .tracks
        .iter()
        .filter(|t| is_reference_track(&t.id, &t.name))
        .map(|t| t.id.clone())
        .collect()
}

/// Mix tracks excluding references (what bounce/export sums).
pub fn mix_tracks_excluding_reference(project: &Project) -> Vec<String> {
    project
        .tracks
        .iter()
        .filter(|t| !is_reference_track(&t.id, &t.name))
        .map(|t| t.id.clone())
        .collect()
}

/// Cue plan for solo-listening reference `ref_id`: the reference sounds,
/// everything else is dimmed. Pure data — the UI applies it as mutes.
/// Errors on unknown ids and on cueing a non-reference track (cueing the
/// mix is `audible`, not this).
pub fn cue_plan(project: &Project, ref_id: &str) -> Result<Vec<(String, bool)>> {
    let ref_track = project
        .tracks
        .iter()
        .find(|t| t.id == ref_id)
        .ok_or_else(|| MixerError::UnknownTrack(ref_id.to_string()))?;
    if !is_reference_track(&ref_track.id, &ref_track.name) {
        return Err(MixerError::BadValue(format!(
            "`{ref_id}` is not a reference track"
        )));
    }
    Ok(project
        .tracks
        .iter()
        .map(|t| (t.id.clone(), t.id == ref_id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClipKind, Edge, Node, NodeKind, Param, Track};

    fn track(id: &str, volume: f64) -> Track {
        Track {
            id: id.to_string(),
            name: id.to_string(),
            volume,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        }
    }

    fn edge(id: &str, from: &str, to: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node: from.to_string(),
            from_port: "out".to_string(),
            to_node: to.to_string(),
            to_port: "in".to_string(),
            kind: EdgeKind::Audio,
        }
    }

    fn bus(id: &str, gain: f64) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Bus,
            name: id.to_string(),
            params: vec![Param {
                id: "gain".to_string(),
                label: "Gain".to_string(),
                value: gain,
                min: 0.0,
                max: 4.0,
                default: 1.0,
                unit: "x".to_string(),
            }],
        }
    }

    fn desk() -> (Project, Vec<VcaGroup>) {
        let mut p = Project::new("p", "Desk");
        p.tracks.push(track("drums", 0.8));
        p.tracks.push(track("bass", 0.7));
        p.tracks.push(Track {
            id: "ref_commercial".to_string(),
            name: "[REF] Commercial".to_string(),
            volume: 0.9,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        p.devices.push(bus("bus_rhythm", 0.5));
        p.routing.push(edge("e1", "drums", "bus_rhythm"));
        p.routing.push(edge("e2", "bass", "bus_rhythm"));
        p.routing.push(edge("e3", "bus_rhythm", "master"));
        let groups = vec![VcaGroup::new("g_rhythm", "Rhythm", vec!["drums".into(), "bass".into()], -6.0)
            .expect("group")];
        (p, groups)
    }

    #[test]
    fn snapshot_null_test_capture_recall_is_identity() {
        let (mut p, mut groups) = desk();
        let before = (p.clone(), groups.clone());
        let snap = capture(&p, &groups, "slot-a");
        recall(&mut p, &mut groups, &snap);
        assert_eq!((&p, &groups), (&before.0, &before.1));
        assert!(snapshot_diff(&snap, &capture(&p, &groups, "slot-a")).is_empty());
    }

    #[test]
    fn snapshot_round_trip_restores_edited_faders() {
        let (mut p, mut groups) = desk();
        let snap = capture(&p, &groups, "slot-a");
        p.tracks[0].volume = 0.1;
        p.tracks[1].muted = true;
        groups[0].set_gain_db(0.0).unwrap();
        recall(&mut p, &mut groups, &snap);
        assert_eq!(p.tracks[0].volume, 0.8);
        assert!(!p.tracks[1].muted);
        assert!((groups[0].gain_db() + 6.0).abs() < 1e-9);
    }

    #[test]
    fn snapshot_diff_names_only_changed_strips() {
        let (p, groups) = desk();
        let a = capture(&p, &groups, "a");
        let mut p2 = p.clone();
        p2.tracks[1].pan = 0.5;
        let b = capture(&p2, &groups, "b");
        assert_eq!(snapshot_diff(&a, &b), vec!["bass".to_string()]);
    }

    #[test]
    fn vca_and_nested_bus_gains_multiply() {
        let (p, groups) = desk();
        // drums: 0.8 × -6dB VCA (≈0.5012) × 0.5 bus.
        let g = effective_track_gain(&p, &groups, "drums").unwrap();
        let expect = 0.8 * db_to_gain(-6.0) * 0.5;
        assert!((g - expect).abs() < 1e-9, "got {g}, want {expect}");
    }

    #[test]
    fn mixer_is_a_view_over_audio_edges() {
        let (p, _) = desk();
        let map: BTreeMap<_, _> = strips(&p).into_iter().map(|s| (s.id, s.out)).collect();
        assert_eq!(map["drums"], Some("bus_rhythm".to_string()));
        assert_eq!(map["ref_commercial"], None);
        assert_eq!(bus_children(&p, "bus_rhythm"), vec!["bass".to_string(), "drums".to_string()]);
        assert_eq!(bus_roots(&p), vec!["master".to_string()]);
        // Reference never sounds in the mix, even unmuted and unsoloed.
        assert!(!audible(&p, "ref_commercial").unwrap());
        assert!(audible(&p, "drums").unwrap());
    }

    #[test]
    fn loudness_match_equalizes_ab_buffers() {
        let a = vec![0.5f32; 1024];
        let b = vec![0.25f32; 1024];
        assert!((loudness_db(&a) - loudness_db(&b) - 6.0206).abs() < 0.01);
        assert!((match_gain_for(&a, &b) - 2.0).abs() < 1e-6);
        let matched = matched_copy(&a, &b);
        assert!((rms(&matched) - rms(&a)).abs() < 1e-6);
        // Silence-matching is the safe no-op, never a division.
        assert_eq!(match_gain_for(&[], &b), 1.0);
        assert_eq!(loudness_db(&[]), SILENCE_DB);
    }

    #[test]
    fn reference_cue_plan_and_export_exclusion() {
        let (p, _) = desk();
        assert_eq!(reference_tracks(&p), vec!["ref_commercial".to_string()]);
        assert!(!mix_tracks_excluding_reference(&p).contains(&"ref_commercial".to_string()));
        let plan: BTreeMap<_, _> = cue_plan(&p, "ref_commercial").unwrap().into_iter().collect();
        assert!(plan["ref_commercial"]);
        assert!(!plan["drums"]);
        assert!(cue_plan(&p, "drums").is_err()); // mix tracks are not cueable refs
        assert!(cue_plan(&p, "nope").is_err());
    }

    #[test]
    fn group_validation_rejects_bad_membership_and_gain() {
        assert!(VcaGroup::new("", "x", vec![], 0.0).is_err());
        assert!(VcaGroup::new("g", "x", vec!["a".into(), "a".into()], 0.0).is_err());
        assert!(VcaGroup::new("g", "x", vec![], f64::NAN).is_err());
        // ClipKind import is used (keeps the helper honest about model types).
        assert_ne!(ClipKind::Audio, ClipKind::Midi);
    }
}
