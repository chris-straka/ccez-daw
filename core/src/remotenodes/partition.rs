//! Graph partition: cut the node graph into local and remote halves.
//!
//! Teaching note: distributing audio is a *cut* problem, not a placement
//! problem. Pick the nodes that run remotely; every signal edge crossing
//! the cut becomes network I/O — `inputs_to_remote` buffers stream
//! client→server each block, `outputs_from_remote` stream back. The
//! cheapest cut puts the boundary where the fewest edges cross (usually
//! one stem into one farm node). Control-rate edges (`Modulation`,
//! `Sidechain`) never cross: they are previous-block reads, so each side
//! simply uses its own last block — no network, no ordering impact, same
//! as the local scheduler.

use std::collections::{BTreeMap, BTreeSet};

use crate::audio::graph::{AudioGraph, GraphError};

#[cfg(test)]
use crate::audio::graph::LATENCY_PARAM;
#[cfg(test)]
use crate::model::{Edge, EdgeKind, Node, NodeKind, Param, Project, Track};

/// A cut of the graph's node set. Invariants: every node is on exactly
/// one side, the remote side is non-empty, and all ids are known nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub local: BTreeSet<String>,
    pub remote: BTreeSet<String>,
}

impl Partition {
    /// Split `graph` so `remote_ids` render on the DSP node and everything
    /// else stays local. Unknown ids error like the graph itself.
    pub fn split(graph: &AudioGraph, remote_ids: &[&str]) -> Result<Self, GraphError> {
        if remote_ids.is_empty() {
            return Err(GraphError::UnknownNode("<empty remote set>".to_string()));
        }
        let remote: BTreeSet<String> = remote_ids.iter().map(|s| s.to_string()).collect();
        for id in &remote {
            if !graph.nodes.contains_key(id) {
                return Err(GraphError::UnknownNode(id.clone()));
            }
        }
        let local: BTreeSet<String> = graph
            .nodes
            .keys()
            .filter(|id| !remote.contains(*id))
            .cloned()
            .collect();
        Ok(Self { local, remote })
    }

    /// Signal edges crossing the cut, grouped by direction. The client
    /// streams `to_remote` producer buffers each block and the server
    /// returns `from_remote` producer buffers.
    pub fn cut_edges(&self, graph: &AudioGraph) -> CutEdges {
        let mut to_remote = Vec::new();
        let mut from_remote = Vec::new();
        for (from, to) in &graph.signal_edges {
            let f_remote = self.remote.contains(from);
            let t_remote = self.remote.contains(to);
            if !f_remote && t_remote {
                to_remote.push((from.clone(), to.clone()));
            } else if f_remote && !t_remote {
                from_remote.push((from.clone(), to.clone()));
            }
        }
        CutEdges {
            to_remote,
            from_remote,
        }
    }

    /// Remote nodes in signal-flow order (subset of the graph's topo
    /// order). Both the server executor and the local failover path use
    /// this, so degraded output matches live output sample-for-sample.
    pub fn remote_order(&self, graph: &AudioGraph) -> Result<Vec<String>, GraphError> {
        let order = graph.topo_order()?;
        Ok(order
            .into_iter()
            .filter(|id| self.remote.contains(id))
            .collect())
    }

    /// Local nodes in signal-flow order.
    pub fn local_order(&self, graph: &AudioGraph) -> Result<Vec<String>, GraphError> {
        let order = graph.topo_order()?;
        Ok(order
            .into_iter()
            .filter(|id| self.local.contains(id))
            .collect())
    }

    /// Per-block compensation the client must still apply on *local* mix
    /// points fed by remote outputs: the graph's own edge delays,
    /// restricted to remote→local cut edges. (Inside the remote subgraph
    /// the server applies the same map; see [`crate::remotenodes::executor`].)
    pub fn remote_feed_delays(
        &self,
        graph: &AudioGraph,
    ) -> Result<BTreeMap<(String, String), u64>, GraphError> {
        let cut = self.cut_edges(graph);
        let mut delays = BTreeMap::new();
        for (from, to) in &cut.from_remote {
            delays.insert((from.clone(), to.clone()), graph.edge_delay(from, to)?);
        }
        Ok(delays)
    }
}

/// The two directions of one graph cut.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CutEdges {
    /// Local producer → remote consumer. Streamed client→server per block.
    pub to_remote: Vec<(String, String)>,
    /// Remote producer → local consumer. Streamed server→client per block.
    pub from_remote: Vec<(String, String)>,
}

#[cfg(test)]
pub(crate) fn track(id: &str) -> Track {
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

#[cfg(test)]
pub(crate) fn device(id: &str, latency: f64) -> Node {
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

#[cfg(test)]
pub(crate) fn edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge {
            id: id.to_string(),
            from_node: from.to_string(),
            from_port: "out".to_string(),
            to_node: to.to_string(),
            to_port: "in".to_string(),
            kind,
        }
    }

    /// Canonical rig: `a -> dly(64) -> mix`, `b -> mix`.
    #[cfg(test)]
    pub(crate) fn rig_project() -> Project {
        let mut p = Project::new("p", "RemoteRig");
        p.tracks.push(track("a"));
        p.tracks.push(track("b"));
        p.devices.push(device("dly", 64.0));
        p.routing.push(edge("e1", "a", "dly", EdgeKind::Audio));
        p.routing.push(edge("e2", "dly", "mix", EdgeKind::Audio));
        p.routing.push(edge("e3", "b", "mix", EdgeKind::Audio));
        p
    }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_covers_every_node_exactly_once() {
        let g = AudioGraph::from_project(&rig_project());
        let part = Partition::split(&g, &["dly"]).unwrap();
        assert!(part.remote.contains("dly"));
        assert!(part.local.contains("a"));
        assert!(!part.local.contains("dly"));
        assert_eq!(part.local.len() + part.remote.len(), g.nodes.len());
    }

    #[test]
    fn cut_edges_point_both_ways() {
        let g = AudioGraph::from_project(&rig_project());
        let part = Partition::split(&g, &["dly"]).unwrap();
        let cut = part.cut_edges(&g);
        assert_eq!(
            cut.to_remote,
            vec![("a".to_string(), "dly".to_string())]
        );
        assert_eq!(
            cut.from_remote,
            vec![("dly".to_string(), "mix".to_string())]
        );
    }

    #[test]
    fn unknown_and_empty_remote_sets_error() {
        let g = AudioGraph::from_project(&rig_project());
        assert!(matches!(
            Partition::split(&g, &["nope"]),
            Err(GraphError::UnknownNode(_))
        ));
        assert!(Partition::split(&g, &[]).is_err());
    }

    #[test]
    fn remote_feed_delays_come_from_the_graph() {
        // mix's fast input (b) waits 64 for the dly path; the dly->mix
        // feed itself needs no extra delay.
        let g = AudioGraph::from_project(&rig_project());
        let part = Partition::split(&g, &["dly"]).unwrap();
        let delays = part.remote_feed_delays(&g).unwrap();
        assert_eq!(
            delays.get(&("dly".to_string(), "mix".to_string())),
            Some(&0)
        );
    }
}
