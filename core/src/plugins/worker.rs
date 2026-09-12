//! The child side of the plugin sandbox.
//!
//! Teaching note: "sandboxed out-of-process" means the untrusted code
//! (today a mock gain plugin, tomorrow a real `.clap` via clack-host)
//! runs in a *separate OS process* and talks to the host over a pipe.
//! A segfault then kills the child, not the DAW. The price is one IPC
//! round-trip per call — fine for params and state, and for audio the
//! phase-2 move is shared-memory buffers behind this same process
//! boundary (the boundary stays; only the transport gets faster).
//!
//! Wire protocol: one JSON object per line on stdin, one JSON object per
//! line on stdout (see [`WorkerRequest`] / [`WorkerResponse`]). Newline
//! framing keeps the host parser trivial and debuggable (`echo ... |
//! plugin-worker` works by hand).
//!
//! The worker binary (`core/src/bin/plugin-worker.rs`) is two lines: it
//! calls [`worker_main`] and exits with its code.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};

use super::host::PluginState;

/// Binary name of the worker (resolved next to the test executable).
pub const WORKER_BIN_NAME: &str = "plugin-worker";

/// Host -> worker: one JSON line per request; the worker answers each
/// with exactly one [`WorkerResponse`] line, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WorkerRequest {
    /// Start DSP at this sample rate. First request of every worker.
    Init { sample_rate: f64 },
    SetParam { id: String, value: f64 },
    GetState,
    SetState { state: PluginState },
    /// Render one mono block. `input.len()` is the block length.
    Process { frames: usize, input: Vec<f32> },
    /// Clean exit (code 0). The host also kills ungracefully — workers
    /// must hold no session-critical state, so both are equivalent.
    Shutdown,
}

/// Worker -> host: exactly one line per request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<PluginState>,
}

impl WorkerResponse {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            output: None,
            state: None,
        }
    }

    fn fail(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            output: None,
            state: None,
        }
    }
}

/// Mock DSP: a gain plugin. One param (`gain`, default 1.0) plus an
/// opaque `blob` it never interprets — the blob proves state chunks
/// round-trip through kill + restore byte-exact.
#[derive(Debug)]
pub struct MockDsp {
    sample_rate: f64,
    params: BTreeMap<String, f64>,
    blob: Vec<u8>,
}

impl MockDsp {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            params: BTreeMap::from([("gain".to_string(), 1.0)]),
            blob: Vec::new(),
        }
    }

    /// Rate from the `Init` handshake. Unity-gain DSP ignores it; real
    /// plugin DSP (filters, delays) is built from it.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    fn gain(&self) -> f32 {
        self.params.get("gain").copied().unwrap_or(1.0) as f32
    }

    pub fn set_param(&mut self, id: &str, value: f64) {
        self.params.insert(id.to_string(), value);
    }

    pub fn state(&self) -> PluginState {
        PluginState::new(self.params.clone(), self.blob.clone())
    }

    pub fn set_state(&mut self, state: &PluginState) {
        self.params = state.params.clone();
        self.blob = state.blob.clone();
    }

    /// Pure function of `(input, gain)`: the same math the host test
    /// asserts, so the pipe proves transport, not DSP cleverness.
    pub fn process(&self, input: &[f32]) -> Vec<f32> {
        let g = self.gain();
        input.iter().map(|s| s * g).collect()
    }
}

/// Run the worker loop on stdin/stdout until `Shutdown` or EOF.
/// Returns the process exit code.
pub fn worker_main() -> i32 {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut out = std::io::stdout();
    let mut dsp: Option<MockDsp> = None;

    for line in &mut lines {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: Result<WorkerRequest, _> = serde_json::from_str(&line);
        let mut respond = |resp: &WorkerResponse| {
            let mut s = serde_json::to_string(resp).expect("response serializes");
            s.push('\n');
            if out.write_all(s.as_bytes()).is_err() || out.flush().is_err() {
                std::process::exit(1);
            }
        };
        let req = match req {
            Ok(r) => r,
            Err(e) => {
                respond(&WorkerResponse::fail(format!("bad request: {e}")));
                continue;
            }
        };
        match req {
            WorkerRequest::Init { sample_rate } => {
                dsp = Some(MockDsp::new(sample_rate));
                respond(&WorkerResponse::ok());
            }
            WorkerRequest::SetParam { id, value } => match &mut dsp {
                Some(d) => {
                    d.set_param(&id, value);
                    respond(&WorkerResponse::ok());
                }
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::GetState => match &dsp {
                Some(d) => respond(&WorkerResponse {
                    ok: true,
                    error: None,
                    output: None,
                    state: Some(d.state()),
                }),
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::SetState { state } => match &mut dsp {
                Some(d) => {
                    d.set_state(&state);
                    respond(&WorkerResponse::ok());
                }
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::Process { input, .. } => match &dsp {
                Some(d) => respond(&WorkerResponse {
                    ok: true,
                    error: None,
                    output: Some(d.process(&input)),
                    state: None,
                }),
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::Shutdown => {
                respond(&WorkerResponse::ok());
                return 0;
            }
        }
    }
    0
}

/// Test helper: build the worker binary when missing (tests resolve it
/// next to the test executable). Panics with a clear message when the
/// build fails — a missing worker is a setup error, not a test failure.
#[cfg(test)]
pub(crate) fn ensure_worker_built() {
    use crate::plugins::host::PluginHost;
    let bin = PluginHost::default_worker_bin();
    if bin.exists() {
        return;
    }
    // `cargo test` runs with the workspace root as CWD; a direct
    // `cargo test -p` from elsewhere lands in the package dir instead.
    let manifest = if std::path::Path::new("core/Cargo.toml").exists() {
        "core/Cargo.toml"
    } else {
        "Cargo.toml"
    };
    let status = std::process::Command::new("cargo")
        .args(["build", "--manifest-path", manifest, "--bin", WORKER_BIN_NAME])
        .status()
        .expect("cargo build launches");
    assert!(
        status.success() && bin.exists(),
        "plugin worker must build at {}",
        bin.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_messages_round_trip_through_json() {
        // Newline framing relies on requests serializing to ONE line: no
        // pretty printing, no embedded newlines, ever.
        for req in [
            WorkerRequest::Init { sample_rate: 44100.0 },
            WorkerRequest::SetParam {
                id: "gain".to_string(),
                value: 0.5,
            },
            WorkerRequest::GetState,
            WorkerRequest::Process {
                frames: 2,
                input: vec![1.0, -1.0],
            },
            WorkerRequest::Shutdown,
        ] {
            let line = serde_json::to_string(&req).expect("serialize");
            assert!(!line.contains('\n'));
            let back: WorkerRequest = serde_json::from_str(&line).expect("deserialize");
            assert_eq!(back, req);
        }
    }

    #[test]
    fn mock_dsp_gain_math_and_state() {
        let mut dsp = MockDsp::new(48000.0);
        assert_eq!(dsp.sample_rate(), 48000.0);
        assert_eq!(dsp.process(&[1.0, 0.5]), vec![1.0, 0.5]); // unity default
        dsp.set_param("gain", 0.5);
        assert_eq!(dsp.process(&[1.0, -2.0]), vec![0.5, -1.0]);
        // State carries params AND the opaque blob the DSP never reads.
        let mut st = dsp.state();
        st.blob = vec![9, 8, 7];
        dsp.set_param("gain", 0.0);
        dsp.set_state(&st);
        assert_eq!(dsp.process(&[1.0]), vec![0.5]);
        assert_eq!(dsp.state(), st);
    }
}
