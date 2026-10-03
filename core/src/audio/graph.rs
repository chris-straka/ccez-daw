//! Universal node graph: audio, MIDI, modulation, and sidechains in one
//! routing model.
//!
//! Teaching note: most DAWs keep four separate routers (one for audio, one
//! for MIDI, one for modulation, one for sidechains) and they drift apart.
//! Here there is exactly one edge list — the frozen `Edge` table from
//! `contracts/project-schema.md` — and the four kinds are *views* over it:
//!
//! - `Audio` + `Midi` edges carry signal and define **processing order**
//!   (a node runs after the nodes feeding it).
//! - `Modulation` + `Sidechain` edges are **control-rate**: the consumer
//!   reads the producer's *previous-block* value, so they never impose
//!   order and never create audio cycles. A "feedback" LFO is therefore
//!   legal; a feedback audio cable is a [`GraphError::Cycle`].
//!
//! Latency compensation is per-edge input alignment: at every mix point the
//! faster inputs are delayed to match the slowest one (see
//! [`AudioGraph::edge_delay`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::model::{EdgeKind, Project};

/// Device-node param that declares intrinsic processing latency in samples.
/// Any device `Node` may carry it (e.g. a lookahead limiter reports 128);
/// missing means zero. Lives inside the frozen `Param` shape, so no schema
/// change was needed.
pub const LATENCY_PARAM: &str = "latency_samples";

/// One node in signal-flow order. `latency` is the node's intrinsic delay
/// in samples (its output for input block `t` emerges at `t + latency`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub latency: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    Cycle(Vec<String>),
    UnknownNode(String),
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cycle(nodes) => write!(f, "audio cycle through: {}", nodes.join(", ")),
            Self::UnknownNode(id) => write!(f, "unknown graph node `{id}`"),
        }
    }
}

impl std::error::Error for GraphError {}

/// Signal-flow view of the project routing: every `Node`, track, and edge
/// endpoint, with control-rate edges kept aside so they can't false-cycle.
#[derive(Debug, Clone, Default)]
pub struct AudioGraph {
    /// All addressable nodes (tracks, devices, buses, implicit endpoints).
    pub nodes: BTreeMap<String, GraphNode>,
    /// Signal edges (`Audio` + `Midi`): `(producer, consumer)`.
    pub signal_edges: Vec<(String, String)>,
    /// Control edges (`Modulation` + `Sidechain`): `(producer, consumer)`.
    /// Previous-block reads only; excluded from ordering and compensation.
    pub control_edges: Vec<(String, String)>,
}

impl AudioGraph {
    /// Build the graph from a project. Node ids come from tracks and
    /// devices; edge endpoints not present in either become implicit
    /// zero-latency nodes (mix buses / external ports) instead of errors,
    /// so partially-specified routing still renders.
    pub fn from_project(project: &Project) -> Self {
        let mut nodes: BTreeMap<String, GraphNode> = BTreeMap::new();
        for track in &project.tracks {
            nodes.insert(
                track.id.clone(),
                GraphNode {
                    id: track.id.clone(),
                    latency: 0,
                },
            );
        }
        for device in &project.devices {
            let latency = device
                .params
                .iter()
                .find(|p| p.id == LATENCY_PARAM)
                .map(|p| p.value.max(0.0) as u64)
                .unwrap_or(0);
            nodes.insert(
                device.id.clone(),
                GraphNode {
                    id: device.id.clone(),
                    latency,
                },
            );
        }
        let mut graph = Self {
            nodes,
            signal_edges: Vec::new(),
            control_edges: Vec::new(),
        };
        for edge in &project.routing {
            graph.ensure_node(&edge.from_node);
            graph.ensure_node(&edge.to_node);
            let pair = (edge.from_node.clone(), edge.to_node.clone());
            match edge.kind {
                EdgeKind::Audio | EdgeKind::Midi => graph.signal_edges.push(pair),
                EdgeKind::Modulation | EdgeKind::Sidechain => graph.control_edges.push(pair),
            }
        }
        graph
    }

    /// Insert a bare node when absent (the live loop's `mix` sink is not
    /// necessarily a project node). Additive; topology inference unchanged.
    pub fn ensure_node(&mut self, id: &str) {
        if !self.nodes.contains_key(id) {
            self.nodes.insert(
                id.to_string(),
                GraphNode {
                    id: id.to_string(),
                    latency: 0,
                },
            );
        }
    }

    fn successors(&self) -> BTreeMap<&str, Vec<&str>> {
        let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for id in self.nodes.keys() {
            map.insert(id.as_str(), Vec::new());
        }
        for (from, to) in &self.signal_edges {
            if let Some(v) = map.get_mut(from.as_str()) {
                v.push(to.as_str());
            }
        }
        map
    }

    /// Signal-flow order: every producer before its consumers (Kahn's
    /// algorithm). Control-rate edges are ignored — a modulation-only loop
    /// is legal and orders arbitrarily. A signal loop is unrenderable and
    /// returns [`GraphError::Cycle`].
    pub fn topo_order(&self) -> Result<Vec<String>, GraphError> {
        let mut indegree: BTreeMap<&str, usize> = BTreeMap::new();
        for id in self.nodes.keys() {
            indegree.insert(id.as_str(), 0);
        }
        let succ = self.successors();
        for (_, tos) in &succ {
            for to in tos {
                *indegree.get_mut(*to).unwrap_or(&mut 0) += 1;
            }
        }
        let mut ready: VecDeque<&str> = indegree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&id, _)| id)
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(id) = ready.pop_front() {
            order.push(id.to_string());
            for next in &succ[id] {
                let d = indegree.get_mut(*next).expect("successor is a node");
                *d -= 1;
                if *d == 0 {
                    ready.push_back(next);
                }
            }
        }
        if order.len() != self.nodes.len() {
            let mut stuck: Vec<String> = indegree
                .iter()
                .filter(|(_, &d)| d > 0)
                .map(|(&id, _)| id.to_string())
                .collect();
            stuck.sort();
            return Err(GraphError::Cycle(stuck));
        }
        Ok(order)
    }

    /// Output-emergence time of each node: `out[node] = arrival[node] +
    /// latency[node]`, where `arrival[node]` is the slowest producer output
    /// feeding it (0 for sources). Follows signal edges only.
    pub fn output_times(&self) -> Result<BTreeMap<String, u64>, GraphError> {
        let order = self.topo_order()?;
        let mut preds: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for id in self.nodes.keys() {
            preds.insert(id.as_str(), Vec::new());
        }
        for (from, to) in &self.signal_edges {
            if let Some(v) = preds.get_mut(to.as_str()) {
                v.push(from.as_str());
            }
        }
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for id in order {
            let arrival = preds[id.as_str()]
                .iter()
                .map(|p| out.get(*p).copied().unwrap_or(0))
                .max()
                .unwrap_or(0);
            let latency = self.nodes[&id].latency;
            out.insert(id.clone(), arrival + latency);
        }
        Ok(out)
    }

    /// Latency compensation as **per-edge input alignment**: samples of
    /// delay to insert on the `from -> to` signal input so every input to
    /// `to` arrives together, i.e. `arrival[to] - out[from]` (>= 0 by
    /// construction). The renderer realizes these as delay lines; the slow
    /// path itself needs no delay. Unknown edges error.
    pub fn edge_delay(&self, from: &str, to: &str) -> Result<u64, GraphError> {
        if !self
            .signal_edges
            .iter()
            .any(|(f, t)| f == from && t == to)
        {
            if !self.nodes.contains_key(from) {
                return Err(GraphError::UnknownNode(from.to_string()));
            }
            if !self.nodes.contains_key(to) {
                return Err(GraphError::UnknownNode(to.to_string()));
            }
            return Ok(0);
        }
        let out = self.output_times()?;
        let arrival_to = self
            .signal_edges
            .iter()
            .filter(|(_, t)| t == to)
            .map(|(f, _)| out.get(f).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);
        Ok(arrival_to - out.get(from).copied().unwrap_or(0))
    }

    /// Every signal edge's compensation delay. Control edges are absent by
    /// design (previous-block reads need no alignment).
    pub fn all_edge_delays(&self) -> Result<BTreeMap<(String, String), u64>, GraphError> {
        let mut delays = BTreeMap::new();
        let edges = self.signal_edges.clone();
        for (from, to) in edges {
            let d = self.edge_delay(&from, &to)?;
            delays.insert((from, to), d);
        }
        Ok(delays)
    }

    /// Direct signal producers of `node` (empty for sources).
    pub fn producers_of(&self, node: &str) -> Vec<String> {
        let mut set = BTreeSet::new();
        for (from, to) in &self.signal_edges {
            if to == node {
                set.insert(from.clone());
            }
        }
        set.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, Node, NodeKind, Project, Track};

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
            params: vec![crate::model::Param {
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

    fn edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge {
            id: id.to_string(),
            from_node: from.to_string(),
            from_port: "out".to_string(),
            to_node: to.to_string(),
            to_port: "in".to_string(),
            kind,
        }
    }

    fn latency_project() -> Project {
        let mut p = Project::new("p", "Latency");
        p.tracks.push(track("src_a"));
        p.tracks.push(track("src_b"));
        p.devices.push(device("dly", 64.0));
        p.routing.push(edge("e1", "src_a", "dly", EdgeKind::Audio));
        p.routing.push(edge("e2", "dly", "mix", EdgeKind::Audio));
        p.routing.push(edge("e3", "src_b", "mix", EdgeKind::Audio));
        p
    }

    #[test]
    fn one_graph_carries_all_four_kinds() {
        let mut p = latency_project();
        p.routing.push(edge("m1", "lfo", "dly", EdgeKind::Modulation));
        p.routing.push(edge("s1", "src_a", "mix", EdgeKind::Sidechain));
        p.routing.push(edge("n1", "clip", "src_a", EdgeKind::Midi));
        let g = AudioGraph::from_project(&p);
        assert_eq!(g.signal_edges.len(), 4); // 3 audio + 1 midi
        assert_eq!(g.control_edges.len(), 2); // modulation + sidechain
        assert!(g.nodes.contains_key("lfo")); // implicit endpoint, zero latency
        assert_eq!(g.nodes["lfo"].latency, 0);
        assert_eq!(g.nodes["dly"].latency, 64);
    }

    #[test]
    fn topo_orders_producers_before_consumers() {
        let g = AudioGraph::from_project(&latency_project());
        let order = g.topo_order().expect("acyclic");
        let pos = |id: &str| order.iter().position(|n| n == id).unwrap();
        assert!(pos("src_a") < pos("dly"));
        assert!(pos("dly") < pos("mix"));
        assert!(pos("src_b") < pos("mix"));
    }

    #[test]
    fn modulation_only_loop_is_not_a_cycle() {
        let mut p = latency_project();
        p.routing.push(edge("m1", "dly", "src_a", EdgeKind::Modulation));
        p.routing.push(edge("m2", "src_a", "dly", EdgeKind::Modulation));
        let g = AudioGraph::from_project(&p);
        g.topo_order().expect("control loops are legal");
    }

    #[test]
    fn audio_loop_is_a_cycle() {
        let mut p = latency_project();
        p.routing.push(edge("loop", "mix", "src_a", EdgeKind::Audio));
        let g = AudioGraph::from_project(&p);
        let err = g.topo_order().expect_err("signal loop must fail");
        assert!(matches!(err, GraphError::Cycle(_)));
    }

    #[test]
    fn compensation_delays_fast_path_to_slowest() {
        let g = AudioGraph::from_project(&latency_project());
        // src_b -> mix must wait 64 samples for the src_a -> dly path.
        assert_eq!(g.edge_delay("src_b", "mix").unwrap(), 64);
        // The slow path itself needs no extra delay.
        assert_eq!(g.edge_delay("dly", "mix").unwrap(), 0);
        assert_eq!(g.edge_delay("src_a", "dly").unwrap(), 0);
    }
}
