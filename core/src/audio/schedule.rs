//! Multicore schedule + the phase-2 remote-node seam.
//!
//! Teaching note: a topo level is a set of nodes whose producers all ran
//! in earlier levels, so every node in one level is independent and may
//! run on its own thread. [`Schedule::levels`] is that levelization; the
//! renderer runs each level with `std::thread::scope`, which is why 1
//! thread and N threads produce bit-identical output.
//!
//! Remote nodes (DSP living in another process / on another machine) are a
//! **phase-2 hook only**: [`Placement::Remote`] marks them,
//! [`Schedule::mark_remote`] assigns them, and any attempt to actually
//! render one fails with [`ScheduleError::RemoteUnimplemented`]. The seam
//! exists so phase 2 adds a transport without re-cutting the scheduler.

use std::collections::BTreeMap;

use super::graph::{AudioGraph, GraphError};

/// Where one graph node executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// This process, on the schedule's thread pool.
    Local,
    /// Somewhere else in phase 2. Carries the address but no transport.
    Remote(RemoteRef),
}

/// Address of not-yet-implemented remote DSP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    /// Opaque endpoint (e.g. `tcp://mix-farm:4412`). Never dialed in v0.
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// Hit the phase-2 seam: rendering a [`Placement::Remote`] node.
    RemoteUnimplemented(String),
    Graph(GraphError),
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemoteUnimplemented(id) => write!(
                f,
                "remote node `{id}` is a phase-2 hook: no transport yet"
            ),
            Self::Graph(e) => write!(f, "schedule graph: {e}"),
        }
    }
}

impl std::error::Error for ScheduleError {}

impl From<GraphError> for ScheduleError {
    fn from(e: GraphError) -> Self {
        Self::Graph(e)
    }
}

/// Levelized execution plan: nodes in one level are mutually independent.
#[derive(Debug, Clone)]
pub struct Schedule {
    pub levels: Vec<Vec<String>>,
    placements: BTreeMap<String, Placement>,
}

impl Schedule {
    /// Levelize the graph. Fails on signal cycles, like the graph itself.
    pub fn build(graph: &AudioGraph) -> Result<Self, GraphError> {
        let order = graph.topo_order()?;
        let mut depth: BTreeMap<&str, usize> = BTreeMap::new();
        for id in &order {
            let d = graph
                .producers_of(id)
                .iter()
                .map(|p| depth.get(p.as_str()).copied().unwrap_or(0) + 1)
                .max()
                .unwrap_or(0);
            depth.insert(id.as_str(), d);
        }
        let max_depth = depth.values().copied().max().unwrap_or(0);
        let mut levels: Vec<Vec<String>> = vec![Vec::new(); max_depth + 1];
        for id in &order {
            let d = depth[id.as_str()];
            levels[d].push(id.clone());
        }
        let placements = graph
            .nodes
            .keys()
            .map(|id| (id.clone(), Placement::Local))
            .collect();
        Ok(Self {
            levels,
            placements,
        })
    }

    /// Mark a node remote (phase-2 hook). Unknown ids error like the graph.
    pub fn mark_remote(&mut self, graph: &AudioGraph, id: &str, endpoint: &str) -> Result<(), GraphError> {
        if !graph.nodes.contains_key(id) {
            return Err(GraphError::UnknownNode(id.to_string()));
        }
        self.placements.insert(
            id.to_string(),
            Placement::Remote(RemoteRef {
                endpoint: endpoint.to_string(),
            }),
        );
        Ok(())
    }

    pub fn placement(&self, id: &str) -> Option<&Placement> {
        self.placements.get(id)
    }

    /// Fail if any node is remote. The renderer calls this first so a
    /// phase-2 address that slipped into a v0 graph errors loudly instead
    /// of rendering silence.
    pub fn require_all_local(&self) -> Result<(), ScheduleError> {
        for (id, placement) in &self.placements {
            if matches!(placement, Placement::Remote(_)) {
                return Err(ScheduleError::RemoteUnimplemented(id.clone()));
            }
        }
        Ok(())
    }

    /// Nodes in execution order (level by level). Remote nodes are included;
    /// pair with [`Schedule::require_all_local`] before rendering.
    pub fn ordered_nodes(&self) -> impl Iterator<Item = &str> {
        self.levels.iter().flat_map(|level| level.iter().map(|s| s.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::graph::LATENCY_PARAM;
    use crate::model::{Edge, EdgeKind, Node, NodeKind, Param, Project, Track};

    fn fixture() -> AudioGraph {
        let mut p = Project::new("p", "Sched");
        for id in ["a", "b"] {
            p.tracks.push(Track {
                id: id.to_string(),
                name: id.to_string(),
                volume: 0.8,
                pan: 0.0,
                muted: false,
                solo: false,
                clip_ids: vec![],
                device_ids: vec![],
            });
        }
        p.devices.push(Node {
            id: "dly".to_string(),
            kind: NodeKind::Device,
            name: "dly".to_string(),
            params: vec![Param {
                id: LATENCY_PARAM.to_string(),
                label: "Latency".to_string(),
                value: 64.0,
                min: 0.0,
                max: 8192.0,
                default: 0.0,
                unit: "samples".to_string(),
            }],
        });
        let mut n = 0;
        for (from, to, kind) in [
            ("a", "dly", EdgeKind::Audio),
            ("dly", "mix", EdgeKind::Audio),
            ("b", "mix", EdgeKind::Audio),
        ] {
            n += 1;
            p.routing.push(Edge {
                id: format!("e{n}"),
                from_node: from.to_string(),
                from_port: "out".to_string(),
                to_node: to.to_string(),
                to_port: "in".to_string(),
                kind,
            });
        }
        AudioGraph::from_project(&p)
    }

    #[test]
    fn levels_keep_producers_before_consumers() {
        let g = fixture();
        let s = Schedule::build(&g).expect("acyclic");
        assert_eq!(s.levels.len(), 3);
        assert!(s.levels[0].contains(&"a".to_string()));
        assert!(s.levels[0].contains(&"b".to_string()));
        assert_eq!(s.levels[1], vec!["dly".to_string()]);
        assert_eq!(s.levels[2], vec!["mix".to_string()]);
    }

    #[test]
    fn remote_seam_marks_but_never_renders() {
        let g = fixture();
        let mut s = Schedule::build(&g).expect("acyclic");
        s.require_all_local().expect("all local by default");
        s.mark_remote(&g, "dly", "tcp://farm:4412").expect("known node");
        assert!(matches!(
            s.placement("dly"),
            Some(Placement::Remote(_))
        ));
        let err = s.require_all_local().expect_err("remote must block render");
        assert_eq!(
            err,
            ScheduleError::RemoteUnimplemented("dly".to_string())
        );
        assert!(s.mark_remote(&g, "ghost", "tcp://x").is_err());
    }
}
