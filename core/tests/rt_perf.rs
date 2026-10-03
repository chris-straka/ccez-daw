//! Realtime performance gate: audio-callback hot paths under budget.
//!
//! Run: `cargo test --manifest-path core/Cargo.toml --test rt_perf`.
//!
//! What this proves: the three hottest callback-reachable paths — one graph
//! render block, one mixer-style sum, one plugin-chain wet/dry pass — finish
//! well inside a 512-frame / 44.1 kHz callback budget (~11.6 ms) on ordinary
//! CI hardware. Budgets are deliberately generous (10x+ headroom over
//! observed times) so this gate catches regressions, not noise: it fails
//! when an algorithmic cliff lands (per-block clone, lock, or pipe round
//! trip), never on a slow CI minute.
//!
//! What this does NOT prove: allocation-freedom. Timing is a proxy; the
//! allocation audit itself lives in docs/notes/rt-perf.md (violations
//! RT-1..RT-7 with file:line). A future `#[global_allocator]` counting
//! harness can promote the proxy to a hard guarantee.
//!
//! Budgets (512 frames unless noted; see docs/notes/rt-perf.md):
//! - graph render block (4-node compensated rig): < 50 ms for 200 blocks
//!   (i.e. < 250 us mean per block).
//! - mixer sum (8 stems x 512 frames, `Proc::Mix`): < 100 ms for 200 blocks.
//! - plugin chain math (`apply_wet_dry` x 512 frames): < 50 ms for 2000 passes.
//! - callback handoff (10k params + pump): completes, drained == 10_002.
//!   (Throughput sanity for the UI->audio channel, not a latency claim.)

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ccez_core::audio::graph::AudioGraph;
use ccez_core::audio::render::{Proc, RenderGraph};
use ccez_core::audio::{AudioBackend, AudioCommand, AudioEngine, NullBackend};
use ccez_core::model::{Edge, EdgeKind, Node, NodeKind, Param, Project, Track};
use ccez_core::plugins::chain::apply_wet_dry;

const FRAMES: usize = 512;
const GRAPH_BUDGET: Duration = Duration::from_millis(50);
const GRAPH_ITERS: u32 = 200;
const MIX_BUDGET: Duration = Duration::from_millis(100);
const MIX_ITERS: u32 = 200;
const CHAIN_BUDGET: Duration = Duration::from_millis(50);
const CHAIN_ITERS: u32 = 2000;

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

/// Compensated rig: impulse `a` through a 64-sample delay into `mix`,
/// impulse `b` straight into `mix` (matches the render.rs unit rig).
fn rig() -> (RenderGraph, BTreeMap<(String, String), u64>) {
    let mut p = Project::new("p", "Perf");
    p.tracks.push(track("a"));
    p.tracks.push(track("b"));
    p.devices.push(Node {
        id: "dly".to_string(),
        kind: NodeKind::Device,
        name: "dly".to_string(),
        params: vec![Param {
            id: ccez_core::audio::graph::LATENCY_PARAM.to_string(),
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

/// Eight constant stems summed at `mix`: the mixer-sum hot path in graph
/// form (the widest fan-in a v0 mix bus sees).
fn mix_rig(stems: usize) -> (RenderGraph, BTreeMap<(String, String), u64>) {
    let mut p = Project::new("p", "MixPerf");
    for i in 0..stems {
        p.tracks.push(track(&format!("s{i}")));
        p.routing.push(Edge {
            id: format!("e{i}"),
            from_node: format!("s{i}"),
            from_port: "out".to_string(),
            to_node: "mix".to_string(),
            to_port: "in".to_string(),
            kind: EdgeKind::Audio,
        });
    }
    let topo = AudioGraph::from_project(&p);
    let delays = topo.all_edge_delays().expect("acyclic");
    let mut g = RenderGraph::from_audio_graph(topo);
    for i in 0..stems {
        g.set_proc(&format!("s{i}"), Proc::Constant(0.125));
    }
    g.set_proc("mix", Proc::Mix);
    (g, delays)
}

#[test]
fn graph_render_block_stays_in_budget() {
    let (g, delays) = rig();
    // Warm up once (page faults, first BTreeMap grows), then time.
    let first = g.render(FRAMES, 1, &delays, 0).expect("renders");
    assert_eq!(first["mix"][64], 2.0);
    let start = Instant::now();
    for _ in 0..GRAPH_ITERS {
        let out = g.render(FRAMES, 1, &delays, 0).expect("renders");
        std::hint::black_box(out["mix"][64]);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < GRAPH_BUDGET,
        "graph block over budget: {elapsed:?} for {GRAPH_ITERS} x {FRAMES}-frame blocks (budget {GRAPH_BUDGET:?})"
    );
}

#[test]
fn mixer_sum_stays_in_budget() {
    let (g, delays) = mix_rig(8);
    let first = g.render(FRAMES, 1, &delays, 0).expect("renders");
    assert!((first["mix"][0] - 1.0).abs() < 1e-6);
    let start = Instant::now();
    for _ in 0..MIX_ITERS {
        let out = g.render(FRAMES, 1, &delays, 0).expect("renders");
        std::hint::black_box(out["mix"][0]);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < MIX_BUDGET,
        "mixer sum over budget: {elapsed:?} for {MIX_ITERS} x {FRAMES}-frame blocks (budget {MIX_BUDGET:?})"
    );
}

#[test]
fn plugin_chain_wet_dry_math_stays_in_budget() {
    let dry: Vec<f32> = (0..FRAMES).map(|t| (t as f32) / (FRAMES as f32)).collect();
    let wet: Vec<f32> = dry.iter().map(|s| s * 0.5).collect();
    // Warm up once (page faults, allocator, CPU ramp) like the sibling
    // tests: without this the first timed pass eats cold-start noise and
    // the gate fails on a loaded minute instead of on real regressions.
    let mut acc = 0.0f32;
    for t in 0..FRAMES {
        acc += apply_wet_dry(dry[t], wet[t], 0.7, 0.3);
    }
    let start = Instant::now();
    for _ in 0..CHAIN_ITERS {
        let mut sum = 0.0f32;
        for t in 0..FRAMES {
            sum += apply_wet_dry(dry[t], wet[t], 0.7, 0.3);
        }
        acc += sum;
    }
    let elapsed = start.elapsed();
    std::hint::black_box(acc);
    assert!(
        elapsed < CHAIN_BUDGET,
        "chain math over budget: {elapsed:?} for {CHAIN_ITERS} x {FRAMES}-sample passes (budget {CHAIN_BUDGET:?})"
    );
}

#[test]
fn callback_handoff_absorbs_param_burst() {
    // The UI->audio channel contract under load: a 10k-param burst plus
    // swap plus stop must all arrive without blocking either side.
    // Completion (with exact drain count) IS the assertion.
    let (engine, rx, params) = AudioEngine::channel();
    let (g, delays) = rig();
    engine.send(AudioCommand::SwapGraph {
        graph: g,
        delays,
        out_node: "mix".to_string(),
    });
    let pump = std::thread::spawn(move || {
        let mut backend = NullBackend::new(rx, params);
        let mut iters = 0;
        while backend.pump(32) {
            iters += 1;
            assert!(iters < 1_000_000, "audio thread stalled");
        }
        backend.drained
    });
    for i in 0..10_000 {
        engine.set_param("mix:volume", (i as f32) / 10_000.0);
    }
    engine.send(AudioCommand::Stop);
    let drained = pump.join().expect("pump thread");
    assert_eq!(drained, 10_002);
}
