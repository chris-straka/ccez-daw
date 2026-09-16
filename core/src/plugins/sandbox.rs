//! Out-of-process plugin sandbox: spawn, supervise, and resurrect.
//!
//! Teaching note: realtime audio has a hard deadline, but plugin
//! *robustness* is a different problem — a bad plugin pointer kills the
//! whole address space it shares. The fix is an old OS idea: put the
//! untrusted code in its own process. [`SandboxedPlugin`] owns one child
//! (the [`worker`](crate::plugins::worker) binary) plus pipes to it:
//!
//! - Every call is one JSON line down, one JSON line back
//!   ([`WorkerRequest`](crate::plugins::worker::WorkerRequest) /
//!   [`WorkerResponse`](crate::plugins::worker::WorkerResponse)).
//! - The host detects death four ways: the write fails (broken pipe),
//!   the read hits EOF, [`alive`](SandboxedPlugin::alive) sees the child
//!   reaped, or the watchdog deadline fires on a mute child. All four
//!   surface as [`SandboxError::Crashed`] — and the last known-good
//!   [`PluginState`](crate::plugins::host::PluginState) is already
//!   sitting in the host, untouched by the crash.
//! - [`recover`](SandboxedPlugin::recover) respawns the child, replays
//!   `Init` + `SetState(last_good)`, and hands audio back. The session
//!   never went down; only that insert dropped samples while dead.
//!
//! Hang note: every round-trip carries a watchdog deadline
//! ([`DEFAULT_WORKER_TIMEOUT`], tunable per plugin via
//! [`set_timeout`](SandboxedPlugin::set_timeout)). A worker that is
//! *alive but mute* past the deadline is killed and reports `Crashed`,
//! exactly like the segfault case. The
//! [`ping`](SandboxedPlugin::ping) heartbeat is the no-side-effect poll
//! to run between renders.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::host::{PluginDescriptor, PluginKind, PluginState};
use super::worker::{WorkerRequest, WorkerResponse};

/// Default watchdog deadline per worker round-trip. Blocks are
/// millisecond-scale, so ten seconds is generous on loaded CI while still
/// turning a mute worker into a detected crash instead of a stuck session.
pub const DEFAULT_WORKER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    /// The worker is dead (segfault, kill, clean exit — from the host's
    /// side they are identical: no process, no pipe). Carries context.
    Crashed(String),
    /// The worker is up but answered nonsense (protocol bug, not crash).
    Protocol(String),
    Io(String),
    /// `recover()` with no snapshot to restore (nothing ever succeeded).
    NoSnapshot,
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Crashed(m) => write!(f, "plugin worker crashed: {m}"),
            Self::Protocol(m) => write!(f, "plugin protocol error: {m}"),
            Self::Io(m) => write!(f, "plugin io: {m}"),
            Self::NoSnapshot => write!(f, "no plugin snapshot to recover from"),
        }
    }
}

impl std::error::Error for SandboxError {}

pub type Result<T> = std::result::Result<T, SandboxError>;

/// One plugin's private worker process + its host-side truth.
///
/// The invariant: `last_good` always holds the state from the most recent
/// *successful* mutating call (`spawn`'s baseline, `set_param`,
/// `snapshot`, `restore`). A crash can therefore only lose calls that
/// never completed — and `recover()` replays exactly `last_good`.
pub struct SandboxedPlugin {
    descriptor: PluginDescriptor,
    worker_bin: PathBuf,
    sample_rate: f64,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    last_good: Option<PluginState>,
    timeout: Duration,
}

impl SandboxedPlugin {
    /// Spawn the worker and run the `Init` handshake. Takes a baseline
    /// snapshot so `recover()` works even before the first edit.
    pub fn spawn(
        worker_bin: &Path,
        descriptor: &PluginDescriptor,
        sample_rate: f64,
    ) -> Result<Self> {
        let mut this = Self {
            descriptor: descriptor.clone(),
            worker_bin: worker_bin.to_path_buf(),
            sample_rate,
            child: None,
            stdin: None,
            stdout: None,
            last_good: None,
            timeout: DEFAULT_WORKER_TIMEOUT,
        };
        this.respawn()?;
        let state = this.snapshot()?;
        this.last_good = Some(state);
        Ok(this)
    }

    /// Start (or restart) the child and handshake. Does NOT restore state
    /// — callers layer that on (`spawn` snapshots fresh; `recover`
    /// replays `last_good`).
    fn respawn(&mut self) -> Result<()> {
        let mut child = Command::new(&self.worker_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| SandboxError::Io(format!("spawn worker: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SandboxError::Io("worker stdin unavailable".to_string()))?;
        let stdout: ChildStdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Io("worker stdout unavailable".to_string()))?;
        self.child = Some(child);
        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout));
        self.call(&WorkerRequest::Init {
            sample_rate: self.sample_rate,
        })?;
        // Real bundles load inside the worker: the `Init` handshake is
        // identical for every backend, then a Clap/Vst3/Au/Wasm descriptor
        // adds one `LoadClap` / `LoadVst3` / `LoadAu` / `LoadWasm`
        // round-trip. Recovery replays the same two steps, so host and
        // recovery code never branch on plugin kind.
        if self.descriptor.kind == PluginKind::Clap {
            let path = self.descriptor.path.clone().ok_or_else(|| {
                SandboxError::Protocol("clap descriptor has no bundle path".to_string())
            })?;
            self.call(&WorkerRequest::LoadClap {
                path,
                plugin_id: self.descriptor.plugin_uid.clone(),
            })?;
        }
        if self.descriptor.kind == PluginKind::Vst3 {
            let path = self.descriptor.path.clone().ok_or_else(|| {
                SandboxError::Protocol("vst3 descriptor has no bundle path".to_string())
            })?;
            self.call(&WorkerRequest::LoadVst3 {
                path,
                class_id: self.descriptor.plugin_uid.clone(),
            })?;
        }
        if self.descriptor.kind == PluginKind::Au {
            let desc = self.descriptor.au_desc.clone().ok_or_else(|| {
                SandboxError::Protocol("au descriptor has no component codes".to_string())
            })?;
            self.call(&WorkerRequest::LoadAu {
                component_type: desc.component_type,
                component_subtype: desc.component_subtype,
                manufacturer: desc.manufacturer,
            })?;
        }
        if self.descriptor.kind == PluginKind::Wasm {
            let path = self.descriptor.path.clone().ok_or_else(|| {
                SandboxError::Protocol("wasm descriptor has no module path".to_string())
            })?;
            #[cfg(feature = "wasm-runtime")]
            self.call(&WorkerRequest::LoadWasm { path })?;
            // Without the feature there is no wasmtime to hand the module
            // to: refuse before spawning anything half-loaded, the same
            // clean error the worker itself would answer.
            #[cfg(not(feature = "wasm-runtime"))]
            {
                let _ = path;
                return Err(SandboxError::Protocol(
                    "wasm devices need the wasm-runtime feature".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Watchdog deadline for one round-trip. Tests shorten it to prove
    /// hang detection without waiting out the production default.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Heartbeat: ask the worker for an immediate no-op reply. A healthy
    /// worker answers well inside [`Self::timeout`]; a hung one trips the
    /// watchdog and reports `Crashed`, exactly like a segfault — and
    /// [`recover`](SandboxedPlugin::recover) brings it back the same way.
    pub fn ping(&mut self) -> Result<()> {
        self.call(&WorkerRequest::Ping)?;
        Ok(())
    }

    /// Test hook: the live worker's pid (a suspended pid is the
    /// alive-but-mute stand-in; see the hang-detection test).
    #[cfg(test)]
    pub(crate) fn child_pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }

    /// One synchronous round-trip. Any transport failure is inspected:
    /// a dead child becomes `Crashed` (the case this module owns); a live
    /// child answering garbage becomes `Protocol`; a live child answering
    /// nothing before the watchdog deadline becomes `Crashed` too (the
    /// hang case this module now owns as well).
    fn call(&mut self, req: &WorkerRequest) -> Result<WorkerResponse> {
        let mut line = serde_json::to_string(req)
            .map_err(|e| SandboxError::Protocol(format!("request serializes: {e}")))?;
        line.push('\n');
        let written: std::result::Result<(), String> = match self.stdin.as_mut() {
            Some(s) => s
                .write_all(line.as_bytes())
                .and(s.flush())
                .map_err(|e| e.to_string()),
            None => Err("no worker stdin".to_string()),
        };
        if let Err(e) = written {
            return Err(self.death_of(format!("write failed: {e}")));
        }
        // The read moves to a scoped thread so the watchdog can fire:
        // pipes have no portable read deadline, but `recv_timeout` does.
        // The reader is moved back on success; on timeout the thread is
        // left to drain EOF after the kill below and then exit.
        let reader = match self.stdout.take() {
            Some(s) => s,
            None => return Err(self.death_of("no worker stdout".to_string())),
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut answer = String::new();
            let read = reader.read_line(&mut answer).map(Some).map_err(|e| e.to_string());
            let _ = tx.send((reader, read, answer));
        });
        let timeout = self.timeout;
        let (reader, read, answer) = match rx.recv_timeout(timeout) {
            Ok(triple) => triple,
            Err(_) => {
                return Err(self.timeout_of(format!(
                    "worker timed out after {}ms",
                    timeout.as_millis()
                )));
            }
        };
        self.stdout = Some(reader);
        match read {
            Ok(Some(0)) | Ok(None) => Err(self.death_of("EOF on worker stdout".to_string())),
            Ok(Some(_)) => {
                let resp: WorkerResponse =
                    serde_json::from_str(answer.trim()).map_err(|e| {
                        SandboxError::Protocol(format!(
                            "bad worker reply `{}`: {e}",
                            answer.trim()
                        ))
                    })?;
                if resp.ok {
                    Ok(resp)
                } else {
                    Err(SandboxError::Protocol(
                        resp.error.unwrap_or_else(|| "worker refused".to_string()),
                    ))
                }
            }
            Err(e) => Err(self.death_of(format!("read failed: {e}"))),
        }
    }

    /// Reap-check the child and convert *this* failure into `Crashed`.
    /// Never panics on an already-dead child: double-kills are routine
    /// (kill a crashed plugin, then recover it). Kills first so a
    /// *stopped* (suspended, alive-but-mute) child cannot block `wait`.
    fn death_of(&mut self, context: String) -> SandboxError {
        let exit = self
            .child
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
            .map(|s| s.to_string());
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
        self.stdin = None;
        self.stdout = None;
        SandboxError::Crashed(match exit {
            Some(s) => format!("{context} (exited: {s})"),
            None => context,
        })
    }

    /// The watchdog path: the child is alive (or unprovably dead) but
    /// mute, so kill it outright and report `Crashed` — `recover()` then
    /// respawns and replays `last_good` exactly like the segfault case.
    fn timeout_of(&mut self, context: String) -> SandboxError {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
        // `stdout` already moved into the timed-out reader thread, which
        // drains EOF from the killed pipe and exits on its own.
        self.stdout = None;
        SandboxError::Crashed(context)
    }

    /// `true` while the child is unreaped. Kills nothing; reaps an exited
    /// child so it never lingers as a zombie.
    pub fn alive(&mut self) -> bool {
        match self.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            Some(Ok(Some(_))) => {
                let _ = self.child.as_mut().map(|c| c.wait());
                false
            }
            _ => false,
        }
    }

    pub fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    /// Render one mono block through the worker.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        let resp = self.call(&WorkerRequest::Process {
            frames: input.len(),
            input: input.to_vec(),
        })?;
        resp.output
            .ok_or_else(|| SandboxError::Protocol("worker omitted output".to_string()))
    }

    /// Set one param. On success the host-side snapshot advances — this
    /// is what makes a later crash lose nothing committed.
    pub fn set_param(&mut self, id: &str, value: f64) -> Result<()> {
        self.call(&WorkerRequest::SetParam {
            id: id.to_string(),
            value,
        })?;
        // Refresh the cached truth: cheap (one extra round-trip) and it
        // keeps `last_good` exact even for params the worker clamps.
        let state = self.snapshot()?;
        self.last_good = Some(state);
        Ok(())
    }

    /// Query the worker's processing latency in samples at the current
    /// rate (feeds [`LatencyMap`](super::latency::LatencyMap); 0 = the
    /// mock's honest answer for memory-free gain). Pure probe: it neither
    /// reads nor writes `last_good`.
    pub fn latency_samples(&mut self) -> Result<u32> {
        let resp = self.call(&WorkerRequest::GetLatency)?;
        resp.latency_samples
            .ok_or_else(|| SandboxError::Protocol("worker omitted latency".to_string()))
    }

    /// Pull the worker's current state; caches it as `last_good`.
    pub fn snapshot(&mut self) -> Result<PluginState> {
        let resp = self.call(&WorkerRequest::GetState)?;
        let state = resp
            .state
            .ok_or_else(|| SandboxError::Protocol("worker omitted state".to_string()))?;
        self.last_good = Some(state.clone());
        Ok(state)
    }

    /// Push `state` into the worker (respawning first when dead) and
    /// adopt it as `last_good`.
    pub fn restore(&mut self, state: &PluginState) -> Result<()> {
        if !self.alive() {
            self.respawn()?;
        }
        self.call(&WorkerRequest::SetState { state: state.clone() })?;
        self.last_good = Some(state.clone());
        Ok(())
    }

    /// Kill the worker without ceremony — the test stand-in for a
    /// segfault. Idempotent: killing the dead is a no-op.
    pub fn kill(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
        self.stdout = None;
        Ok(())
    }

    /// Respawn a dead worker and replay `last_good`. A live worker just
    /// re-snapshots — recovery is idempotent and never destructive.
    /// Returns the state the resurrected worker now holds.
    pub fn recover(&mut self) -> Result<PluginState> {
        if self.alive() {
            return self.snapshot();
        }
        let state = self.last_good.clone().ok_or(SandboxError::NoSnapshot)?;
        self.respawn()?;
        self.call(&WorkerRequest::SetState { state: state.clone() })?;
        self.last_good = Some(state.clone());
        Ok(state)
    }
}

impl Drop for SandboxedPlugin {
    fn drop(&mut self) {
        // Best effort: ask politely only when the pipes are usable, then
        // always reap. A Drop must never block on a dead worker.
        if self.child.is_some() {
            if let Ok(mut line) = serde_json::to_string(&WorkerRequest::Shutdown) {
                line.push('\n');
                let _ = self
                    .stdin
                    .as_mut()
                    .map(|s| s.write_all(line.as_bytes()).and(s.flush()));
            }
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::host::PluginHost;
    use crate::plugins::worker::ensure_worker_built;

    fn sandbox() -> SandboxedPlugin {
        ensure_worker_built();
        SandboxedPlugin::spawn(
            &PluginHost::default_worker_bin(),
            &PluginDescriptor::mock("s", "Sandbox"),
            44100.0,
        )
        .expect("spawn")
    }

    #[test]
    fn spawn_snapshot_process_round_trip() {
        let mut p = sandbox();
        assert!(p.alive());
        p.set_param("gain", 2.0).expect("set_param");
        assert_eq!(p.process(&[0.5, -0.5]).expect("process"), vec![1.0, -1.0]);
        let snap = p.snapshot().expect("snapshot");
        assert_eq!(snap.params.get("gain"), Some(&2.0));
    }

    #[test]
    fn dead_worker_reports_crashed_and_recovers_exact() {
        let mut p = sandbox();
        p.set_param("gain", 0.5).expect("gain");
        // Opaque blob rides along: restore must be byte-exact, not just params.
        let mut pre = p.snapshot().expect("snapshot");
        pre.blob = vec![7, 7, 7];
        p.restore(&pre).expect("restore blob");
        assert_eq!(p.process(&[1.0]).expect("process"), vec![0.5]);

        // The crash: ungraceful kill, no handshake.
        p.kill().expect("kill");
        assert!(!p.alive());
        let err = p.process(&[1.0]).expect_err("dead worker must fail");
        assert!(matches!(err, SandboxError::Crashed(_)), "got {err:?}");

        // Recovery replays the last known-good state, blob included.
        let back = p.recover().expect("recover");
        assert_eq!(back, pre);
        assert!(p.alive());
        assert_eq!(p.process(&[1.0]).expect("renders"), vec![0.5]);
        assert_eq!(p.snapshot().expect("state"), pre);
    }

    #[test]
    fn failed_load_wasm_leaves_mock_running() {
        // The `LoadWasm` failure contract, shared with Clap/Vst3/Au: a
        // refused load reports `Protocol` and the previous backend keeps
        // rendering. This holds on every build — without `wasm-runtime`
        // the worker refuses for the missing feature, with it for the
        // missing file — so the mock below survives either way.
        let mut p = sandbox();
        let err = p
            .call(&WorkerRequest::LoadWasm {
                path: "/nonexistent/ghost.wasm".to_string(),
            })
            .expect_err("bad module must fail");
        assert!(matches!(err, SandboxError::Protocol(_)), "got {err:?}");
        assert!(p.alive());
        assert_eq!(p.process(&[1.0]).expect("mock survives"), vec![1.0]);
    }

    #[test]
    fn latency_probe_reports_zero_for_mock_gain() {
        let mut p = sandbox();
        // Memory-free gain is honestly zero-latency; the probe must not
        // disturb rendering or the recovery snapshot.
        assert_eq!(p.latency_samples().expect("latency"), 0);
        assert_eq!(p.process(&[1.0]).expect("process"), vec![1.0]);
        p.kill().expect("kill");
        let err = p.latency_samples().expect_err("dead probe must fail");
        assert!(matches!(err, SandboxError::Crashed(_)), "got {err:?}");
        p.recover().expect("recover");
        assert_eq!(p.latency_samples().expect("latency again"), 0);
    }

    #[test]
    fn recover_is_idempotent_on_a_live_worker() {
        let mut p = sandbox();
        p.set_param("gain", 3.0).expect("gain");
        let a = p.recover().expect("recover live");
        assert_eq!(a.params.get("gain"), Some(&3.0));
        assert!(p.alive());
    }

    #[test]
    fn heartbeat_answers_on_a_live_worker() {
        let mut p = sandbox();
        p.ping().expect("ping");
        assert!(p.alive());
        // The probe disturbs neither rendering nor the recovery snapshot.
        assert_eq!(p.process(&[1.0]).expect("process"), vec![1.0]);
        p.set_param("gain", 2.0).expect("gain");
        p.ping().expect("ping again");
        assert_eq!(p.process(&[1.0]).expect("renders"), vec![2.0]);
    }

    #[cfg(unix)]
    #[test]
    fn muted_worker_trips_the_watchdog_and_recovers() {
        use std::time::{Duration, Instant};
        let mut p = sandbox();
        p.set_timeout(Duration::from_millis(300));
        p.ping().expect("healthy ping");
        p.set_param("gain", 0.5).expect("gain");
        let pid = p.child_pid().expect("worker pid");
        // Suspend the child: still unreaped (`alive`) but answering
        // nothing — the alive-but-mute stand-in for a deadlocked plugin.
        let status = std::process::Command::new("kill")
            .args(["-STOP", &pid.to_string()])
            .status()
            .expect("kill -STOP launches");
        assert!(status.success());
        assert!(p.alive(), "a stopped child is still unreaped");
        // The watchdog fires near the deadline instead of blocking forever.
        let started = Instant::now();
        let err = p.process(&[1.0]).expect_err("mute worker must fail");
        assert!(
            matches!(err, SandboxError::Crashed(_)),
            "mute must read as Crashed, got {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "watchdog must fire, not block"
        );
        assert!(!p.alive());
        // Recovery respawns a fresh worker and replays last-known-good gain.
        let back = p.recover().expect("recover");
        assert_eq!(back.params.get("gain"), Some(&0.5));
        assert!(p.alive());
        p.ping().expect("ping after recover");
        assert_eq!(p.process(&[1.0]).expect("renders"), vec![0.5]);
    }
}
