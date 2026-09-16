//! Deterministic offline renderer: the DSP the cpal callback runs.
//!
//! Teaching note: realtime audio is just "fill this buffer before the
//! deadline, thousands of times a second". This module does exactly that
//! fill, minus the hardware: [`RenderGraph::render`] takes a block length
//! and returns every sink's samples. The cpal callback in [`device`](super::device)
//! calls the same code path, so offline tests prove realtime behavior.
//!
//! Two rules keep it realtime-safe: no allocation inside the per-node
//! process step (buffers are pre-sized), and node order comes from the
//! [`Schedule`](super::schedule::Schedule) levels — nodes in one level
//! are independent, so multicore rendering is bit-identical to serial.

use std::collections::BTreeMap;

use super::graph::AudioGraph;
use super::schedule::{Schedule, ScheduleError};

/// What one node does to a block. Deliberately tiny: v0 proves the graph,
/// schedule, and compensation machinery; real instruments/effects (Track B
/// agent 2+) add variants without touching the plumbing.
#[derive(Debug, Clone, PartialEq)]
pub enum Proc {
    /// Emits silence. The null-render contract: silence in = silence out.
    Null,
    /// Emits 1.0 at frame 0 of the render, else 0.0. Test probe only.
    Impulse,
    /// Emits a constant value every frame.
    Constant(f32),
    /// Pass-through that delays its summed input by `latency` samples
    /// (models lookahead DSP; must match the graph's `latency_samples`).
    Delay(u64),
    /// Sums its inputs sample by sample.
    Mix,
}

/// Executable form of an [`AudioGraph`]: signal topology plus one [`Proc`]
/// per node. Nodes default to [`Proc::Null`]; tests assign probes.
#[derive(Debug, Clone, Default)]
pub struct RenderGraph {
    pub topo: AudioGraph,
    procs: BTreeMap<String, Proc>,
}

impl RenderGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adopt a routing graph wholesale (all nodes start [`Proc::Null`]).
    pub fn from_audio_graph(topo: AudioGraph) -> Self {
        Self {
            topo,
            procs: BTreeMap::new(),
        }
    }

    pub fn set_proc(&mut self, id: &str, proc: Proc) {
        self.procs.insert(id.to_string(), proc);
    }

    pub fn proc_of(&self, id: &str) -> &Proc {
        self.procs.get(id).unwrap_or(&Proc::Null)
    }

    /// Direct signal producers of `node`.
    pub fn producers_of(&self, node: &str) -> Vec<String> {
        self.topo.producers_of(node)
    }

    /// Render `frames` samples for every node, returned as mono buffers keyed
    /// by node id. `threads == 1` renders serially; higher values split each
    /// schedule level across scoped threads — bit-identical either way.
    /// `edge_delays` carries [`AudioGraph::all_edge_delays`] compensation
    /// (pass an empty map for the uncompensated behavior).
    pub fn render(
        &self,
        frames: usize,
        threads: usize,
        edge_delays: &BTreeMap<(String, String), u64>,
    ) -> Result<BTreeMap<String, Vec<f32>>, ScheduleError> {
        let schedule = Schedule::build(&self.topo)?;
        schedule.require_all_local()?;
        let mut buffers: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        for level in &schedule.levels {
            if threads <= 1 || level.len() < 2 {
                for id in level {
                    buffers.insert(id.clone(), self.render_node(id, frames, &buffers, edge_delays));
                }
            } else {
                // Scoped threads borrow `buffers` read-only; each thread
                // writes exactly its own node's buffer. No locks, no copies
                // of other nodes' audio.
                let outs: Vec<(String, Vec<f32>)> = std::thread::scope(|s| {
                    let handles: Vec<_> = level
                        .iter()
                        .map(|id| {
                            s.spawn(|| {
                                (
                                    id.clone(),
                                    self.render_node(id, frames, &buffers, edge_delays),
                                )
                            })
                        })
                        .collect();
                    handles.into_iter().map(|h| h.join().expect("render thread")).collect()
                });
                for (id, buf) in outs {
                    buffers.insert(id, buf);
                }
            }
        }
        Ok(buffers)
    }

    /// Render one node's block from its (already rendered, compensated)
    /// producers. Pure function of inputs: the multicore seam.
    fn render_node(
        &self,
        id: &str,
        frames: usize,
        buffers: &BTreeMap<String, Vec<f32>>,
        edge_delays: &BTreeMap<(String, String), u64>,
    ) -> Vec<f32> {
        // Sum producer buffers, each shifted by its compensation delay.
        // A delay of d drops the producer's first d frames into the line
        // and pulls d leading zeros — exactly the mix-point alignment the
        // graph computed.
        //
        // Realtime note: the delay-table key needs owned strings, so borrow
        // the producer's audio *before* moving its id into the key. That is
        // one `String` alloc per edge instead of two (`id` is hoisted and
        // cloned once per edge; `producer` is moved, never cloned). The
        // remaining per-edge alloc disappears only with interned node ids —
        // tracked in docs/notes/rt-perf.md as follow-up RT-4.
        let mut input = vec![0.0f32; frames];
        let id_owned = id.to_string();
        for producer in self.producers_of(id) {
            let buf = buffers.get(producer.as_str());
            let delay = edge_delays
                .get(&(producer, id_owned.clone()))
                .copied()
                .unwrap_or(0) as usize;
            if let Some(buf) = buf {
                for t in 0..frames {
                    if t >= delay {
                        input[t] += buf[t - delay];
                    }
                }
            }
        }
        match self.proc_of(id) {
            Proc::Null => vec![0.0; frames],
            Proc::Impulse => {
                // Impulse sources ignore their inputs (they *are* the signal).
                let mut out = vec![0.0; frames];
                if frames > 0 {
                    out[0] = 1.0;
                }
                out
            }
            Proc::Constant(v) => vec![*v; frames],
            Proc::Delay(latency) => {
                let latency = *latency as usize;
                let mut out = vec![0.0; frames];
                for t in 0..frames {
                    if t >= latency {
                        out[t] = input[t - latency];
                    }
                }
                out
            }
            Proc::Mix => input,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeKind, Project, Track};

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

    /// `src -> dly(64) -> mix`, `src -> fast -> mix` hmm — simpler canonical
    /// rig: impulse `a` through a 64-sample delay into `mix`, impulse `b`
    /// straight into `mix`. Graph latencies must mirror the procs.
    fn rig() -> (RenderGraph, BTreeMap<(String, String), u64>) {
        let mut p = Project::new("p", "Render");
        p.tracks.push(track("a"));
        p.tracks.push(track("b"));
        p.devices.push(crate::model::Node {
            id: "dly".to_string(),
            kind: crate::model::NodeKind::Device,
            name: "dly".to_string(),
            params: vec![crate::model::Param {
                id: crate::audio::graph::LATENCY_PARAM.to_string(),
                label: "Latency".to_string(),
                value: 64.0,
                min: 0.0,
                max: 8192.0,
                default: 0.0,
                unit: "samples".to_string(),
            }],
        });
        let mut n = 0;
        for (from, to) in [("a", "dly"), ("dly", "mix"), ("b", "mix")] {
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
        let topo = AudioGraph::from_project(&p);
        let delays = topo.all_edge_delays().expect("acyclic");
        let mut g = RenderGraph::from_audio_graph(topo);
        g.set_proc("a", Proc::Impulse);
        g.set_proc("b", Proc::Impulse);
        g.set_proc("dly", Proc::Delay(64));
        g.set_proc("mix", Proc::Mix);
        (g, delays)
    }

    #[test]
    fn render_null_silence_in_silence_out() {
        // Every node Null (the default): the whole graph must be bit-zero.
        let mut p = Project::new("p", "Null");
        p.tracks.push(track("a"));
        p.routing.push(Edge {
            id: "e1".to_string(),
            from_node: "a".to_string(),
            from_port: "out".to_string(),
            to_node: "mix".to_string(),
            to_port: "in".to_string(),
            kind: EdgeKind::Audio,
        });
        let g = RenderGraph::from_audio_graph(AudioGraph::from_project(&p));
        let out = g.render(512, 4, &BTreeMap::new()).expect("renders");
        for (id, buf) in &out {
            assert!(buf.iter().all(|&s| s == 0.0), "{id} must be silent");
        }
        assert_eq!(out["mix"].len(), 512);
    }

    #[test]
    fn latency_compensation_aligns_mix_inputs() {
        let (g, delays) = rig();
        // The graph must ask for exactly 64 samples on the fast path.
        assert_eq!(
            delays.get(&("b".to_string(), "mix".to_string())).copied(),
            Some(64)
        );
        assert_eq!(
            delays.get(&("dly".to_string(), "mix".to_string())).copied(),
            Some(0)
        );

        // Compensated: both impulses land on frame 64 -> peak of 2.0 there,
        // zeros everywhere else.
        let out = g.render(256, 1, &delays).expect("renders");
        let mix = &out["mix"];
        assert_eq!(mix.len(), 256);
        for (t, &s) in mix.iter().enumerate() {
            if t == 64 {
                assert_eq!(s, 2.0, "both paths arrive at frame 64");
            } else {
                assert_eq!(s, 0.0, "frame {t} must be silent");
            }
        }

        // Uncompensated (empty delays): the fast impulse leaks at frame 0
        // while the delayed one lands at 64 — audibly a flam, measurably a
        // misalignment. This is the bug compensation exists to fix.
        let raw = g.render(256, 1, &BTreeMap::new()).expect("renders");
        assert_eq!(raw["mix"][0], 1.0);
        assert_eq!(raw["mix"][64], 1.0);
    }

    #[test]
    fn multicore_render_is_bit_identical() {
        let (g, delays) = rig();
        let serial = g.render(256, 1, &delays).expect("serial");
        let parallel = g.render(256, 8, &delays).expect("parallel");
        assert_eq!(serial, parallel);
    }
}
