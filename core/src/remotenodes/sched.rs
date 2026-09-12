//! Partition policy + load/capability handshake + transport message types.
//!
//! Teaching note: partitioning is a placement question, not a networking
//! one. Given a project, [`PartitionPolicy::partition`] picks the remote
//! candidate set deterministically; given worker advertisements,
//! [`Registry::assign`] picks *which* endpoint takes each candidate by
//! least load. The actual audio never flows here — when the sibling
//! transport module lands, it moves [`RemoteJob`]s one way and
//! [`RemoteResult`]s back; until then the plan applies through the
//! existing `Schedule::mark_remote` seam.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::audio::graph::{AudioGraph, GraphError};
use crate::audio::schedule::Schedule;
use crate::model::{NodeKind, Project};

/// Default ceiling on per-node lookahead for remote candidates, in samples.
/// A node that needs more future audio than this stays local: every network
/// hop already adds latency, and stacking lookahead on top of it pushes mix
/// alignment past what the delay lines can hide.
pub const DEFAULT_MAX_REMOTE_LATENCY: u64 = 512;
/// Default ceiling on the remote fraction of the graph. Keeps at least half
/// the nodes local so one farm outage can never take the whole mix with it.
pub const DEFAULT_MAX_REMOTE_SHARE: f32 = 0.5;

/// What one remote worker can do. Workers advertise these (out of band —
/// UDP beacon, config file, whatever the transport owns); the scheduler
/// only ever *reads* them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCapability {
    /// Opaque endpoint, same shape as `RemoteRef::endpoint` (`tcp://…`).
    /// Never dialed here.
    pub endpoint: String,
    /// Node kinds this worker accepts (usually `[Device]`).
    pub kinds: Vec<NodeKind>,
    /// Max nodes this worker takes before it counts as full.
    pub max_nodes: usize,
    /// Current load, `0.0` (idle) to `1.0` (saturated). Best-effort hint.
    pub load: f32,
    /// `false` removes the worker from selection without forgetting it.
    pub online: bool,
}

impl NodeCapability {
    pub fn accepts(&self, kind: &NodeKind) -> bool {
        self.online && self.kinds.contains(kind)
    }
}

// Sane test/fallback fill: accepts devices, room for 4 nodes, idle.
// Production code always sets every field explicitly.
impl Default for NodeCapability {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            kinds: vec![NodeKind::Device],
            max_nodes: 4,
            load: 0.0,
            online: true,
        }
    }
}

/// Live view of advertised workers. Owns the handshake: advertise →
/// heartbeat → assign. All deterministic, so tests and replays agree.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    nodes: BTreeMap<String, NodeCapability>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advertise (or re-advertise) a worker. Same endpoint twice replaces.
    pub fn advertise(&mut self, cap: NodeCapability) {
        self.nodes.insert(cap.endpoint.clone(), cap);
    }

    /// Forget a worker entirely.
    pub fn withdraw(&mut self, endpoint: &str) -> bool {
        self.nodes.remove(endpoint).is_some()
    }

    /// Best-effort load update. Unknown endpoints are ignored (a stale
    /// heartbeat must not conjure a worker).
    pub fn heartbeat(&mut self, endpoint: &str, load: f32) {
        if let Some(cap) = self.nodes.get_mut(endpoint) {
            cap.load = load.clamp(0.0, 1.0);
        }
    }

    pub fn get(&self, endpoint: &str) -> Option<&NodeCapability> {
        self.nodes.get(endpoint)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Assign each requested node to the least-loaded online worker that
    /// accepts its kind and still has capacity. Requests are
    /// `(node_id, kind)`; the returned map covers only assignable nodes —
    /// the caller keeps the rest local (fail-open at plan time). Ties break
    /// by endpoint, so the same registry + requests always agree.
    pub fn assign(&self, requests: &[(&str, NodeKind)]) -> BTreeMap<String, String> {
        let mut used: BTreeMap<&str, usize> = BTreeMap::new();
        let mut out = BTreeMap::new();
        // Deterministic request order regardless of caller order.
        let mut sorted: Vec<(&str, &NodeKind)> =
            requests.iter().map(|(id, k)| (*id, k)).collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (id, kind) in sorted {
            let mut best: Option<&NodeCapability> = None;
            for cap in self.nodes.values() {
                if !cap.accepts(kind) {
                    continue;
                }
                if used.get(cap.endpoint.as_str()).copied().unwrap_or(0) >= cap.max_nodes {
                    continue;
                }
                best = Some(match best {
                    None => cap,
                    Some(b)
                        if (cap.load, &cap.endpoint) < (b.load, &b.endpoint) =>
                    {
                        cap
                    }
                    Some(b) => b,
                });
            }
            if let Some(cap) = best {
                *used.entry(cap.endpoint.as_str()).or_insert(0) += 1;
                out.insert(id.to_string(), cap.endpoint.clone());
            }
        }
        out
    }
}

/// Which subgraphs go remote. Field order is the rule order; every rule is
/// deliberately conservative (a node stays local unless it clearly wins).
#[derive(Debug, Clone)]
pub struct PartitionPolicy {
    /// Nodes with intrinsic latency above this stay local.
    pub max_remote_latency: u64,
    /// Max fraction of all graph nodes that may go remote (`0.0`–`1.0`).
    pub max_remote_share: f32,
    /// Only these kinds are ever candidates (default: just devices — tracks
    /// and mix buses stay where the delay lines are).
    pub remote_kinds: Vec<NodeKind>,
}

impl Default for PartitionPolicy {
    fn default() -> Self {
        Self {
            max_remote_latency: DEFAULT_MAX_REMOTE_LATENCY,
            max_remote_share: DEFAULT_MAX_REMOTE_SHARE,
            remote_kinds: vec![NodeKind::Device],
        }
    }
}

impl PartitionPolicy {
    /// Split the project's graph into a remote candidate set. Rules:
    ///
    /// 1. Signal cycles fail (`GraphError::Cycle`) — an unrenderable graph
    ///    is also an unpartitionable one.
    /// 2. Sinks (no signal consumers — mix buses) stay local: mix-point
    ///    alignment happens where the delay lines live.
    /// 3. Only `remote_kinds` are candidates; anything else stays local.
    /// 4. Nodes above `max_remote_latency` stay local (lookahead + network
    ///    hop is worse than local DSP).
    /// 5. At most `floor(max_remote_share * total_nodes)` go remote,
    ///    heaviest latency first, ties by node id — so big lookahead DSP
    ///    (the expensive stuff worth offloading) wins the slots.
    pub fn partition(&self, project: &Project) -> Result<PartitionPlan, GraphError> {
        let graph = AudioGraph::from_project(project);
        graph.topo_order()?; // rule 1: refuse cycles up front
        let consumers = signal_consumers(&graph);
        let mut candidates: Vec<(&String, u64)> = graph
            .nodes
            .iter()
            .filter(|(id, _)| {
                // Rule 2: sinks (mix buses) stay local — they have no
                // consumers, so keep only nodes that feed something.
                !consumers.get(id.as_str()).map(|c| c.is_empty()).unwrap_or(true)
            })
            .filter(|(id, node)| {
                node_kind_of(project, id) // rule 3
                    .map(|k| self.remote_kinds.contains(k))
                    .unwrap_or(false)
                    && node.latency <= self.max_remote_latency // rule 4
            })
            .map(|(id, node)| (id, node.latency))
            .collect();
        candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0))); // rule 5 order
        let budget = (self.max_remote_share.max(0.0).min(1.0) * graph.nodes.len() as f32).floor()
            as usize;
        let remote: BTreeSet<String> = candidates
            .into_iter()
            .take(budget)
            .map(|(id, _)| id.clone())
            .collect();
        let local = graph
            .nodes
            .keys()
            .filter(|id| !remote.contains(*id))
            .cloned()
            .collect();
        Ok(PartitionPlan {
            remote,
            local,
            endpoints: BTreeMap::new(),
        })
    }
}

/// The policy's answer: which nodes want to go remote, and (after
/// [`PartitionPlan::assign_endpoints`]) which endpoint takes each one.
#[derive(Debug, Clone, Default)]
pub struct PartitionPlan {
    /// Candidate node ids (no endpoints yet until assigned).
    pub remote: BTreeSet<String>,
    /// Everything else.
    pub local: Vec<String>,
    /// Assigned endpoint per remote node. Empty until `assign_endpoints`.
    pub endpoints: BTreeMap<String, String>,
}

impl PartitionPlan {
    /// Run the load/capability handshake: match remote candidates against
    /// the registry. Unassignable nodes fall back to local (removed from
    /// `remote`, appended to `local`) — planning never strands a node.
    pub fn assign_endpoints(&mut self, project: &Project, registry: &Registry) {
        let requests: Vec<(&str, NodeKind)> = self
            .remote
            .iter()
            .map(|id| (id.as_str(), node_kind_of(project, id).cloned().unwrap_or(NodeKind::Bus)))
            .collect();
        self.endpoints = registry.assign(&requests);
        let stranded: Vec<String> = self
            .remote
            .iter()
            .filter(|id| !self.endpoints.contains_key(*id))
            .cloned()
            .collect();
        for id in stranded {
            self.remote.remove(&id);
            self.local.push(id);
        }
        self.local.sort();
    }

    /// Endpoint for a remote node, if assigned.
    pub fn endpoint_of(&self, node: &str) -> Option<&str> {
        self.endpoints.get(node).map(String::as_str)
    }

    /// Mark every assigned-remote node on a schedule through the existing
    /// seam. Nodes without an endpoint stay local (same fail-open rule as
    /// `assign_endpoints`: planning never strands a node).
    pub fn apply_to(
        &self,
        schedule: &mut Schedule,
        graph: &AudioGraph,
        default_endpoint: &str,
    ) -> Result<(), GraphError> {
        for id in &self.remote {
            let ep = self.endpoint_of(id).unwrap_or(default_endpoint);
            schedule.mark_remote(graph, id, ep)?;
        }
        Ok(())
    }
}

/// One block of remote work, scheduler → transport. Plain data: the
/// transport owns framing, retries, and the socket; both sides share this
/// shape so neither has to read the other's code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteJob {
    pub job_id: String,
    pub node_id: String,
    pub endpoint: String,
    pub frames: usize,
}

/// Remote work answered, transport → scheduler. Samples are mono f32, the
/// same shape the local renderer produces per node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteResult {
    pub job_id: String,
    pub node_id: String,
    pub samples: Vec<f32>,
}

/// Remote work failed, transport → scheduler. `reason` is human-readable;
/// the failover policy (not the transport) decides what happens next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteError {
    pub job_id: String,
    pub node_id: String,
    pub reason: String,
}

// -- internals ---------------------------------------------------------------

fn signal_consumers(graph: &AudioGraph) -> BTreeMap<&str, Vec<&str>> {
    let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in graph.nodes.keys() {
        map.insert(id.as_str(), Vec::new());
    }
    for (from, to) in &graph.signal_edges {
        if let Some(v) = map.get_mut(from.as_str()) {
            v.push(to.as_str());
        }
    }
    map
}

fn node_kind_of<'a>(project: &'a Project, id: &str) -> Option<&'a NodeKind> {
    if project.tracks.iter().any(|t| t.id == id) {
        return Some(&TRACK_KIND);
    }
    if let Some(dev) = project.devices.iter().find(|d| d.id == id) {
        return Some(&dev.kind);
    }
    if project.clips.iter().any(|c| c.id == id) {
        return Some(&CLIP_KIND);
    }
    None
}

const TRACK_KIND: NodeKind = NodeKind::Track;
const CLIP_KIND: NodeKind = NodeKind::Clip;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::graph::LATENCY_PARAM;
    use crate::model::{Edge, EdgeKind, Node, Param, Track};

    fn track(id: &str) -> Track {
        Track {
            id: id.to_string(),
            name: id.to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        }
    }

    fn device(id: &str, latency: f64) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Device,
            name: id.to_string(),
            params: vec![Param {
                id: LATENCY_PARAM.to_string(),
                label: "Latency".to_string(),
                value: latency,
                min: 0.0,
                max: 8192.0,
                default: 0.0,
                unit: "samples".to_string(),
            }],
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

    /// Two heavy devices feeding one mix bus: `a -> heavy(256) -> mix`,
    /// `b -> light(16) -> mix`. Sinks (`mix`) must stay local.
    fn farm_project() -> Project {
        let mut p = Project::new("p", "Farm");
        p.tracks.push(track("a"));
        p.tracks.push(track("b"));
        p.devices.push(device("heavy", 256.0));
        p.devices.push(device("light", 16.0));
        let mut n = 0;
        for (from, to) in [("a", "heavy"), ("heavy", "mix"), ("b", "light"), ("light", "mix")] {
            n += 1;
            p.routing.push(edge(&format!("e{n}"), from, to));
        }
        p
    }

    fn registry_two_workers() -> Registry {
        let mut r = Registry::new();
        r.advertise(NodeCapability {
            endpoint: "tcp://farm-a:4412".to_string(),
            kinds: vec![NodeKind::Device],
            max_nodes: 4,
            load: 0.8,
            online: false, // flaky box, out of rotation
            ..Default::default()
        });
        r.advertise(NodeCapability {
            endpoint: "tcp://farm-b:4412".to_string(),
            kinds: vec![NodeKind::Device],
            max_nodes: 4,
            load: 0.2,
            online: true,
            ..Default::default()
        });
        r
    }

    #[test]
    fn partition_sends_heavy_devices_remote_and_keeps_mix_local() {
        let p = farm_project();
        let plan = PartitionPolicy::default().partition(&p).expect("partition");
        // 6 nodes (a, b, heavy, light, mix + …): budget floor(0.5*6)=3, two
        // device candidates, heavy first. Mix (sink) is never remote.
        assert!(plan.remote.contains("heavy"));
        assert!(!plan.remote.contains("mix"));
        assert!(!plan.local.contains(&"heavy".to_string()));
    }

    #[test]
    fn partition_respects_latency_ceiling_and_share_budget() {
        let p = farm_project();
        // Latency ceiling below heavy's 256: heavy stays local too.
        let strict = PartitionPolicy {
            max_remote_latency: 64,
            ..PartitionPolicy::default()
        };
        let plan = strict.partition(&p).expect("partition");
        assert!(!plan.remote.contains("heavy"));
        assert!(plan.remote.contains("light"));
        // Zero share: everything stays local.
        let none = PartitionPolicy {
            max_remote_share: 0.0,
            ..PartitionPolicy::default()
        };
        let plan = none.partition(&p).expect("partition");
        assert!(plan.remote.is_empty());
    }

    #[test]
    fn partition_refuses_signal_cycles() {
        let mut p = farm_project();
        p.routing.push(edge("loop", "mix", "a"));
        let err = PartitionPolicy::default()
            .partition(&p)
            .expect_err("cycle must fail");
        assert!(matches!(err, GraphError::Cycle(_)));
    }

    #[test]
    fn handshake_skips_offline_and_full_workers() {
        let mut r = registry_two_workers();
        // Only farm-b is online: both devices land there.
        let got = r.assign(&[("heavy", NodeKind::Device), ("light", NodeKind::Device)]);
        assert_eq!(got.get("heavy").map(String::as_str), Some("tcp://farm-b:4412"));
        // Unknown heartbeat must not conjure a worker.
        r.heartbeat("tcp://ghost:4412", 0.0);
        assert!(r.get("tcp://ghost:4412").is_none());
        // Full worker (max_nodes hit) spills nothing — extra node unassigned.
        r.advertise(NodeCapability {
            endpoint: "tcp://tiny:4412".to_string(),
            kinds: vec![NodeKind::Device],
            max_nodes: 0,
            load: 0.0,
            online: true,
            ..Default::default()
        });
        let got = r.assign(&[("heavy", NodeKind::Device)]);
        assert_eq!(got.get("heavy").map(String::as_str), Some("tcp://farm-b:4412"));
    }

    #[test]
    fn assign_endpoints_falls_back_to_local_when_no_worker_fits() {
        let p = farm_project();
        let mut plan = PartitionPolicy::default().partition(&p).expect("partition");
        assert!(!plan.remote.is_empty());
        // Empty registry: every candidate falls back to local, none stranded.
        plan.assign_endpoints(&p, &Registry::new());
        assert!(plan.remote.is_empty());
        assert!(plan.endpoints.is_empty());
        // Two-worker registry: candidates get the online worker.
        let mut plan = PartitionPolicy::default().partition(&p).expect("partition");
        plan.assign_endpoints(&p, &registry_two_workers());
        for id in &plan.remote {
            assert_eq!(plan.endpoint_of(id), Some("tcp://farm-b:4412"));
        }
    }

    #[test]
    fn plan_applies_through_the_mark_remote_seam() {
        let p = farm_project();
        let graph = AudioGraph::from_project(&p);
        let mut plan = PartitionPolicy::default().partition(&p).expect("partition");
        plan.assign_endpoints(&p, &registry_two_workers());
        let mut schedule = Schedule::build(&graph).expect("schedule");
        plan.apply_to(&mut schedule, &graph, "tcp://default:4412")
            .expect("apply");
        for id in &plan.remote {
            assert!(matches!(
                schedule.placement(id),
                Some(crate::audio::schedule::Placement::Remote(_))
            ));
        }
        // Unknown node in the plan errors like the graph, not silently.
        let mut bad = plan.clone();
        bad.remote.insert("ghost".to_string());
        assert!(bad.apply_to(&mut schedule, &graph, "tcp://x").is_err());
    }

    #[test]
    fn remote_messages_round_trip_as_plain_data() {
        let job = RemoteJob {
            job_id: "j1".to_string(),
            node_id: "heavy".to_string(),
            endpoint: "tcp://farm-b:4412".to_string(),
            frames: 512,
        };
        let json = serde_json::to_string(&job).expect("encode");
        assert_eq!(serde_json::from_str::<RemoteJob>(&json).expect("decode"), job);
        let res = RemoteResult {
            job_id: "j1".to_string(),
            node_id: "heavy".to_string(),
            samples: vec![0.5; 512],
        };
        let json = serde_json::to_string(&res).expect("encode");
        assert_eq!(serde_json::from_str::<RemoteResult>(&json).expect("decode"), res);
    }
}
