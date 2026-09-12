//! Failover policy + plan-aware offline bounce.
//!
//! Teaching note: a farm node can vanish mid-session (network cut, box
//! rebooted, heartbeat stale). The policy question is small enough to be a
//! pure function — [`FailoverPolicy::decide`] maps (reachable?, attempts)
//! to one of three outcomes — and the offline answer follows from it:
//! [`bounce_with_failover`] renders every track whose remote nodes are
//! still reachable *as planned* and bounces the orphaned ones locally,
//! reporting exactly which stems fell back. Fail-closed sessions (a live
//! broadcast with no safe local path) get the same report minus the audio:
//! orphaned tracks land in `failed` instead of `stems`.

use std::collections::BTreeSet;

use crate::bounce::{BounceConfig, BounceError, Stem};
use crate::model::Project;

use super::sched::PartitionPlan;

/// What to do when a remote node stops answering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverMode {
    /// Render the orphaned subgraph locally. The mix continues (possibly
    /// with higher CPU); the report names every stem that fell back.
    FailOpenLocal,
    /// Refuse to guess: orphaned tracks are skipped and named in
    /// [`BounceReport::failed`]. The mix stops rather than lies.
    FailClosed,
}

/// Retry budget + mode. Retries happen in the transport (it owns the
/// socket); this policy only decides the outcome *after* the transport
/// reports the node unreachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailoverPolicy {
    pub mode: FailoverMode,
    /// How many failed attempts before this policy even runs. The transport
    /// retries up to this count; the decision below assumes it already did.
    pub max_retries: u32,
}

impl Default for FailoverPolicy {
    fn default() -> Self {
        Self {
            mode: FailoverMode::FailOpenLocal,
            max_retries: 2,
        }
    }
}

/// One node's fate under the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverDecision {
    /// Node answers (or was never remote): carry on.
    UseRemote,
    /// Node is dark: render its subgraph locally (fail-open only).
    BounceLocal,
    /// Node is dark and guessing is forbidden: skip it (fail-closed).
    Abort,
}

impl FailoverPolicy {
    /// Decide one node's fate. `reachable` is the transport's verdict after
    /// its retries; `attempts` is how many times it tried (informational —
    /// the transport enforces `max_retries`, the policy just records it).
    pub fn decide(&self, reachable: bool, _attempts: u32) -> FailoverDecision {
        if reachable {
            return FailoverDecision::UseRemote;
        }
        match self.mode {
            FailoverMode::FailOpenLocal => FailoverDecision::BounceLocal,
            FailoverMode::FailClosed => FailoverDecision::Abort,
        }
    }
}

/// What [`bounce_with_failover`] produced. `stems` and `failed` partition
/// the project's tracks; `fell_back` names the remote nodes that were
/// rendered locally (empty when the farm held).
#[derive(Debug, Clone)]
pub struct BounceReport {
    pub stems: Vec<Stem>,
    pub fell_back: Vec<String>,
    pub failed: Vec<String>,
}

impl BounceReport {
    pub fn ok(stems: Vec<Stem>) -> Self {
        Self {
            stems,
            fell_back: Vec::new(),
            failed: Vec::new(),
        }
    }
}

/// Offline bounce under a partition plan. `reachable` is the set of
/// endpoints that answered the transport's handshake *for this bounce*.
///
/// - Every track whose remote nodes are all reachable bounces normally
///   (local render — the transport swaps in real remote audio behind this
///   same per-track shape when it lands).
/// - A track with an unreachable remote node follows the policy: fail-open
///   bounces it locally and records the orphaned nodes in `fell_back`;
///   fail-closed skips it and records the track in `failed`.
/// - Tracks with no remote nodes always bounce normally.
///
/// Nodes in the plan without an assigned endpoint count as unreachable:
/// planning never strands a node, and neither does the bounce.
pub fn bounce_with_failover(
    project: &Project,
    plan: &PartitionPlan,
    reachable: &BTreeSet<String>,
    policy: &FailoverPolicy,
    config: &BounceConfig,
) -> Result<BounceReport, BounceError> {
    let mut report = BounceReport::ok(Vec::new());
    for track in &project.tracks {
        // Remote nodes this track depends on: its own id plus its devices.
        let dark: Vec<String> = std::iter::once(&track.id)
            .chain(track.device_ids.iter())
            .filter(|id| plan.remote.contains(*id))
            .filter(|id| {
                // No assigned endpoint counts as dark: planning never
                // strands a node, and neither does the bounce.
                plan.endpoint_of(id)
                    .map(|ep| !reachable.contains(ep))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        if dark.is_empty() {
            report.stems.push(crate::bounce::render_track(
                project,
                &track.id,
                config,
            )?);
            continue;
        }
        match policy.decide(false, policy.max_retries) {
            FailoverDecision::UseRemote => unreachable!("dark nodes never decide UseRemote"),
            FailoverDecision::BounceLocal => {
                report.stems.push(crate::bounce::render_track(
                    project,
                    &track.id,
                    config,
                )?);
                for node in dark {
                    if !report.fell_back.contains(&node) {
                        report.fell_back.push(node);
                    }
                }
            }
            FailoverDecision::Abort => report.failed.push(track.id.clone()),
        }
    }
    report.fell_back.sort();
    report.failed.sort();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, ClipKind, Track};
    use crate::remotenodes::sched::{NodeCapability, PartitionPolicy, Registry};

    fn track(id: &str, devices: &[&str]) -> Track {
        Track {
            id: id.to_string(),
            name: id.to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![format!("clip-{id}")],
            device_ids: devices.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn clip(id_track: &str) -> Clip {
        Clip {
            id: format!("clip-{id_track}"),
            track_id: id_track.to_string(),
            name: "Phrase".to_string(),
            start_beats: 0.0,
            length_beats: 4.0,
            kind: ClipKind::Midi,
            source: "take:1".to_string(),
        }
    }

    /// Two sounding tracks sharing one farm: `lead -> heavy -> mix`,
    /// `bass` direct. Only `heavy` can go remote.
    fn project() -> Project {
        use crate::audio::graph::LATENCY_PARAM;
        use crate::model::{Edge, EdgeKind, Node, NodeKind, Param};
        let mut p = Project::new("p", "Failover");
        p.tempo = 120.0;
        p.tracks.push(track("lead", &["heavy"]));
        p.tracks.push(track("bass", &[]));
        p.clips.push(clip("lead"));
        p.clips.push(clip("bass"));
        p.devices.push(Node {
            id: "heavy".to_string(),
            kind: NodeKind::Device,
            name: "heavy".to_string(),
            params: vec![Param {
                id: LATENCY_PARAM.to_string(),
                label: "Latency".to_string(),
                value: 128.0,
                min: 0.0,
                max: 8192.0,
                default: 0.0,
                unit: "samples".to_string(),
            }],
        });
        let mut n = 0;
        for (from, to) in [("lead", "heavy"), ("heavy", "mix"), ("bass", "mix")] {
            n += 1;
            p.routing.push(Edge {
                id: format!("e{n}"),
                from_node: from.to_string(),
                from_port: "out".to_string(),
                to_node: to.to_string(),
                to_port: "in".to_string(),
                kind: EdgeKind::Audio,
            });
        }
        p
    }

    fn farm() -> Registry {
        let mut r = Registry::new();
        r.advertise(NodeCapability {
            endpoint: "tcp://farm:4412".to_string(),
            kinds: vec![crate::model::NodeKind::Device],
            max_nodes: 8,
            load: 0.1,
            online: true,
            ..Default::default()
        });
        r
    }

    fn cfg() -> BounceConfig {
        BounceConfig::new(8000, 0.0, 4.0).expect("config")
    }

    #[test]
    fn decide_routes_on_reachability_and_mode() {
        let open = FailoverPolicy::default();
        assert_eq!(open.decide(true, 0), FailoverDecision::UseRemote);
        assert_eq!(open.decide(false, 2), FailoverDecision::BounceLocal);
        let closed = FailoverPolicy {
            mode: FailoverMode::FailClosed,
            max_retries: 0,
        };
        assert_eq!(closed.decide(true, 0), FailoverDecision::UseRemote);
        assert_eq!(closed.decide(false, 0), FailoverDecision::Abort);
    }

    #[test]
    fn healthy_farm_bounces_every_track_with_no_fallback() {
        let p = project();
        let mut plan = PartitionPolicy::default().partition(&p).expect("partition");
        assert!(plan.remote.contains("heavy"));
        plan.assign_endpoints(&p, &farm());
        let reachable: BTreeSet<String> = ["tcp://farm:4412".to_string()].into();
        let report =
            bounce_with_failover(&p, &plan, &reachable, &FailoverPolicy::default(), &cfg())
                .expect("bounce");
        assert_eq!(report.stems.len(), 2);
        assert!(report.fell_back.is_empty());
        assert!(report.failed.is_empty());
        assert!(report.stems.iter().all(|s| s.peak() > 0.1));
    }

    #[test]
    fn dark_farm_fails_open_to_identical_local_audio() {
        let p = project();
        let mut plan = PartitionPolicy::default().partition(&p).expect("partition");
        plan.assign_endpoints(&p, &farm());
        // Nobody answers: fail-open still delivers every stem, and the lead
        // stem is sample-identical to a pure-local bounce (fallback renders
        // the same deterministic path — it never invents audio).
        let report = bounce_with_failover(
            &p,
            &plan,
            &BTreeSet::new(),
            &FailoverPolicy::default(),
            &cfg(),
        )
        .expect("bounce");
        assert_eq!(report.stems.len(), 2);
        assert_eq!(report.fell_back, vec!["heavy".to_string()]);
        assert!(report.failed.is_empty());
        let local = crate::bounce::render_track(&p, "lead", &cfg()).expect("local");
        let lead = report.stems.iter().find(|s| s.name == "lead").expect("lead stem");
        assert_eq!(lead.samples, local.samples);
    }

    #[test]
    fn dark_farm_fails_closed_by_skipping_orphaned_tracks() {
        let p = project();
        let mut plan = PartitionPolicy::default().partition(&p).expect("partition");
        plan.assign_endpoints(&p, &farm());
        let closed = FailoverPolicy {
            mode: FailoverMode::FailClosed,
            max_retries: 2,
        };
        let report =
            bounce_with_failover(&p, &plan, &BTreeSet::new(), &closed, &cfg()).expect("bounce");
        // lead depended on heavy (dark) → skipped; bass never went remote.
        assert_eq!(report.failed, vec!["lead".to_string()]);
        assert_eq!(report.stems.len(), 1);
        assert!(report.fell_back.is_empty());
    }

    #[test]
    fn unassigned_endpoint_counts_as_unreachable() {
        let p = project();
        // No handshake: heavy is remote in the plan but has no endpoint.
        let plan = PartitionPolicy::default().partition(&p).expect("partition");
        assert!(plan.remote.contains("heavy"));
        assert!(plan.endpoint_of("heavy").is_none());
        let report = bounce_with_failover(
            &p,
            &plan,
            &["tcp://farm:4412".to_string()].into(),
            &FailoverPolicy::default(),
            &cfg(),
        )
        .expect("bounce");
        assert_eq!(report.fell_back, vec!["heavy".to_string()]);
        assert_eq!(report.stems.len(), 2);
    }
}
