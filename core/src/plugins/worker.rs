//! The child side of the plugin sandbox.
//!
//! Teaching note: "sandboxed out-of-process" means the untrusted code
//! (a mock gain plugin, a real `.clap` via `LoadClap` + `ClapBackend`,
//! or a `.wasm` module via `LoadWasm` + `WasmBackend`)
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

use super::au::{AuBackend, AuComponentDesc};
use super::clap::ClapBackend;
use super::host::PluginState;
use super::vst3::Vst3Backend;
#[cfg(feature = "wasm-runtime")]
use super::wasm::WasmBackend;

/// Binary name of the worker (resolved next to the test executable).
pub const WORKER_BIN_NAME: &str = "plugin-worker";

/// Host -> worker: one JSON line per request; the worker answers each
/// with exactly one [`WorkerResponse`] line, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WorkerRequest {
    /// Start DSP at this sample rate. First request of every worker.
    Init { sample_rate: f64 },
    /// Replace the mock backend with a real `.clap` bundle loaded through
    /// [`ClapBackend`] (activated at the `Init` rate). `plugin_id` selects
    /// one of [`ClapBackend::available_plugins`] (multi-plugin bundles);
    /// `None` keeps the old default (first plugin in the entry). Requires
    /// `Init` first; a failure leaves the mock running so the host can
    /// report the error and tear the worker down cleanly.
    LoadClap {
        path: String,
        #[serde(default)]
        plugin_id: Option<String>,
    },
    /// Replace the mock backend with a real `*.vst3` bundle loaded through
    /// [`Vst3Backend`] (mono buses at the `Init` rate). `class_id` selects
    /// one of [`Vst3Backend::available_classes`]; `None` keeps the old
    /// default (first audio-effect class). Same contract as `LoadClap`:
    /// requires `Init` first; a failure leaves the previous backend running
    /// so the host can report the error and tear the worker down cleanly.
    LoadVst3 {
        path: String,
        #[serde(default)]
        class_id: Option<String>,
    },
    /// Replace the mock backend with a real Audio Unit driven through
    /// [`AuBackend`] (mono `Float32` at the `Init` rate; effects pull
    /// input through the render callback, sources render without one).
    /// macOS only — elsewhere the load fails cleanly with
    /// `UnsupportedPlatform`. Same contract as `LoadClap`: requires
    /// `Init` first; a failure leaves the previous backend running.
    /// Untested component types refuse with `UnsupportedType` before
    /// anything loads.
    LoadAu {
        component_type: u32,
        component_subtype: u32,
        manufacturer: u32,
    },
    /// Replace the mock backend with a `.wasm` device module loaded
    /// through [`WasmBackend`](super::wasm::WasmBackend) (mono `f32` at
    /// the `Init` rate; params cross as `SetParam`, state as
    /// `GetState`/`SetState`). Same contract as `LoadClap`: requires
    /// `Init` first; a failure leaves the previous backend running so
    /// the host can report the error and tear the worker down cleanly.
    /// Needs feature `wasm-runtime` — without it the load fails cleanly.
    LoadWasm { path: String },
    SetParam { id: String, value: f64 },
    GetState,
    SetState { state: PluginState },
    /// Heartbeat: the worker answers immediately with no side effects.
    /// The host's watchdog sends these to tell "alive but mute" (a hung
    /// worker that never answers) apart from "alive and well".
    Ping,
    /// Render one mono block. `input.len()` is the block length.
    Process { frames: usize, input: Vec<f32> },
    /// Report the plugin's processing latency in samples at the current
    /// rate (feeds [`LatencyMap`](super::latency::LatencyMap); 0 = none).
    GetLatency,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_samples: Option<u32>,
}

impl WorkerResponse {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            output: None,
            state: None,
            latency_samples: None,
        }
    }

    fn fail(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            output: None,
            state: None,
            latency_samples: None,
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
    latency_samples: u32,
}

impl MockDsp {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            params: BTreeMap::from([("gain".to_string(), 1.0)]),
            blob: Vec::new(),
            latency_samples: 0,
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

    /// Processing latency in samples (mock: always 0 unless a test sets
    /// it; real backends query the plugin API at the current rate).
    pub fn latency_samples(&self) -> u32 {
        self.latency_samples
    }

    #[cfg(test)]
    pub fn set_latency_samples(&mut self, samples: u32) {
        self.latency_samples = samples;
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

/// The DSP behind one worker process: the mock gain plugin until a
/// `LoadClap` / `LoadVst3` / `LoadAu` / `LoadWasm` request swaps in a
/// real bundle. Every op below dispatches on the active backend, so the
/// host protocol never branches on kind.
enum Backend {
    Mock(MockDsp),
    Clap(ClapBackend),
    Vst3(Vst3Backend),
    Au(AuBackend),
    #[cfg(feature = "wasm-runtime")]
    Wasm(WasmBackend),
}

impl Backend {
    fn set_param(&mut self, id: &str, value: f64) -> std::result::Result<(), String> {
        match self {
            Self::Mock(dsp) => {
                dsp.set_param(id, value);
                Ok(())
            }
            Self::Clap(clap) => clap.set_param(id, value).map_err(|e| e.to_string()),
            Self::Vst3(vst3) => vst3.set_param(id, value).map_err(|e| e.to_string()),
            Self::Au(au) => au.set_param(id, value).map_err(|e| e.to_string()),
            #[cfg(feature = "wasm-runtime")]
            Self::Wasm(wasm) => wasm.set_param(id, value).map_err(|e| e.to_string()),
        }
    }

    fn state(&self) -> PluginState {
        match self {
            Self::Mock(dsp) => dsp.state(),
            Self::Clap(clap) => clap.state(),
            Self::Vst3(vst3) => vst3.state(),
            Self::Au(au) => au.state(),
            #[cfg(feature = "wasm-runtime")]
            Self::Wasm(wasm) => wasm.state(),
        }
    }

    fn set_state(&mut self, state: &PluginState) {
        match self {
            Self::Mock(dsp) => dsp.set_state(state),
            Self::Clap(clap) => clap.set_state(state),
            Self::Vst3(vst3) => vst3.set_state(state),
            Self::Au(au) => au.set_state(state),
            #[cfg(feature = "wasm-runtime")]
            Self::Wasm(wasm) => {
                // The gain guest cannot refuse its own code-0 push; a
                // trapped instance surfaces on the next fallible op.
                let _ = wasm.set_state(state);
            }
        }
    }

    fn process(&mut self, input: &[f32]) -> std::result::Result<Vec<f32>, String> {
        match self {
            Self::Mock(dsp) => Ok(dsp.process(input)),
            Self::Clap(clap) => clap.process(input).map_err(|e| e.to_string()),
            Self::Vst3(vst3) => vst3.process(input).map_err(|e| e.to_string()),
            Self::Au(au) => au.process(input).map_err(|e| e.to_string()),
            #[cfg(feature = "wasm-runtime")]
            Self::Wasm(wasm) => wasm.process(input).map_err(|e| e.to_string()),
        }
    }

    fn latency_samples(&mut self) -> u32 {
        match self {
            Self::Mock(dsp) => dsp.latency_samples(),
            Self::Clap(clap) => clap.latency_samples(),
            Self::Vst3(vst3) => vst3.latency_samples(),
            Self::Au(au) => au.latency_samples(),
            #[cfg(feature = "wasm-runtime")]
            Self::Wasm(wasm) => wasm.latency_samples(),
        }
    }
}

/// Run the worker loop on stdin/stdout until `Shutdown` or EOF.
/// Returns the process exit code.
pub fn worker_main() -> i32 {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut out = std::io::stdout();
    let mut backend: Option<Backend> = None;
    let mut sample_rate: Option<f64> = None;

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
            WorkerRequest::Init { sample_rate: rate } => {
                sample_rate = Some(rate);
                backend = Some(Backend::Mock(MockDsp::new(rate)));
                respond(&WorkerResponse::ok());
            }
            WorkerRequest::LoadClap { path, plugin_id } => match sample_rate {
                Some(rate) => {
                    let loaded = match plugin_id {
                        Some(id) => ClapBackend::load_selected(&path, &id, rate),
                        None => ClapBackend::load(&path, rate),
                    };
                    match loaded {
                        Ok(clap) => {
                            backend = Some(Backend::Clap(clap));
                            respond(&WorkerResponse::ok());
                        }
                        Err(e) => respond(&WorkerResponse::fail(e.to_string())),
                    }
                }
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::LoadVst3 { path, class_id } => match sample_rate {
                Some(rate) => {
                    let loaded = match class_id {
                        Some(id) => Vst3Backend::load_class(&path, rate, &id),
                        None => Vst3Backend::load(&path, rate),
                    };
                    match loaded {
                        Ok(vst3) => {
                            backend = Some(Backend::Vst3(vst3));
                            respond(&WorkerResponse::ok());
                        }
                        Err(e) => respond(&WorkerResponse::fail(e.to_string())),
                    }
                }
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::LoadAu {
                component_type,
                component_subtype,
                manufacturer,
            } => match sample_rate {
                Some(rate) => {
                    let desc = AuComponentDesc::new(component_type, component_subtype, manufacturer);
                    match AuBackend::load(&desc, rate) {
                        Ok(au) => {
                            backend = Some(Backend::Au(au));
                            respond(&WorkerResponse::ok());
                        }
                        Err(e) => respond(&WorkerResponse::fail(e.to_string())),
                    }
                }
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::LoadWasm { path } => match sample_rate {
                Some(rate) => {
                    #[cfg(feature = "wasm-runtime")]
                    match WasmBackend::load(&path, rate) {
                        Ok(wasm) => {
                            backend = Some(Backend::Wasm(wasm));
                            respond(&WorkerResponse::ok());
                        }
                        Err(e) => respond(&WorkerResponse::fail(e.to_string())),
                    }
                    #[cfg(not(feature = "wasm-runtime"))]
                    {
                        let _ = rate;
                        respond(&WorkerResponse::fail(format!(
                            "cannot load `{path}`: wasm devices need the wasm-runtime feature"
                        )));
                    }
                }
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::SetParam { id, value } => match &mut backend {
                Some(b) => match b.set_param(&id, value) {
                    Ok(()) => respond(&WorkerResponse::ok()),
                    Err(e) => respond(&WorkerResponse::fail(e)),
                },
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::GetState => match &backend {
                Some(b) => respond(&WorkerResponse {
                    ok: true,
                    error: None,
                    output: None,
                    state: Some(b.state()),
                    latency_samples: None,
                }),
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::GetLatency => match &mut backend {
                Some(b) => respond(&WorkerResponse {
                    ok: true,
                    error: None,
                    output: None,
                    state: None,
                    latency_samples: Some(b.latency_samples()),
                }),
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::Ping => respond(&WorkerResponse::ok()),
            WorkerRequest::SetState { state } => match &mut backend {
                Some(b) => {
                    b.set_state(&state);
                    respond(&WorkerResponse::ok());
                }
                None => respond(&WorkerResponse::fail("init first")),
            },
            WorkerRequest::Process { input, .. } => match &mut backend {
                Some(b) => match b.process(&input) {
                    Ok(output) => respond(&WorkerResponse {
                        ok: true,
                        error: None,
                        output: Some(output),
                        state: None,
                        latency_samples: None,
                    }),
                    Err(e) => respond(&WorkerResponse::fail(e)),
                },
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

/// Test helper: build the worker binary *with* the `wasm-runtime`
/// feature and stage a copy at a temp path. The copy keeps WASM tests
/// hermetic: other tests keep spawning the default-path binary (a
/// behavior-identical superset for every non-WASM backend), and a later
/// default rebuild can never pull the rug out mid-test.
///
/// Built once per test process ([`std::sync::OnceLock`]): the stage is a
/// copy, not a rename, so re-copying over a *running* worker's file
/// would truncate its image out from under it (SIGKILL on macOS) — one
/// copy shared by every WASM test avoids that entirely.
#[cfg(all(test, feature = "wasm-runtime"))]
pub(crate) fn ensure_wasm_worker_built() -> std::path::PathBuf {
    use crate::plugins::host::PluginHost;
    static STAGED: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    STAGED
        .get_or_init(|| {
            let manifest = if std::path::Path::new("core/Cargo.toml").exists() {
                "core/Cargo.toml"
            } else {
                "Cargo.toml"
            };
            let status = std::process::Command::new("cargo")
                .args([
                    "build",
                    "--manifest-path",
                    manifest,
                    "--bin",
                    WORKER_BIN_NAME,
                    "--features",
                    "wasm-runtime",
                ])
                .status()
                .expect("cargo build launches");
            let bin = PluginHost::default_worker_bin();
            assert!(
                status.success() && bin.exists(),
                "wasm plugin worker must build at {}",
                bin.display()
            );
            let staged = std::env::temp_dir().join(format!(
                "ccez-plugin-worker-wasm-{}",
                std::process::id()
            ));
            std::fs::copy(&bin, &staged).expect("stage wasm worker");
            staged
        })
        .clone()
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
            WorkerRequest::LoadClap {
                path: "/lib/gain.clap".to_string(),
                plugin_id: None,
            },
            WorkerRequest::LoadClap {
                path: "/lib/gain.clap".to_string(),
                plugin_id: Some("com.ccez.gain-two".to_string()),
            },
            WorkerRequest::LoadVst3 {
                path: "/lib/gain.vst3".to_string(),
                class_id: None,
            },
            WorkerRequest::LoadVst3 {
                path: "/lib/gain.vst3".to_string(),
                class_id: Some("C7ECDAE207A14C05B3E201179A64A70D".to_string()),
            },
            WorkerRequest::Ping,
            WorkerRequest::LoadAu {
                component_type: 0x6175_6678,
                component_subtype: 0x6465_6c79,
                manufacturer: 0x6170_706c,
            },
            WorkerRequest::LoadWasm {
                path: "/lib/gain.wasm".to_string(),
            },
            WorkerRequest::SetParam {
                id: "gain".to_string(),
                value: 0.5,
            },
            WorkerRequest::GetState,
            WorkerRequest::Process {
                frames: 2,
                input: vec![1.0, -1.0],
            },
            WorkerRequest::GetLatency,
            WorkerRequest::Shutdown,
        ] {
            let line = serde_json::to_string(&req).expect("serialize");
            assert!(!line.contains('\n'));
            let back: WorkerRequest = serde_json::from_str(&line).expect("deserialize");
            assert_eq!(back, req);
        }
    }

    #[test]
    fn legacy_load_lines_without_selection_still_parse() {
        // Descriptors written before selection existed carry no id: they
        // must keep loading the bundle default, not fail deserialization.
        let req: WorkerRequest =
            serde_json::from_str(r#"{"op":"load_clap","path":"/lib/gain.clap"}"#)
                .expect("legacy load_clap");
        assert_eq!(
            req,
            WorkerRequest::LoadClap {
                path: "/lib/gain.clap".to_string(),
                plugin_id: None,
            }
        );
        let req: WorkerRequest =
            serde_json::from_str(r#"{"op":"load_vst3","path":"/lib/gain.vst3"}"#)
                .expect("legacy load_vst3");
        assert_eq!(
            req,
            WorkerRequest::LoadVst3 {
                path: "/lib/gain.vst3".to_string(),
                class_id: None,
            }
        );
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
        // Latency defaults to zero-latency (in-place gain has no memory).
        assert_eq!(dsp.latency_samples(), 0);
        dsp.set_latency_samples(128);
        assert_eq!(dsp.latency_samples(), 128);
    }
}
