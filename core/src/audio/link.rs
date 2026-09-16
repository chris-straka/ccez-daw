//! Link-style session clock: shared tempo/phase consensus without hardware.
//!
//! Teaching note: Ableton Link is "the band agrees on a tempo without a
//! conductor". Every peer publishes its tempo to a shared session and reads
//! the session tempo back; beats advance from one shared clock, so two
//! peers that joined the same session always agree on tempo *and* on where
//! beat 1 lands (phase). There is no leader and no network master clock —
//! just a session everyone follows when link is enabled, and a local clock
//! when it is not.
//!
//! [`LinkBus`] is that session. [`LinkBus::join`] adds one peer;
//! [`LinkSession::set_tempo`] moves the whole session;
//! [`LinkSession::beats`] / [`LinkSession::phase`] read the shared clock.
//! By default no sockets are ever opened, so this is headless/CI-safe by
//! construction: with no network the bus is simply a local session with
//! yourself as the only peer. The optional network half lives in
//! [`super::link_net`] (cargo feature `link-net`): a std-only UDP
//! peer-discovery layer that syncs tempo/phase across processes on
//! 127.0.0.1 or a LAN. See `docs/notes/link-sync.md` for the crate
//! verdict (why no external Link crate) and the wire protocol.
//!
//! Reads no frozen types and adds no IPC or project-schema surface, so the
//! typegen drift gate is unaffected.

use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Beats per session cycle used for [`LinkSession::phase`]. Link's default
/// quantum: bar-length phase in 4/4.
pub const DEFAULT_QUANTUM: f64 = 4.0;
/// Session tempo clamp, matching the transport's local range.
pub const MIN_TEMPO_BPM: f64 = 1.0;
pub const MAX_TEMPO_BPM: f64 = 960.0;

fn clamp_tempo(bpm: f64) -> Option<f64> {
    if bpm.is_finite() && bpm > 0.0 {
        Some(bpm.clamp(MIN_TEMPO_BPM, MAX_TEMPO_BPM))
    } else {
        None
    }
}

/// Wall-clock time in unix nanos (for the `link-net` state birth stamp).
#[cfg(feature = "link-net")]
pub(crate) fn unix_nanos_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[derive(Debug)]
struct Shared {
    tempo_bpm: f64,
    quantum: f64,
    /// Session clock anchor: the session read `anchor_beats` at `anchor_at`,
    /// and advances at `tempo_bpm` from there. Every tempo (or quantum)
    /// change rebases the anchor to "now", so the clock never jumps.
    anchor_beats: f64,
    anchor_at: Instant,
    peers: usize,
    /// Birth time (unix nanos) of the current tempo/quantum state: stamped
    /// at each local edit, inherited on network adoption. Lets the
    /// `link-net` layer order states across processes — co-located with
    /// the data under the same lock, so an edit can never be overwritten
    /// by a stale adoption before the network thread observes it. Unused
    /// (always zero) without the feature.
    #[cfg(feature = "link-net")]
    birth_nanos: u64,
}

impl Shared {
    fn beats_locked(&self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.anchor_at).as_secs_f64();
        // Elapsed time is monotonic, tempo is positive: always finite.
        self.anchor_beats + elapsed * self.tempo_bpm / 60.0
    }

    fn rebase_locked(&mut self, now: Instant) {
        self.anchor_beats = self.beats_locked(now);
        self.anchor_at = now;
    }
}

/// One Link-style session: the shared tempo/phase bus. Cloneable aliases
/// share the same session (no new peer); [`LinkBus::join`] adds a peer.
#[derive(Debug, Clone)]
pub struct LinkBus {
    shared: Arc<Mutex<Shared>>,
}

impl LinkBus {
    /// A fresh session at `tempo_bpm` with no peers yet. Invalid tempos
    /// fall back to 120 BPM rather than poisoning the clock.
    pub fn new(tempo_bpm: f64) -> Self {
        let tempo = clamp_tempo(tempo_bpm).unwrap_or(120.0);
        Self {
            shared: Arc::new(Mutex::new(Shared {
                tempo_bpm: tempo,
                quantum: DEFAULT_QUANTUM,
                anchor_beats: 0.0,
                anchor_at: Instant::now(),
                peers: 0,
                #[cfg(feature = "link-net")]
                birth_nanos: 0,
            })),
        }
    }

    /// Join the session as a new peer. The newcomer adopts the session
    /// tempo (Link join semantics) — it does not reset the clock.
    pub fn join(&self) -> LinkSession {
        self.shared.lock().expect("link bus lock").peers += 1;
        LinkSession { bus: self.clone() }
    }

    /// Current peer count (joined sessions not yet dropped).
    pub fn num_peers(&self) -> usize {
        self.shared.lock().expect("link bus lock").peers
    }
}

/// One peer's handle on a [`LinkBus`]. Dropping leaves the session.
/// `Clone` is an alias for the *same* peer (no count change); only
/// [`LinkBus::join`] adds peers.
#[derive(Debug, Clone)]
pub struct LinkSession {
    bus: LinkBus,
}

impl LinkSession {
    /// A solo session for one peer: the headless/CI fallback (no network
    /// attempted, local monotonic clock, one peer: yourself).
    pub fn local() -> Self {
        LinkBus::new(120.0).join()
    }

    /// Join one more peer onto this session (same bus, peer count + 1).
    pub fn join_peer(&self) -> LinkSession {
        self.bus.join()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.bus.shared.lock().expect("link bus lock")
    }

    /// Session tempo in BPM — identical for every peer on the bus.
    pub fn tempo(&self) -> f64 {
        self.lock().tempo_bpm
    }

    /// Move the whole session to `tempo_bpm`: every peer follows from the
    /// next read. Rebases the clock anchor so phase stays continuous.
    /// Non-finite / non-positive values are ignored (never poison peers).
    pub fn set_tempo(&self, tempo_bpm: f64) {
        if let Some(t) = clamp_tempo(tempo_bpm) {
            let now = Instant::now();
            let mut s = self.lock();
            s.rebase_locked(now);
            s.tempo_bpm = t;
            #[cfg(feature = "link-net")]
            {
                s.birth_nanos = unix_nanos_now();
            }
        }
    }

    /// Session beat position on the shared clock.
    pub fn beats(&self) -> f64 {
        let now = Instant::now();
        self.lock().beats_locked(now)
    }

    /// Position inside the session cycle (`quantum` beats): identical for
    /// every peer that reads at the same instant. Always in `[0, quantum)`.
    pub fn phase(&self) -> f64 {
        let q = self.quantum();
        if q <= 0.0 {
            return 0.0;
        }
        self.beats().rem_euclid(q)
    }

    pub fn quantum(&self) -> f64 {
        self.lock().quantum
    }

    /// Set the cycle length in beats. Rebases the anchor so phase stays
    /// continuous; non-finite / non-positive values are ignored.
    pub fn set_quantum(&self, quantum: f64) {
        if quantum.is_finite() && quantum > 0.0 {
            let now = Instant::now();
            let mut s = self.lock();
            s.rebase_locked(now);
            s.quantum = quantum;
            #[cfg(feature = "link-net")]
            {
                s.birth_nanos = unix_nanos_now();
            }
        }
    }

    /// Peers currently on this session (including self).
    pub fn num_peers(&self) -> usize {
        self.bus.num_peers()
    }

    /// Snapshot of the session clock for the UDP discovery layer
    /// ([`super::link_net`], cargo feature `link-net`). `Clone`d handles
    /// share the bus, so the snapshot is identical for every alias. The
    /// birth travels with the data under one lock, so the network thread
    /// can never observe a half-applied state.
    #[cfg(feature = "link-net")]
    pub(crate) fn snapshot(&self) -> LinkSnapshot {
        let now = Instant::now();
        let s = self.lock();
        LinkSnapshot {
            tempo_bpm: s.tempo_bpm,
            quantum: s.quantum,
            beats: s.beats_locked(now),
            birth_nanos: s.birth_nanos,
        }
    }

    /// Adopt a remote peer's state iff its `birth_nanos` is strictly newer
    /// than the local state's: tempo/quantum are taken over, the clock
    /// anchor is rebased to `remote_beats` ("now") so phase converges to
    /// the sender, and the birth is inherited (never refreshed by
    /// forwarding). The check and the apply hold one lock, so a local
    /// edit racing an adoption keeps whichever state is newer — a stale
    /// packet can never clobber a newer local edit, even one the network
    /// thread has not observed yet. Degenerate values are ignored, like
    /// the local setters — a corrupt packet never poisons the session
    /// (and never advances the birth past genuine states). Returns true
    /// when the state was adopted.
    #[cfg(feature = "link-net")]
    pub(crate) fn apply_remote_if_newer(
        &self,
        tempo_bpm: f64,
        quantum: f64,
        remote_beats: f64,
        birth_nanos: u64,
    ) -> bool {
        if clamp_tempo(tempo_bpm).is_none() {
            return false;
        }
        if !(quantum.is_finite() && quantum > 0.0) {
            return false;
        }
        if !remote_beats.is_finite() {
            return false;
        }
        let now = Instant::now();
        let mut s = self.lock();
        if birth_nanos <= s.birth_nanos {
            return false;
        }
        s.tempo_bpm = tempo_bpm.clamp(MIN_TEMPO_BPM, MAX_TEMPO_BPM);
        s.quantum = quantum;
        s.anchor_beats = remote_beats;
        s.anchor_at = now;
        s.birth_nanos = birth_nanos;
        true
    }
}

/// Session clock reading for the UDP discovery layer.
#[cfg(feature = "link-net")]
#[derive(Debug, Clone, Copy)]
pub(crate) struct LinkSnapshot {
    pub tempo_bpm: f64,
    pub quantum: f64,
    pub beats: f64,
    pub birth_nanos: u64,
}

impl Drop for LinkSession {
    fn drop(&mut self) {
        // Leaving is infallible: a poisoned lock still lets us decrement.
        if let Ok(mut s) = self.bus.shared.lock() {
            s.peers = s.peers.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_peers_agree_on_tempo_and_phase() {
        let bus = LinkBus::new(120.0);
        let a = bus.join();
        let b = bus.join();
        assert_eq!(a.num_peers(), 2);
        assert_eq!(b.num_peers(), 2);
        // One peer moves the session; the other follows.
        a.set_tempo(128.0);
        assert_eq!(b.tempo(), 128.0);
        assert_eq!(a.tempo(), 128.0);
        // Same clock, same instant: phases agree.
        let pa = a.phase();
        let pb = b.phase();
        assert!(
            (pa - pb).abs() < 0.05,
            "peers must agree on phase ({pa} vs {pb})"
        );
        // Either peer can move the session back.
        b.set_tempo(100.0);
        assert_eq!(a.tempo(), 100.0);
        // Leaving restores the count.
        drop(b);
        assert_eq!(a.num_peers(), 1);
    }

    #[test]
    fn degenerate_tempo_never_poisons_peers() {
        let bus = LinkBus::new(120.0);
        let a = bus.join();
        let b = bus.join();
        a.set_tempo(0.0);
        a.set_tempo(f64::NAN);
        a.set_tempo(f64::INFINITY);
        assert_eq!(b.tempo(), 120.0);
        assert!(b.beats().is_finite());
        assert!(b.phase().is_finite());
    }

    #[test]
    fn local_fallback_is_a_solo_session() {
        let s = LinkSession::local();
        assert_eq!(s.num_peers(), 1);
        assert_eq!(s.tempo(), 120.0);
        s.set_tempo(90.0);
        assert_eq!(s.tempo(), 90.0);
    }
}
