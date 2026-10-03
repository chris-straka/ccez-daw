//! TCP transport: DSP-node server and failover session.
//!
//! Teaching note: the network is *never* trusted with correctness, only
//! with speed. Every block the server renders, the client can render
//! too (same [`RemoteExecutor`], same procs) — the server is a fast
//! path, not the truth. That single rule gives all three headline
//! behaviors for free: loopback output is bit-identical by construction,
//! a dead server just means "render locally" (sticky [`SessionStatus`]),
//! and sequence numbers only have to detect *which* blocks to fill in
//! locally, never to repair audio.
//!
//! Latency hiding is pipelining, not prediction: [`RemoteSession::render_many`]
//! sends several `RenderRequest`s before reading, so the server never
//! idles waiting for the next request. Depth comes from
//! [`LatencyEstimator::recommended_depth`].

use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::audio::render::RenderGraph;

use super::codec::{self, CodecError, Message};
use super::executor::{render_subset, RemoteExecutor};
use super::latency::LatencyEstimator;
use super::partition::Partition;

/// Largest block the server will render in one request (2^20 frames ≈
/// 21s at 48kHz mono ≈ 4 MiB — anything bigger is a bug, not audio).
pub const MAX_FRAMES: usize = 1 << 20;

#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    Codec(CodecError),
    Server(String),
    SeqMismatch { expected: u64, got: u64 },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "transport io: {e}"),
            Self::Codec(e) => write!(f, "transport codec: {e}"),
            Self::Server(m) => write!(f, "remote error: {m}"),
            Self::SeqMismatch { expected, got } => {
                write!(f, "sequence mismatch: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for TransportError {}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<CodecError> for TransportError {
    fn from(e: CodecError) -> Self {
        Self::Codec(e)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// A localhost DSP node: owns the full graph (same project as the
/// client), renders only its remote subset per request.
pub struct RemoteServer {
    listener: TcpListener,
    executor: RemoteExecutor,
    name: String,
}

impl RemoteServer {
    /// Bind (port 0 picks an ephemeral loopback port). Returns the server
    /// plus the address clients dial.
    pub fn bind<A: ToSocketAddrs>(
        addr: A,
        executor: RemoteExecutor,
        name: &str,
    ) -> Result<(Self, SocketAddr), TransportError> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr()?;
        Ok((
            Self {
                listener,
                executor,
                name: name.to_string(),
            },
            local,
        ))
    }

    /// Serve forever: one thread per connection, sequential requests per
    /// connection. Returns only on accept failure.
    pub fn run(self) -> Result<(), TransportError> {
        for stream in self.listener.incoming() {
            let stream = stream?;
            let exec = self.executor.clone();
            let name = self.name.clone();
            std::thread::spawn(move || {
                let _ = serve_conn(stream, &exec, &name);
            });
        }
        Ok(())
    }

    /// Serve exactly `n` connections (then join handlers and return).
    /// Test and single-client seam; production uses [`Self::run`].
    pub fn run_n(self, n: usize) -> Result<(), TransportError> {
        let mut handles = Vec::new();
        for stream in self.listener.incoming().take(n) {
            let stream = stream?;
            let exec = self.executor.clone();
            let name = self.name.clone();
            handles.push(std::thread::spawn(move || {
                let _ = serve_conn(stream, &exec, &name);
            }));
        }
        for h in handles {
            let _ = h.join();
        }
        Ok(())
    }
}

fn serve_conn(stream: TcpStream, exec: &RemoteExecutor, name: &str) -> Result<(), TransportError> {
    let mut stream = stream;
    loop {
        match codec::read_message(&mut stream) {
            Ok(Message::Hello { .. }) => {
                codec::write_message(
                    &mut stream,
                    &Message::Hello {
                        name: name.to_string(),
                    },
                )?;
            }
            Ok(Message::Ping { seq, t_send_ms }) => {
                codec::write_message(&mut stream, &Message::Pong { seq, t_send_ms })?;
            }
            Ok(Message::RenderRequest {
                block,
                seq,
                frames,
                inputs,
            }) => {
                let reply = execute_request(exec, block, seq, frames as usize, &inputs);
                codec::write_message(&mut stream, &reply)?;
            }
            Ok(other) => {
                let _ = codec::write_message(
                    &mut stream,
                    &Message::RenderError {
                        block: u64::MAX,
                        msg: format!("unexpected message: {other:?}"),
                    },
                );
            }
            Err(CodecError::Truncated) => return Ok(()), // orderly EOF
            Err(e) => return Err(TransportError::Codec(e)),
        }
    }
}

fn execute_request(
    exec: &RemoteExecutor,
    block: u64,
    seq: u64,
    frames: usize,
    inputs: &BTreeMap<String, Vec<f32>>,
) -> Message {
    if frames == 0 || frames > MAX_FRAMES {
        return Message::RenderError {
            block,
            msg: format!("bad frame count {frames}"),
        };
    }
    if inputs.values().any(|b| b.len() != frames) {
        return Message::RenderError {
            block,
            msg: "boundary buffer length != frames".to_string(),
        };
    }
    // Loop position from the block index (exact while every block in the
    // stream renders the same frame count, the norm for device callbacks).
    let start_frame = block * frames as u64;
    Message::RenderResponse {
        block,
        seq,
        degraded: false,
        outputs: exec.execute(inputs, frames, start_frame),
    }
}

// ---------------------------------------------------------------------------
// Client session
// ---------------------------------------------------------------------------

/// Knobs for the client side of the link.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    /// Pipelined in-flight blocks for [`RemoteSession::render_many`].
    pub prefetch_depth: usize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(2),
            prefetch_depth: 4,
        }
    }
}

/// Liveness of the link. Sticky: once `Degraded`, the session renders
/// locally until [`RemoteSession::reconnect`] succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Live,
    Degraded { reason: String },
}

/// How one block was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSource {
    /// Server answered in sequence.
    Remote,
    /// Filled in locally (failover). Bit-identical to a local render.
    LocalFailover,
}

/// One rendered block: every node id (local and remote) maps to its
/// `frames` samples.
#[derive(Debug, Clone)]
pub struct BlockOutput {
    pub block: u64,
    pub source: BlockSource,
    pub buffers: BTreeMap<String, Vec<f32>>,
    /// Measured request→response turnaround, when remote.
    pub rtt_ms: Option<f64>,
}

/// Client end of the DSP link. Owns the fallback graph so failover needs
/// no server, no re-setup, and no dropped samples — just local CPU.
pub struct RemoteSession {
    addr: SocketAddr,
    stream: Option<TcpStream>,
    graph: RenderGraph,
    local_order: Vec<String>,
    failover_exec: RemoteExecutor,
    edge_delays: BTreeMap<(String, String), u64>,
    estimator: LatencyEstimator,
    next_seq: u64,
    status: SessionStatus,
    options: SessionOptions,
}

impl RemoteSession {
    /// Dial the server (Hello handshake) and prepare the session. The
    /// `graph` is the full local graph — also the failover renderer.
    pub fn connect(
        addr: SocketAddr,
        graph: RenderGraph,
        partition: Partition,
        edge_delays: BTreeMap<(String, String), u64>,
        options: SessionOptions,
    ) -> Result<Self, TransportError> {
        let local_order = partition
            .local_order(&graph.topo)
            .map_err(|e| TransportError::Server(format!("bad partition: {e}")))?;
        let remote_order = partition
            .remote_order(&graph.topo)
            .map_err(|e| TransportError::Server(format!("bad partition: {e}")))?;
        let failover_exec =
            RemoteExecutor::new(graph.clone(), remote_order, edge_delays.clone());
        let mut s = Self {
            addr,
            stream: None,
            graph,
            local_order,
            failover_exec,
            edge_delays,
            estimator: LatencyEstimator::new(),
            next_seq: 0,
            status: SessionStatus::Live,
            options,
        };
        s.dial()?;
        Ok(s)
    }

    fn dial(&mut self) -> Result<(), TransportError> {
        let stream = TcpStream::connect_timeout(&self.addr, self.options.connect_timeout)?;
        stream.set_read_timeout(Some(self.options.io_timeout))?;
        stream.set_write_timeout(Some(self.options.io_timeout))?;
        let mut stream = stream;
        codec::write_message(
            &mut stream,
            &Message::Hello {
                name: "ccez-daw client".to_string(),
            },
        )?;
        match codec::read_message(&mut stream)? {
            Message::Hello { .. } => {
                self.stream = Some(stream);
                self.status = SessionStatus::Live;
                Ok(())
            }
            other => Err(TransportError::Server(format!(
                "bad handshake reply: {other:?}"
            ))),
        }
    }

    pub fn status(&self) -> &SessionStatus {
        &self.status
    }

    pub fn estimator(&self) -> &LatencyEstimator {
        &self.estimator
    }

    /// Round-trip probe in milliseconds. Errors degrade the session —
    /// a link that won't pong won't carry audio either.
    pub fn ping(&mut self) -> Result<f64, TransportError> {
        let t0 = now_ms() as f64;
        let seq = self.next_seq;
        self.next_seq += 1;
        let live = self
            .stream
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| TransportError::Server("not connected".to_string()));
        if live.is_err() {
            self.degrade("ping while disconnected");
            return live.map(|_| 0.0);
        }
        let stream = self.stream.as_mut().unwrap();
        let send_ms = now_ms();
        let res: Result<f64, TransportError> = (|| {
            codec::write_message(stream, &Message::Ping { seq, t_send_ms: send_ms })?;
            match codec::read_message(stream)? {
                Message::Pong {
                    seq: got,
                    t_send_ms,
                } => {
                    if got != seq || t_send_ms != send_ms {
                        return Err(TransportError::SeqMismatch {
                            expected: seq,
                            got,
                        });
                    }
                    Ok(now_ms() as f64 - t0)
                }
                other => Err(TransportError::Server(format!(
                    "bad pong reply: {other:?}"
                ))),
            }
        })();
        match res {
            Ok(rtt) => {
                self.estimator.observe_rtt_ms(rtt);
                Ok(rtt)
            }
            Err(e) => {
                self.degrade(&e.to_string());
                Err(e)
            }
        }
    }

    fn degrade(&mut self, reason: &str) {
        self.stream = None;
        self.status = SessionStatus::Degraded {
            reason: reason.to_string(),
        };
    }

    /// Try to come back after a [`SessionStatus::Degraded`]. On success
    /// the next block goes remote again; on failure the session stays
    /// degraded with the new reason.
    pub fn reconnect(&mut self) -> Result<(), TransportError> {
        match self.dial() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.degrade(&e.to_string());
                Err(e)
            }
        }
    }

    /// Render every local node whose producers are already in `buffers`
    /// (one sweep in signal-flow order). Returns the newly rendered ids.
    /// `start_frame` positions [`Proc::Loop`] reads (block * frames).
    fn render_available(
        &self,
        buffers: &mut BTreeMap<String, Vec<f32>>,
        frames: usize,
        start_frame: u64,
    ) -> Vec<String> {
        let mut done = Vec::new();
        for id in &self.local_order {
            if buffers.contains_key(id) {
                continue;
            }
            let ready = self
                .graph
                .topo
                .producers_of(id)
                .iter()
                .all(|p| buffers.contains_key(p));
            if ready {
                done.push(id.clone());
            }
        }
        if !done.is_empty() {
            render_subset(&self.graph, &done, buffers, frames, &self.edge_delays, start_frame);
        }
        done
    }

    /// Render one block through the partition. Live: upstream locals,
    /// one network round-trip, downstream locals. Degraded (or on any
    /// transport failure): the remote subset renders locally — output
    /// stays bit-identical to a full local render.
    pub fn render_block(&mut self, block: u64, frames: usize) -> BlockOutput {
        if self.status != SessionStatus::Live || self.stream.is_none() {
            return self.render_local(block, frames);
        }
        let seq = self.next_seq;
        self.next_seq += 1;

        let mut buffers: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        self.render_available(&mut buffers, frames, block * frames as u64);

        let t0 = now_ms() as f64;
        let outcome: Result<BTreeMap<String, Vec<f32>>, TransportError> = (|| {
            let stream = self.stream.as_mut().unwrap();
            codec::write_message(
                stream,
                &Message::RenderRequest {
                    block,
                    seq,
                    frames: frames as u32,
                    inputs: buffers.clone(),
                },
            )?;
            match codec::read_message(stream)? {
                Message::RenderResponse {
                    block: got_block,
                    seq: got_seq,
                    outputs,
                    ..
                } => {
                    if got_seq != seq || got_block != block {
                        return Err(TransportError::SeqMismatch {
                            expected: seq,
                            got: got_seq,
                        });
                    }
                    Ok(outputs)
                }
                Message::RenderError { msg, .. } => Err(TransportError::Server(msg)),
                other => Err(TransportError::Server(format!("bad reply: {other:?}"))),
            }
        })();

        match outcome {
            Ok(outputs) => {
                let rtt = now_ms() as f64 - t0;
                self.estimator.observe_rtt_ms(rtt);
                self.estimator.observe_block(true);
                for (id, buf) in outputs {
                    buffers.insert(id, buf);
                }
                self.render_available(&mut buffers, frames, block * frames as u64);
                BlockOutput {
                    block,
                    source: BlockSource::Remote,
                    buffers,
                    rtt_ms: Some(rtt),
                }
            }
            Err(e) => {
                self.degrade(&e.to_string());
                self.estimator.observe_block(false);
                self.render_local(block, frames)
            }
        }
    }

    /// Latency hiding: pipeline `count` blocks — send them all, then read
    /// all replies in sequence order. Requests beyond a mid-stream
    /// failure (and any seq-mismatched reply) fall back to local render,
    /// so the returned vec always has `count` complete blocks.
    pub fn render_many(&mut self, start_block: u64, count: usize, frames: usize) -> Vec<BlockOutput> {
        if self.status != SessionStatus::Live || self.stream.is_none() {
            return (0..count)
                .map(|i| self.render_local(start_block + i as u64, frames))
                .collect();
        }
        // Send phase: render each block's upstream inputs locally, ship all.
        let mut pending: Vec<(u64, u64, BTreeMap<String, Vec<f32>>, f64)> = Vec::new();
        let stream_ok = (|| -> Result<(), TransportError> {
            for i in 0..count {
                let block = start_block + i as u64;
                let seq = self.next_seq;
                self.next_seq += 1;
                let mut buffers: BTreeMap<String, Vec<f32>> = BTreeMap::new();
                self.render_available(&mut buffers, frames, block * frames as u64);
                let stream = self.stream.as_mut().unwrap();
                codec::write_message(
                    stream,
                    &Message::RenderRequest {
                        block,
                        seq,
                        frames: frames as u32,
                        inputs: buffers.clone(),
                    },
                )?;
                pending.push((block, seq, buffers, now_ms() as f64));
                if pending.len() >= self.options.prefetch_depth.max(1) {
                    break;
                }
            }
            Ok(())
        })();
        if let Err(e) = stream_ok {
            self.degrade(&e.to_string());
            return (0..count)
                .map(|i| self.render_local(start_block + i as u64, frames))
                .collect();
        }
        // Drain phase: read in send order, fill gaps locally.
        let mut out = Vec::with_capacity(count);
        for (block, seq, mut buffers, t0) in pending.drain(..) {
            let reply: Result<BTreeMap<String, Vec<f32>>, TransportError> = (|| {
                let stream = self.stream.as_mut().unwrap();
                match codec::read_message(stream)? {
                    Message::RenderResponse {
                        block: gb,
                        seq: gs,
                        outputs,
                        ..
                    } => {
                        if gs != seq || gb != block {
                            return Err(TransportError::SeqMismatch {
                                expected: seq,
                                got: gs,
                            });
                        }
                        Ok(outputs)
                    }
                    Message::RenderError { msg, .. } => Err(TransportError::Server(msg)),
                    other => Err(TransportError::Server(format!("bad reply: {other:?}"))),
                }
            })();
            match reply {
                Ok(outputs) => {
                    let rtt = now_ms() as f64 - t0;
                    self.estimator.observe_rtt_ms(rtt);
                    self.estimator.observe_block(true);
                    for (id, buf) in outputs {
                        buffers.insert(id, buf);
                    }
                    self.render_available(&mut buffers, frames, block * frames as u64);
                    out.push(BlockOutput {
                        block,
                        source: BlockSource::Remote,
                        buffers,
                        rtt_ms: Some(rtt),
                    });
                }
                Err(e) => {
                    self.degrade(&e.to_string());
                    self.estimator.observe_block(false);
                    out.push(self.render_local(block, frames));
                    // Remainder goes local — the link is down, not slow.
                    let done = out.len();
                    for i in done..count {
                        out.push(self.render_local(start_block + i as u64, frames));
                    }
                    return out;
                }
            }
        }
        // Any blocks beyond the first prefetch window (count > depth):
        // continue live one at a time.
        let mut next = start_block + out.len() as u64;
        while out.len() < count {
            out.push(self.render_block(next, frames));
            next += 1;
        }
        out
    }

    /// Full-local render of one block: upstream locals, remote subset via
    /// the failover executor, downstream locals.
    fn render_local(&mut self, block: u64, frames: usize) -> BlockOutput {
        let start_frame = block * frames as u64;
        let mut buffers: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        self.render_available(&mut buffers, frames, start_frame);
        let remote_out = self.failover_exec.execute(&buffers, frames, start_frame);
        for (id, buf) in remote_out {
            buffers.insert(id, buf);
        }
        self.render_available(&mut buffers, frames, start_frame);
        BlockOutput {
            block,
            source: BlockSource::LocalFailover,
            buffers,
            rtt_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::graph::AudioGraph;
    use crate::audio::render::{Proc, RenderGraph};
    use crate::model::{EdgeKind, Project};
    use crate::remotenodes::partition::{device, edge, track};

    fn rig() -> (RenderGraph, BTreeMap<(String, String), u64>) {
        let mut p = Project::new("p", "Transport");
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

    fn opts() -> SessionOptions {
        SessionOptions {
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(2),
            prefetch_depth: 4,
        }
    }

    fn spawn_server(rg: RenderGraph, delays: BTreeMap<(String, String), u64>) -> SocketAddr {
        let g = rg.topo.clone();
        let part = Partition::split(&g, &["dly"]).unwrap();
        let order = part.remote_order(&g).unwrap();
        let exec = RemoteExecutor::new(rg, order, delays);
        let (server, addr) = RemoteServer::bind("127.0.0.1:0", exec, "test-farm").unwrap();
        std::thread::spawn(move || {
            let _ = server.run_n(1);
        });
        // run_n(1) returns after one connection closes; give the listener
        // a moment to exist before the client dials.
        std::thread::sleep(Duration::from_millis(50));
        addr
    }

    fn session(addr: SocketAddr) -> RemoteSession {
        let (rg, delays) = rig();
        let part = Partition::split(&rg.topo, &["dly"]).unwrap();
        RemoteSession::connect(addr, rg, part, delays, opts()).unwrap()
    }

    #[test]
    fn loopback_render_equivalence() {
        let (rg, delays) = rig();
        let addr = spawn_server(rg.clone(), delays.clone());
        let mut sess = session(addr);
        assert_eq!(sess.status(), &SessionStatus::Live);
        let rtt = sess.ping().unwrap();
        assert!(rtt < 2000.0, "loopback ping absurd: {rtt}ms");

        let frames = 128;
        let expected = rg.render(frames, 1, &delays, 0).unwrap();
        for block in 0..4 {
            let got = sess.render_block(block, frames);
            assert_eq!(got.source, BlockSource::Remote, "block {block} went local");
            assert_eq!(got.block, block);
            for (id, buf) in &expected {
                assert_eq!(got.buffers.get(id), Some(buf), "block {block} node `{id}`");
            }
        }
        // Estimator learned something from real traffic.
        assert!(sess.estimator().smoothed_ms() >= 0.0);
        assert!(sess.estimator().recommended_depth(5.0) >= 1);
    }

    #[test]
    fn pipelined_many_matches_local() {
        let (rg, delays) = rig();
        let addr = spawn_server(rg.clone(), delays.clone());
        let mut sess = session(addr);
        let frames = 128;
        let expected = rg.render(frames, 1, &delays, 0).unwrap();
        let outs = sess.render_many(0, 6, frames);
        assert_eq!(outs.len(), 6);
        for got in &outs {
            assert_eq!(got.source, BlockSource::Remote);
            for (id, buf) in &expected {
                assert_eq!(got.buffers.get(id), Some(buf), "node `{id}`");
            }
        }
    }

    #[test]
    fn disconnect_fails_over_to_identical_local() {
        let (rg, delays) = rig();
        // Live session first, then the link dies mid-stream (dropped
        // socket, the same shape as a killed server process).
        let live = spawn_server(rg.clone(), delays.clone());
        let mut sess = session(live);
        let first = sess.render_block(0, 128);
        assert_eq!(first.source, BlockSource::Remote);
        drop(sess.stream.take());
        sess.degrade("test kill: server process gone");

        assert!(matches!(sess.status(), SessionStatus::Degraded { .. }));
        let frames = 128;
        let expected = rg.render(frames, 1, &delays, 0).unwrap();
        let got = sess.render_block(0, frames);
        assert_eq!(got.source, BlockSource::LocalFailover);
        for (id, buf) in &expected {
            assert_eq!(got.buffers.get(id), Some(buf), "failover node `{id}`");
        }
        // Stays degraded, keeps rendering.
        let got2 = sess.render_block(1, frames);
        assert_eq!(got2.source, BlockSource::LocalFailover);
    }

    #[test]
    fn server_rejects_bad_frames() {
        let (rg, delays) = rig();
        let addr = spawn_server(rg.clone(), delays.clone());
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        codec::write_message(&mut stream, &Message::Hello { name: "t".into() }).unwrap();
        let _ = codec::read_message(&mut stream).unwrap();
        codec::write_message(
            &mut stream,
            &Message::RenderRequest {
                block: 0,
                seq: 0,
                frames: 128,
                inputs: {
                    let mut m = BTreeMap::new();
                    m.insert("a".to_string(), vec![0.0; 64]); // wrong length
                    m
                },
            },
        )
        .unwrap();
        assert!(matches!(
            codec::read_message(&mut stream).unwrap(),
            Message::RenderError { .. }
        ));
    }
}
