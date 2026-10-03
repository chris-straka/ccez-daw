//! Subset executor: render one side of a partition bit-identically.
//!
//! Teaching note: the offline [`RenderGraph`](crate::audio::render::RenderGraph)
//! renders *every* node in schedule order. A partition needs less: the
//! server renders only its remote nodes (local producers arrive as
//! network buffers), and the client renders only local nodes (remote
//! producers arrive as network buffers — or from the failover path).
//! Both sides run this one function over their own node list, with the
//! same [`Proc`](crate::audio::render::Proc) match arms as the full
//! renderer, so `full render == stitched partition render` exactly. The
//! loopback equivalence test pins that property; if the arms ever drift,
//! that test — not a user's ears — catches it.

use std::collections::BTreeMap;

use crate::audio::render::{Proc, RenderGraph};

/// Render `order` (already in signal-flow order) into `buffers`, seeding
/// it with pre-rendered producer buffers (network inputs or failover
/// outputs). Returns the buffers rendered for `order`. Uses only the
/// public [`RenderGraph`] surface: [`RenderGraph::proc_of`] for the DSP
/// and the graph's producer list for the mix.
pub fn render_subset(
    graph: &RenderGraph,
    order: &[String],
    buffers: &mut BTreeMap<String, Vec<f32>>,
    frames: usize,
    edge_delays: &BTreeMap<(String, String), u64>,
    start_frame: u64,
) -> BTreeMap<String, Vec<f32>> {
    let mut out = BTreeMap::new();
    for id in order {
        let buf = render_one(graph, id, frames, buffers, edge_delays, start_frame);
        buffers.insert(id.clone(), buf.clone());
        out.insert(id.clone(), buf);
    }
    out
}

/// One node's block from its (already rendered, compensated) producers.
/// Same math as the full renderer: sum compensated inputs, apply the proc.
fn render_one(
    graph: &RenderGraph,
    id: &str,
    frames: usize,
    buffers: &BTreeMap<String, Vec<f32>>,
    edge_delays: &BTreeMap<(String, String), u64>,
    start_frame: u64,
) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    for producer in graph.producers_of(id) {
        let delay = edge_delays
            .get(&(producer.clone(), id.to_string()))
            .copied()
            .unwrap_or(0) as usize;
        if let Some(buf) = buffers.get(&producer) {
            for t in 0..frames {
                if t >= delay {
                    input[t] += buf[t - delay];
                }
            }
        }
    }
    match graph.proc_of(id) {
        Proc::Null => vec![0.0; frames],
        Proc::Impulse => {
            let mut o = vec![0.0; frames];
            if frames > 0 {
                o[0] = 1.0;
            }
            o
        }
        Proc::Constant(v) => vec![*v; frames],
        Proc::Delay(latency) => {
            let latency = *latency as usize;
            let mut o = vec![0.0; frames];
            for t in 0..frames {
                if t >= latency {
                    o[t] = input[t - latency];
                }
            }
            o
        }
        Proc::Mix => input,
        Proc::Loop { buf } => {
            if buf.is_empty() {
                return vec![0.0; frames];
            }
            let len = buf.len() as u64;
            (0..frames)
                .map(|t| buf[((start_frame + t as u64) % len) as usize])
                .collect()
        }
    }
}

/// The server side of one partition: owns the full graph (same procs as
/// the client built from the same project) and renders only the remote
/// nodes per block.
#[derive(Debug, Clone)]
pub struct RemoteExecutor {
    graph: RenderGraph,
    remote_order: Vec<String>,
    edge_delays: BTreeMap<(String, String), u64>,
}

impl RemoteExecutor {
    pub fn new(
        graph: RenderGraph,
        remote_order: Vec<String>,
        edge_delays: BTreeMap<(String, String), u64>,
    ) -> Self {
        Self {
            graph,
            remote_order,
            edge_delays,
        }
    }

    pub fn remote_order(&self) -> &[String] {
        &self.remote_order
    }

    /// Render one block. `boundary` carries the client's local-producer
    /// buffers for every local→remote cut edge producer. Returns one
    /// buffer per remote node. `start_frame` positions [`Proc::Loop`]
    /// reads (the caller's running frame count).
    pub fn execute(
        &self,
        boundary: &BTreeMap<String, Vec<f32>>,
        frames: usize,
        start_frame: u64,
    ) -> BTreeMap<String, Vec<f32>> {
        let mut buffers = boundary.clone();
        render_subset(
            &self.graph,
            &self.remote_order,
            &mut buffers,
            frames,
            &self.edge_delays,
            start_frame,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::graph::AudioGraph;
    use crate::audio::render::RenderGraph;
    use crate::model::{EdgeKind, Project};

    use super::super::partition::Partition;
    use crate::remotenodes::partition::{device, edge, track};

    fn rig() -> (RenderGraph, BTreeMap<(String, String), u64>) {
        let mut p = Project::new("p", "Exec");
        p.tracks.push(track("a"));
        p.tracks.push(track("b"));
        p.devices.push(device("dly", 64.0));
        p.routing.push(edge("e1", "a", "dly", EdgeKind::Audio));
        p.routing.push(edge("e2", "dly", "mix", EdgeKind::Audio));
        p.routing.push(edge("e3", "b", "mix", EdgeKind::Audio));
        let g = AudioGraph::from_project(&p);
        let delays = g.all_edge_delays().unwrap();
        let mut rg = RenderGraph::from_audio_graph(g);
        rg.set_proc("a", Proc::Impulse);
        rg.set_proc("b", Proc::Constant(0.5));
        rg.set_proc("dly", Proc::Delay(64));
        rg.set_proc("mix", Proc::Mix);
        (rg, delays)
    }

    #[test]
    fn subset_matches_full_render_exactly() {
        // The pin that matters: stitching server + client subsets must
        // equal one local render, bit for bit.
        let (rg, delays) = rig();
        let frames = 128;
        let full = rg.render(frames, 1, &delays, 0).unwrap();

        let part = Partition::split(&rg.topo, &["dly"]).unwrap();
        let remote_order = part.remote_order(&rg.topo).unwrap();
        let local_order = part.local_order(&rg.topo).unwrap();

        // Client upstream pass: local nodes whose producers are all local
        // (here: a, b). dly's producer a is ready, so its buffer ships.
        let mut buffers: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        let upstream: Vec<String> = local_order
            .iter()
            .filter(|id| {
                rg.topo
                    .producers_of(id)
                    .iter()
                    .all(|p| part.local.contains(p))
            })
            .cloned()
            .collect();
        render_subset(&rg, &upstream, &mut buffers, frames, &delays, 0);

        // Server pass over the remote subset from the shipped boundary.
        let exec = RemoteExecutor::new(rg.clone(), remote_order, delays.clone());
        let remote_out = exec.execute(&buffers, frames, 0);
        for (id, buf) in &remote_out {
            buffers.insert(id.clone(), buf.clone());
        }

        // Client downstream pass: remaining local nodes (mix).
        let rest: Vec<String> = local_order
            .into_iter()
            .filter(|id| !buffers.contains_key(id))
            .collect();
        render_subset(&rg, &rest, &mut buffers, frames, &delays, 0);

        for (id, buf) in &full {
            assert_eq!(
                buffers.get(id),
                Some(buf),
                "stitched buffer differs for `{id}`"
            );
        }
    }
}
