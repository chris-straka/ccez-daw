# Link-style clock sync: crate verdict + design

How the transport follows a shared tempo/phase session, why no external
Link crate was added, and why the frozen IPC surface is untouched.

## 1. Rust Link crate verdict: rejected (both candidates)

Evaluated 2026-09-16 via `cargo search` / `cargo info`:

- `ableton-link 0.1.0` — Rust *bindings* for Ableton's C++ Link library.
  Rejected: pulls a C++ build (cmake + platform audio-clock wiring) into
  `core`, breaks headless/CI builds, and ships with an unknown license.
- `ableton-link-rs 0.1.2` — native Rust implementation of the Link
  protocol. Rejected: `GPL-3.0` (copyleft; wrong for this codebase) and a
  `0.1.x` maturity level for clock-critical code.

Verdict: **no dependency added** (`core/Cargo.toml` untouched). The
session semantics that matter for a DAW transport — one shared tempo every
peer follows, one shared phase clock, join/leave peer counts — are
implemented dependency-free in `core/src/audio/link.rs` (~200 lines, std
only). A real UDP peer-discovery layer (or a re-evaluation of the crates
above once mature/permissively licensed) can plug into `LinkBus` later
without touching the transport.

## 2. What was built

- `core/src/audio/link.rs` — `LinkBus` (the session) + `LinkSession` (one
  peer handle). Tempo is session-wide: any peer's `set_tempo` rebases the
  shared clock anchor so phase stays continuous; degenerate tempos (0,
  NaN, infinite) are ignored and never poison peers. `beats()`/`phase()`
  read a monotonic clock, so two peers on one bus always agree. No sockets
  are ever opened: with no network the bus is a local session whose only
  peer is yourself — that *is* the headless/CI fallback, not a special
  case.
- `core/src/audio/transport.rs` — the controller joins a session at
  construction (`with_link_bus` lets tests share one bus). While
  `set_link_enabled(true)` is on, `tempo()`/`stats().tempo_bpm` and block
  scheduling follow the session tempo, so the timeline and the session
  agree by construction; `TransportStats` additionally reports
  `link_enabled`, `link_peers`, and `link_phase` (phase runs even while
  stopped, like real Link). Enabling carries the local tempo into the
  session and disabling snapshots the session tempo back locally, so
  toggling never jumps. Degenerate `set_tempo` inputs stay ignored.

## 3. Why no new IPC commands (and no `EngineState` change)

`EngineState` (`Stopped`/`Playing`/`Recording`) is frozen: it is emitted
to TypeScript by `core/src/emit.rs` and mirrored in
`contracts/ipc-table.md`, so adding a variant or a command would force a
contract bump and a `typegen` regen for what is really *status*, not a new
verb. Enable/disable + peer count are therefore exposed where the UI
already polls transport health — `TransportStats` (Rust-only, never
codegen'd) plus `TransportController` methods — and `src-tauri` needed no
changes. If a future UI toggle needs an `invoke` verb, that is a contract
decision for its own track.

## 4. Tests

- `audio::link`: two in-process peers agree on tempo/phase (either peer
  can move the session; leaving restores the count); degenerate tempos
  never poison peers; `local()` is a usable solo session.
- `audio::transport`: transport follows a peer-driven session tempo via
  `tempo()` and `stats()` (tempo + phase + peer count), and
  enable→edit→disable carries the tempo without jumps.
- `bun run check` (typegen drift + UI check + `cargo test`) stays green:
  no frozen type or IPC table entry was modified.

## 5. Network half: hand-rolled UDP peer discovery (no crate)

Re-evaluated 2026-09-16 via live `cargo info`:

- `ableton-link 0.1.0` — license: **unknown**; Rust bindings over a C++
  build. Still rejected: unknown license + cmake/C++ in `core` breaks
  headless/CI builds.
- `ableton-link-rs 0.1.2` — license: **GPL-3.0**; native Rust but copyleft
  and `0.1.x` for clock-critical code. Still rejected.

Verdict (confirmed): **no dependency added.** The network half is
hand-rolled std-only UDP in `core/src/audio/link_net.rs` (~380 lines,
`std::net::UdpSocket`, no new dependency), behind the cargo feature
`link-net` (default builds compile no socket code at all).

- Seam: `UdpLinkNode::bind(&bus, addr)` joins any existing `LinkBus`, so a
  transport (`with_link_bus`) and a network node share one session with no
  transport changes. Crate-internal hooks on `LinkSession`
  (`snapshot`/`apply_remote`) move state across the seam; the public
  transport API is untouched.
- Protocol (44 bytes, big-endian): magic `"LNK1"` u32, tempo f64,
  quantum f64, beats f64, send-time unix-nanos u64, *birth* unix-nanos u64.
  The birth is stamped at the edit under the session lock and inherited on
  adoption (never refreshed by forwarding); receivers adopt a packet only
  when its birth is strictly newer than the local state's, so each edit
  lands exactly once and steady heartbeats never jitter the phase.
  Receivers extrapolate beats by one-way flight time and rebase the anchor;
  corrupt/short/wrong-magic datagrams and degenerate payloads are dropped
  before they can touch the birth, and can never poison the session.
- Race safety: the birth check and the adoption hold one session lock
  (`LinkSession::apply_remote_if_newer`), so a stale packet racing a newer
  local edit loses even if the edit landed after the network thread's last
  snapshot — polling the clock can never miss an edit, because the stamp
  travels atomically with the data. (An earlier per-peer-sequence design
  flapped on exactly this race; birth-time ordering fixed it.)
- Proof: `audio::link_net::tests::two_udp_sessions_agree_on_loopback` —
  two nodes on 127.0.0.1 (ephemeral ports), one sets 128 BPM, the other
  adopts it, phases agree to < 0.1 beat, then the second moves the session
  to 100 BPM and the first follows. Sandbox-safe: a refused bind prints
  `SKIP` and passes; codec/poison tests use no sockets and always run.
- Limits: phase converges subject to wall-clock skew between machines
  (tempo-exact, phase-best-effort cross-machine; < 0.1 beat on loopback).
  Full Link-style ping-pong clock sync is future work and needs no wire
  change in shape.

## 6. Upstream LinkKit license vs this repo: fallback taken (2026-09-16)

Upstream `Ableton/link` (`LICENSE.md`, fetched live): GPL-2.0-or-later —
free for GPL-compatible use, but "to incorporate Link into a proprietary
software application, please contact <link-devs@ableton.com>" (commercial
license required). Both Rust crates inherit the problem: `ableton-link
0.1.0` wraps that GPL C++ tree (license field: unknown, plus a cmake/C++
build inside `core`), and `ableton-link-rs 0.1.2` is natively GPL-3.0.

This repo carries no `LICENSE` file, no `license` field in
`core/Cargo.toml` / `package.json` (`"private": true`), and ships a
closed desktop product — accepting a GPL copyleft dependency (or an
unknown-license C++ binding needing a paid commercial grant) is blocked
without a deliberate relicensing decision by the owner. A genuine
Ableton Link session (two *real* Link peers converging) therefore cannot
be wired in on this track.

Fallback implemented instead, stated explicitly: the dependency-free
`LinkBus`/`LinkSession` seam keeps its API shape, `UdpLinkNode` (std-only
UDP, feature `link-net`) provides the cross-process session underneath,
and `two_udp_sessions_agree_on_loopback` is the loopback proof (two
peers agree on tempo 128 BPM → 100 BPM and phase < 0.1 beat). Re-evaluate
only on a permissively-licensed, headless-safe Link crate — or after the
owner clears Link commercial licensing.
