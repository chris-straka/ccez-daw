# P-Distributed: remote DSP nodes over local TCP

How one node graph renders across two processes — and why a dead server
sounds exactly like a live one.

## The one idea

A remote node is **not special DSP**. It is the *same* `Proc` math
(`core/src/audio/render.rs`), run in another process, fed the same input
buffers over a socket. The network is never trusted with correctness,
only with speed: every block the server renders, the client can render
too. That single rule buys all three headline behaviors:

- **Loopback equivalence** — client/server outputs are bit-identical by
  construction (pinned by `loopback_render_equivalence`).
- **Graceful degradation** — a dead server just means "render locally".
  Output stays sample-identical; only `BlockSource` flips to
  `LocalFailover` and `SessionStatus` sticks at `Degraded`.
- **No schema churn** — all wire/partition types are new and additive.
  Nothing frozen was touched; `bun run check` stays green.

## Map of `core/src/remotenodes/`

| Module | Job | Key type |
|---|---|---|
| `codec` | Length-prefixed binary frames (`u32 LE len + payload`), bincode-style LE layout | `Message` |
| `partition` | Cut the `AudioGraph` into local/remote halves; boundary edges | `Partition`, `CutEdges` |
| `executor` | Render one side of a cut, bit-identical to `RenderGraph` | `RemoteExecutor`, `render_subset` |
| `latency` | RTT smoothing (EWMA) + prefetch depth from worst-case jitter | `LatencyEstimator` |
| `transport` | DSP-node server + client session with sticky failover | `RemoteServer`, `RemoteSession` |
| `sched` / `sched_failover` | Partition *policy* + handshake + failover/offline bounce (pure planning, no sockets — §7) | `PartitionPolicy`, `Registry`, `FailoverPolicy`, `bounce_with_failover` |

Related seam: `Schedule::mark_remote` (`core/src/audio/schedule.rs`)
marks *which* nodes go remote; this crate moves their audio.

## The block flow

```text
client                                   server (127.0.0.1, std::net)
──────                                   ────────────────────────────
render_available() → upstream locals
  │  RenderRequest{seq, block, boundary bufs}
  ├─────────────────────────────────────────►│
  │                       RemoteExecutor::execute() → remote bufs
  │  RenderResponse{seq, block, remote bufs}
  │◄─────────────────────────────────────────┤
render_available() → downstream locals ◄── error/timeout? render
                                              remote subset locally
```

Sequence numbers pair each reply with its request; a mismatch, timeout,
or closed socket degrades the session **sticky** — it renders locally
until `reconnect()` succeeds. No retry storms, no partial blocks: the
returned `BlockOutput.buffers` always cover every node.

## Latency hiding without prediction

Two different numbers: *per-block latency* (one round trip) vs
*throughput* (blocks/second). `render_many` pipelines `prefetch_depth`
requests before reading, so the server never idles. Depth comes from
`recommended_depth(block_ms) = ceil(worst_rtt / block_ms) + 1` — worst
case, not mean, because the hiding budget must cover jitter. Feed it
with `ping()` probes or per-block turnarounds; `late_ratio()` tells you
when the depth is too shallow.

## Why no new dependencies

`postcard`/`bincode` would be the natural codec, but they add registry
deps to a crate that must build offline and stay minimal. `codec.rs` is
the single seam: its layout matches fixed-int LE conventions, so a
`postcard` derive can replace the hand codec later without touching
`transport.rs`. Same reason for `std::net` blocking I/O over tokio —
localhost audio blocks don't need an async runtime, and tests stay
deterministic (`run_n(1)` + ephemeral ports, no fixtures).

## Try it

```bash
cargo test --manifest-path core/Cargo.toml --lib remotenodes::
```

16 tests: codec round-trips (incl. NaN/subnormal bit-exactness),
cut/partition invariants, subset==full-render pin,
loopback equivalence, pipelined equivalence, disconnect failover,
bad-frame rejection. Plus the sibling track's `sched`/`sched_failover`
policy tests in the same directory.

## 7. The scheduling half: who goes remote, on what, and what if it dies

New to placement? Start here. The transport above moves audio; this half
decides *which* subgraphs move, *which* worker takes them, and what the
offline bounce does when a worker goes dark. All three answers are pure
functions over `Project` data — no sockets, no threads — so they are
deterministic: same project + same advertisements, same plan, every time.

**Partition policy** ([`PartitionPolicy`](../../../core/src/remotenodes/sched.rs)):
which nodes are worth offloading. Rules, in order:

1. Signal cycles fail first — an unrenderable graph is unpartitionable too.
2. Sinks (mix buses, no consumers) stay local: mix-point alignment happens
   where the delay lines live.
3. Only device nodes are candidates by default — tracks and buses stay put.
4. Nodes above `max_remote_latency` (default 512 samples) stay local:
   lookahead stacked on a network hop is worse than local DSP.
5. At most `floor(max_remote_share × total)` nodes go remote (default half,
   so one farm outage can never take the whole mix), heaviest latency
   first — the expensive DSP wins the offload slots.

**Load/capability handshake** ([`Registry`](../../../core/src/remotenodes/sched.rs)):
workers advertise `NodeCapability{endpoint, kinds, max_nodes, load, online}`;
`assign()` deals each candidate to the least-loaded online worker that
accepts its kind and still has capacity (ties by endpoint). No worker
fits → the node falls back to local *at plan time*; planning never strands
a node. `PartitionPlan::apply_to` then marks the winners through the
existing `Schedule::mark_remote` seam — the renderer is untouched.

**Failover + offline bounce** ([`sched_failover`](../../../core/src/remotenodes/sched_failover.rs)):
`FailoverPolicy::decide` maps (reachable?) to one fate — `UseRemote`,
`BounceLocal` (fail-open: render the orphaned subgraph locally), or `Abort`
(fail-closed: skip the track rather than guess). `bounce_with_failover`
runs that per track and reports `BounceReport{stems, fell_back, failed}` —
every stem delivered, every fallback named. The fail-open test pins the
key invariant: fallback audio is *sample-identical* to a pure-local bounce.

```rust
let mut plan = PartitionPolicy::default().partition(&project)?;
plan.assign_endpoints(&project, &registry); // handshake; unfit → local
let report = bounce_with_failover(&project, &plan, &reachable, &policy, &cfg)?;
assert!(report.failed.is_empty()); // fail-open delivered everything
```

Try it: `cargo test --manifest-path core/Cargo.toml --lib remotenodes::sched`
— 12 tests (partition rules, handshake, seam application, message
round-trips, failover decisions, healthy/dark/closed bounces). The full
`remotenodes::` set is 28 tests alongside the transport half above.

## Coordination note (parallel tracks)

`sched.rs`/`sched_failover.rs` and `mod.rs`/`lib.rs` are shared with the
sibling scheduling track: during this session we collided twice (a
dropped `pub mod remotenodes;` that silently compiled the directory
out, and a duplicated `mod.rs` merge). Both were merged additively; if
`remotenodes::` tests ever vanish from `cargo test -- --list` again,
check `lib.rs` still declares the module before debugging anything else.
