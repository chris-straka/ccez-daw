//! UDP peer-discovery half of Link-style sync (cargo feature `link-net`).
//!
//! Teaching note: [`super::link`] is "the band agrees on a tempo" for one
//! process; this module puts the agreement on the network so two processes
//! (or two machines on a LAN) play in the same session. Every node binds
//! one UDP socket, heartbeats its session state to known peers, and adopts
//! newer states it hears — last-writer-wins on the state's wall-clock
//! birth time, exactly like musicians following whoever most recently
//! counted in.
//!
//! Wiring: a [`UdpLinkNode`] joins an existing [`LinkBus`], so a transport
//! and a network node share one session with no transport changes:
//!
//! ```ignore
//! let bus = LinkBus::new(120.0);
//! let transport = TransportController::with_link_bus(&bus);
//! let node = UdpLinkNode::bind(&bus, "127.0.0.1:0".parse()?)?;
//! node.add_peer("127.0.0.1:9001".parse()?);
//! ```
//!
//! Design notes:
//!
//! - std only (`std::net::UdpSocket`), no new dependency — see
//!   `docs/notes/link-sync.md` for why no external Link crate was added.
//! - The background thread owns socket I/O; it touches the session only
//!   through brief mutex snapshots and the atomic adopt, so the audio
//!   callback (which never touches this module) is unaffected.
//! - Steady-state heartbeats are ignored by receivers: each packet carries
//!   the wall-clock *birth time* of the tempo/quantum state it transports
//!   (stamped at the edit, inherited on adoption — never refreshed by
//!   forwarding), and a packet is adopted only when its birth is newer
//!   than the local state's. Phase is therefore rebased exactly once per
//!   edit — a stale heartbeat can never clobber a newer local state no
//!   matter when it was sent, and converged peers exchange only ignored
//!   heartbeats, so the clock never jitters under load. Birth-time ordering
//!   assumes roughly synchronized clocks (true on one machine; NTP-level
//!   on a LAN); see the Limits note below.
//! - Feature-gated (`link-net`): default builds — and therefore default
//!   `cargo test` runs — compile no socket code at all. The loopback proof
//!   below additionally skips gracefully when the sandbox forbids UDP.
//!
//! Limits: both tempo and phase convergence assume roughly synchronized
//! wall clocks (beats are extrapolated by one-way flight time against the
//! sender's timestamp). Same-machine loopback agrees to well under a tenth
//! of a beat; cross-machine sync on an NTP-synced LAN is tempo-exact and
//! phase-best-effort, degrading toward follow-the-fastest-clock under heavy
//! skew. A future ping-pong clock-sync pass (the real Link timeline
//! protocol) could close that gap without changing this wire format's
//! shape.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use super::link::{LinkBus, LinkSession, unix_nanos_now};

/// Wire magic `"LNK1"` (big-endian u32). Packets with any other magic or
/// short length are dropped silently — a corrupt datagram must never poison
/// the session.
pub const LINK_UDP_MAGIC: u32 = 0x4c4e_4b31;
/// Broadcast heartbeat interval.
pub const HEARTBEAT: Duration = Duration::from_millis(50);
/// Socket recv quantum: small enough that `shutdown` joins promptly.
const RECV_TIMEOUT: Duration = Duration::from_millis(20);
/// `magic u32 | tempo f64 | quantum f64 | beats f64 | sent_nanos u64 | birth_nanos u64`.
pub const WIRE_LEN: usize = 4 + 8 + 8 + 8 + 8 + 8;

#[derive(Debug, Clone, Copy, PartialEq)]
struct LinkPacket {
    tempo_bpm: f64,
    quantum: f64,
    beats: f64,
    /// Send time (flight-time extrapolation only — never an ordering key).
    sent_nanos: u64,
    /// Birth time of the transported tempo/quantum state (the ordering
    /// key): stamped at the edit, inherited on adoption.
    birth_nanos: u64,
}

fn encode(p: &LinkPacket) -> [u8; WIRE_LEN] {
    let mut out = [0u8; WIRE_LEN];
    out[0..4].copy_from_slice(&LINK_UDP_MAGIC.to_be_bytes());
    out[4..12].copy_from_slice(&p.tempo_bpm.to_be_bytes());
    out[12..20].copy_from_slice(&p.quantum.to_be_bytes());
    out[20..28].copy_from_slice(&p.beats.to_be_bytes());
    out[28..36].copy_from_slice(&p.sent_nanos.to_be_bytes());
    out[36..44].copy_from_slice(&p.birth_nanos.to_be_bytes());
    out
}

fn decode(buf: &[u8]) -> Option<LinkPacket> {
    if buf.len() < WIRE_LEN {
        return None;
    }
    if u32::from_be_bytes(buf[0..4].try_into().ok()?) != LINK_UDP_MAGIC {
        return None;
    }
    Some(LinkPacket {
        tempo_bpm: f64::from_be_bytes(buf[4..12].try_into().ok()?),
        quantum: f64::from_be_bytes(buf[12..20].try_into().ok()?),
        beats: f64::from_be_bytes(buf[20..28].try_into().ok()?),
        sent_nanos: u64::from_be_bytes(buf[28..36].try_into().ok()?),
        birth_nanos: u64::from_be_bytes(buf[36..44].try_into().ok()?),
    })
}

struct NodeInner {
    session: LinkSession,
    socket: UdpSocket,
    peers: Mutex<Vec<SocketAddr>>,
    stop: AtomicBool,
}

/// One networked peer on a [`LinkBus`]: heartbeats the shared session to
/// known peers and adopts newer remote states. `Send + Sync`; dropping (or
/// [`UdpLinkNode::shutdown`]) stops the background thread.
pub struct UdpLinkNode {
    inner: Arc<NodeInner>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl UdpLinkNode {
    /// Bind `bind` (use port 0 for an ephemeral loopback port) and join
    /// `bus` as one more peer. Fails with `io::Error` when the sandbox
    /// forbids sockets — callers (including the test below) treat that as
    /// "no network here" and fall back to the local session.
    pub fn bind(bus: &LinkBus, bind: SocketAddr) -> io::Result<Self> {
        let session = bus.join();
        let socket = UdpSocket::bind(bind)?;
        socket.set_read_timeout(Some(RECV_TIMEOUT))?;
        let inner = Arc::new(NodeInner {
            session,
            socket: socket.try_clone()?,
            peers: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        drop(socket);
        let worker = Arc::clone(&inner);
        let thread = std::thread::spawn(move || run(worker));
        Ok(Self {
            inner,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Address this node is reachable at (share it with peers out of band).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.socket.local_addr()
    }

    /// Add a peer to heartbeat to (idempotent; self-addresses are ignored
    /// on send).
    pub fn add_peer(&self, peer: SocketAddr) {
        let mut peers = self.inner.peers.lock().expect("link-net peers lock");
        if !peers.contains(&peer) {
            peers.push(peer);
        }
    }

    /// Session tempo — identical for every converged peer.
    pub fn tempo(&self) -> f64 {
        self.inner.session.tempo()
    }

    /// Move the whole session (local + remote peers) to `tempo_bpm`.
    pub fn set_tempo(&self, tempo_bpm: f64) {
        self.inner.session.set_tempo(tempo_bpm);
    }

    /// This node's session phase in beats.
    pub fn phase(&self) -> f64 {
        self.inner.session.phase()
    }

    /// A session handle on the same bus — e.g. for
    /// [`super::transport::TransportController::with_link_bus`]-style wiring
    /// or tests. A clone is an alias for the same peer (no count change).
    pub fn session(&self) -> LinkSession {
        self.inner.session.clone()
    }

    /// Stop the background thread. Idempotent; `Drop` calls this too.
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.lock().expect("link-net thread lock").take() {
            let _ = handle.join();
        }
    }
}

impl Drop for UdpLinkNode {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(inner: Arc<NodeInner>) {
    // Last (tempo, quantum, birth) broadcast: a diverging snapshot is
    // rebroadcast immediately so edits propagate in milliseconds; the
    // periodic heartbeat covers lost datagrams. Steady heartbeats carry
    // an unchanged birth and are ignored by receivers.
    let mut last_sent = (f64::NAN, f64::NAN, 0u64);
    let mut last_tx = std::time::Instant::now()
        .checked_sub(HEARTBEAT)
        .unwrap_or_else(std::time::Instant::now);
    let mut buf = [0u8; 512];
    loop {
        if inner.stop.load(Ordering::Relaxed) {
            break;
        }
        // The birth travels atomically with the data (stamped at the
        // edit under the session lock), so this broadcast can never miss
        // an edit — even one that lands while we block in `recv_from`.
        let snap = inner.session.snapshot();
        let state = (snap.tempo_bpm, snap.quantum, snap.birth_nanos);
        // NaN tempos never occur (setters/adoption reject them), so
        // tuple equality is a sound change detector here.
        let changed = state != last_sent;
        if changed || last_tx.elapsed() >= HEARTBEAT {
            let pkt = LinkPacket {
                tempo_bpm: snap.tempo_bpm,
                quantum: snap.quantum,
                beats: snap.beats,
                sent_nanos: unix_nanos_now(),
                birth_nanos: snap.birth_nanos,
            };
            let wire = encode(&pkt);
            let own = inner.socket.local_addr().ok();
            let peers = inner.peers.lock().expect("link-net peers lock").clone();
            for peer in &peers {
                // Never loop back to ourselves.
                if Some(*peer) == own {
                    continue;
                }
                let _ = inner.socket.send_to(&wire, peer);
            }
            last_sent = state;
            last_tx = std::time::Instant::now();
        }
        match inner.socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                let Some(pkt) = decode(&buf[..n]) else {
                    continue;
                };
                // Extrapolate the sender's clock by one-way flight time so
                // the anchor lands where the sender is *now*, not when it
                // sent.
                let flight_secs =
                    unix_nanos_now().saturating_sub(pkt.sent_nanos) as f64 / 1e9;
                let flight_secs = flight_secs.clamp(0.0, 5.0);
                let beats_now = pkt.beats + flight_secs * pkt.tempo_bpm / 60.0;
                // Atomic check-and-adopt under one lock: a stale packet
                // racing a newer local edit loses, even if the edit
                // landed after our last snapshot; a corrupt packet is
                // rejected without touching the birth. Either way the
                // session can never be poisoned or clobbered here.
                let _ = inner.session.apply_remote_if_newer(
                    pkt.tempo_bpm,
                    pkt.quantum,
                    beats_now,
                    pkt.birth_nanos,
                );
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[test]
    fn packet_round_trips_exactly() {
        // No sockets: pure codec check, always runs.
        let p = LinkPacket {
            tempo_bpm: 128.0,
            quantum: 4.0,
            beats: 123.456,
            sent_nanos: 9_999,
            birth_nanos: 7_777,
        };
        assert_eq!(decode(&encode(&p)), Some(p));
        // Wrong magic / short datagrams are dropped, never decoded.
        let mut bad = encode(&p);
        bad[0] ^= 0xff;
        assert_eq!(decode(&bad), None);
        assert_eq!(decode(&encode(&p)[..WIRE_LEN - 1]), None);
    }

    #[test]
    fn corrupt_remote_state_never_poisons_session() {
        // No sockets: degenerate payloads are ignored, like local setters
        // — even with a newer birth, which must not leak past them.
        let bus = LinkBus::new(120.0);
        let s = bus.join();
        assert!(!s.apply_remote_if_newer(0.0, 4.0, 10.0, 999));
        assert!(!s.apply_remote_if_newer(f64::NAN, 4.0, 10.0, 999));
        assert!(!s.apply_remote_if_newer(120.0, -1.0, 10.0, 999));
        assert!(!s.apply_remote_if_newer(120.0, 4.0, f64::NAN, 999));
        assert_eq!(s.tempo(), 120.0);
        assert!(s.beats().is_finite());
        assert!(s.phase().is_finite());
        // A stale birth is rejected even with a valid payload, and a
        // newer one is adopted (returns true).
        assert!(!s.apply_remote_if_newer(100.0, 4.0, 10.0, 0));
        assert_eq!(s.tempo(), 120.0);
        assert!(s.apply_remote_if_newer(100.0, 4.0, 10.0, 1_000));
        assert_eq!(s.tempo(), 100.0);
    }

    #[test]
    fn two_udp_sessions_agree_on_loopback() {
        let bus_a = LinkBus::new(120.0);
        let bus_b = LinkBus::new(120.0);
        let node_a = match UdpLinkNode::bind(&bus_a, loopback(0)) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("SKIP link-net loopback: bind refused ({e})");
                return;
            }
        };
        let node_b = match UdpLinkNode::bind(&bus_b, loopback(0)) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("SKIP link-net loopback: bind refused ({e})");
                return;
            }
        };
        let addr_a = node_a.local_addr().expect("bound node has an addr");
        let addr_b = node_b.local_addr().expect("bound node has an addr");
        node_a.add_peer(addr_b);
        node_b.add_peer(addr_a);

        // One peer moves the session; the other process must follow.
        node_a.set_tempo(128.0);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while node_b.tempo() != 128.0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            node_b.tempo(),
            128.0,
            "loopback peer must adopt the session tempo"
        );
        // Same converged clock: phases agree well under a tenth of a beat
        // (loopback flight time is sub-millisecond).
        std::thread::sleep(Duration::from_millis(200));
        let (pa, pb) = (node_a.phase(), node_b.phase());
        assert!(
            (pa - pb).abs() < 0.1,
            "loopback peers must agree on phase ({pa} vs {pb})"
        );
        // Either side can move the session back.
        node_b.set_tempo(100.0);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while node_a.tempo() != 100.0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(node_a.tempo(), 100.0);
        node_a.shutdown();
        node_b.shutdown();
    }
}
