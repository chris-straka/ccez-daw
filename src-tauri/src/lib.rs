//! Ccez Daw Tauri 2 shell: thin command layer over the real core engines.
//!
//! Every command below mutates or queries durable state — no stubs. The
//! working project lives in an `Engine` (op log + snapshot + assets under
//! the app-data dir); the transport renders it through `swap_graph` on
//! every play and every applied op while playing.

mod menu;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ccez_core::audio::graph::AudioGraph;
use ccez_core::audio::render::RenderGraph;
use ccez_core::audio::TransportController;
use ccez_core::engine::Engine;
use ccez_core::model::{Clip, EngineState, Op, OpKind, ParamAddress, Project, Track};
use tauri::{Manager, State};

struct AppState {
    engine: Mutex<Engine>,
    projects_root: PathBuf,
    transport: Mutex<TransportController>,
}

fn poisoned(what: &str) -> String {
    format!("{what} lock poisoned")
}

fn io_err(e: impl ToString) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

/// Rebuild the render graph from the current project and hand it to the
/// live backend (no-op while stopped). Click-free by construction: the
/// callback swaps between blocks.
fn push_graph(state: &State<AppState>) {
    let engine = state.engine.lock().expect("engine lock");
    let mut project = engine.project().clone();
    // Decoded `audio` assets ride along so `asset:` clips sound live;
    // engines without audio assets render byte-identical procedural audio.
    let bank = engine.sample_bank();
    drop(engine);
    let transport = state.transport.lock().expect("transport lock");
    let rate = transport.sample_rate();
    project.tempo = transport.tempo();
    drop(transport);
    // Audible path first: the project's mix loop pre-rendered at the
    // transport rate. Bare topology (silent) only when there is nothing
    // to sound or the render fails.
    let (graph, delays) = match ccez_core::audio::live_graph(&project, rate, &bank) {
        Some((graph, delays)) => (graph, delays),
        None => {
            let topo = AudioGraph::from_project(&project);
            let delays = match topo.all_edge_delays() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("graph delays failed: {e}");
                    return;
                }
            };
            (RenderGraph::from_audio_graph(topo), delays)
        }
    };
    state
        .transport
        .lock()
        .expect("transport lock")
        .swap_graph(graph, delays, "mix".to_string());
}

fn push_graph_if_playing(state: &State<AppState>) {
    let state_now = state
        .transport
        .lock()
        .expect("transport lock")
        .engine_state();
    // Recording passes hear mix edits too: the loop was rendered at pass
    // start and every op re-renders it.
    if state_now == EngineState::Playing || state_now == EngineState::Recording {
        push_graph(state);
    }
}

/// Reserve a separate directory before creating a new project's durable files.
fn create_fresh_project(root: &Path, name: &str) -> Result<Engine, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    for suffix in 0u64.. {
        let id = format!("proj_{stamp}_{suffix}");
        let dir = root.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                return Engine::create(&dir, Project::new(&id, name)).map_err(|e| e.to_string())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    unreachable!()
}

fn sync_project_tempo(state: &State<AppState>) {
    let tempo = state.engine.lock().expect("engine lock").project().tempo;
    state
        .transport
        .lock()
        .expect("transport lock")
        .set_tempo(tempo);
}

#[tauri::command]
fn project_new(name: String, state: State<AppState>) -> Result<Project, String> {
    let engine = create_fresh_project(&state.projects_root, &name)?;
    let project = engine.project().clone();
    state
        .transport
        .lock()
        .map_err(|_| poisoned("transport"))?
        .stop();
    *state.engine.lock().map_err(|_| poisoned("engine"))? = engine;
    sync_project_tempo(&state);
    Ok(project)
}

#[tauri::command]
fn project_get(state: State<AppState>) -> Result<Project, String> {
    Ok(state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .project()
        .clone())
}

#[tauri::command]
fn project_save(path: String, state: State<AppState>) -> Result<(), String> {
    let dest = PathBuf::from(&path);
    if dest.exists() {
        return Err(format!("save path exists (pick a new path): {path}"));
    }
    let engine = state.engine.lock().map_err(|_| poisoned("engine"))?;
    engine.export_bundle(&dest).map_err(|e| e.to_string())
}

#[tauri::command]
fn project_open(path: String, state: State<AppState>) -> Result<Project, String> {
    let engine = Engine::open(Path::new(&path)).map_err(|e| e.to_string())?;
    let project = engine.project().clone();
    state
        .transport
        .lock()
        .map_err(|_| poisoned("transport"))?
        .stop();
    *state.engine.lock().map_err(|_| poisoned("engine"))? = engine;
    sync_project_tempo(&state);
    Ok(project)
}

#[tauri::command]
fn op_apply(op: Op, state: State<AppState>) -> Result<u64, String> {
    let changes_tempo = matches!(&op.kind, OpKind::TempoSet);
    let seq = state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .apply(&op.actor, op.kind, &op.target, &op.value_json)
        .map_err(|e| e.to_string())?;
    if changes_tempo {
        sync_project_tempo(&state);
    }
    push_graph_if_playing(&state);
    Ok(seq)
}

#[tauri::command]
fn op_undo(state: State<AppState>) -> Result<u64, String> {
    let (seq, changes_tempo) = {
        let mut engine = state.engine.lock().map_err(|_| poisoned("engine"))?;
        let before = engine.project().tempo;
        let seq = engine.undo().map_err(|e| e.to_string())?;
        (seq, engine.project().tempo != before)
    };
    if changes_tempo {
        sync_project_tempo(&state);
    }
    push_graph_if_playing(&state);
    Ok(seq)
}

#[tauri::command]
fn op_redo(state: State<AppState>) -> Result<u64, String> {
    let (seq, changes_tempo) = {
        let mut engine = state.engine.lock().map_err(|_| poisoned("engine"))?;
        let before = engine.project().tempo;
        let seq = engine.redo().map_err(|e| e.to_string())?;
        (seq, engine.project().tempo != before)
    };
    if changes_tempo {
        sync_project_tempo(&state);
    }
    push_graph_if_playing(&state);
    Ok(seq)
}

#[tauri::command]
fn track_add(name: String, state: State<AppState>) -> Result<String, String> {
    let mut engine = state.engine.lock().map_err(|_| poisoned("engine"))?;
    let id = format!("trk_{}", engine.next_seq());
    let track = Track {
        id: id.clone(),
        name,
        volume: 0.8,
        pan: 0.0,
        muted: false,
        solo: false,
        clip_ids: Vec::new(),
        device_ids: Vec::new(),
    };
    let json = serde_json::to_string(&track).map_err(|e| e.to_string())?;
    engine
        .apply("ui", OpKind::TrackAdded, &id, &json)
        .map_err(|e| e.to_string())?;
    drop(engine);
    push_graph_if_playing(&state);
    Ok(id)
}

#[tauri::command]
fn clip_add(clip: Clip, state: State<AppState>) -> Result<String, String> {
    let mut engine = state.engine.lock().map_err(|_| poisoned("engine"))?;
    let json = serde_json::to_string(&clip).map_err(|e| e.to_string())?;
    engine
        .apply("ui", OpKind::ClipAdded, &clip.id, &json)
        .map_err(|e| e.to_string())?;
    drop(engine);
    push_graph_if_playing(&state);
    Ok(clip.id)
}

#[tauri::command]
fn param_set(target: ParamAddress, value: f64, state: State<AppState>) -> Result<(), String> {
    state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .apply(
            "ui",
            OpKind::ParamSet,
            &format!("{}:{}", target.node, target.param),
            &value.to_string(),
        )
        .map_err(|e| e.to_string())?;
    push_graph_if_playing(&state);
    Ok(())
}

/// Store an opaque blob (MIDI bytes, audio, preset). Clips reference it
/// through their `source` field; bytes stay on disk until loaded.
#[tauri::command]
fn asset_store(
    key: String,
    kind: String,
    bytes: Vec<u8>,
    state: State<AppState>,
) -> Result<(), String> {
    state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .store_asset(&key, &kind, &bytes)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn asset_load(key: String, state: State<AppState>) -> Result<Vec<u8>, String> {
    state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .load_asset(&key)
        .map_err(|e| e.to_string())
}

/// List stored assets without reading bytes (feeds import pickers).
#[tauri::command]
fn asset_list(state: State<AppState>) -> Vec<ccez_core::engine::AssetEntry> {
    state
        .engine
        .lock()
        .map(|e| e.asset_keys())
        .unwrap_or_default()
}

/// Start the realtime transport: opens (or reuses) a cpal output stream
/// driving the deterministic renderer, degrading to the null device on
/// headless/CI machines with no audio hardware. The current project graph
/// is pushed first so play renders the song, never silence. Never panics —
/// a half-broken hardware layer reports `Stopped` instead.
#[tauri::command]
fn engine_play(state: State<AppState>) -> EngineState {
    let transport = state.transport.lock().expect("transport lock");
    match transport.play() {
        Ok(s) => {
            drop(transport);
            push_graph(&state);
            s
        }
        Err(e) => {
            eprintln!("engine_play: audio stream failed ({e}); staying stopped");
            EngineState::Stopped
        }
    }
}

/// Stop the realtime transport: closes/suspends the output stream.
#[tauri::command]
fn engine_stop(state: State<AppState>) -> EngineState {
    state.transport.lock().expect("transport lock").stop()
}

/// Start a recording pass: the transport runs (song audible for context)
/// with the state reading Recording, arming the punch/take path. Degrades
/// to the null device like `engine_play`; a half-broken hardware layer
/// reports `Stopped` instead of panicking.
#[tauri::command]
fn engine_record(state: State<AppState>) -> EngineState {
    let transport = state.transport.lock().expect("transport lock");
    match transport.record() {
        Ok(s) => {
            drop(transport);
            push_graph(&state);
            s
        }
        Err(e) => {
            eprintln!("engine_record: audio stream failed ({e}); staying stopped");
            EngineState::Stopped
        }
    }
}

/// Join/leave the Link session clock (`link.join` action, `L` key):
/// returns the new membership. Tempo follows the session while joined.
#[tauri::command]
fn link_toggle(state: State<AppState>) -> bool {
    state
        .transport
        .lock()
        .expect("transport lock")
        .toggle_link()
}

/// Set the transport tempo in BPM; maps to block scheduling from the next
/// rendered block.
#[tauri::command]
fn engine_set_tempo(tempo: f64, state: State<AppState>) -> Result<(), String> {
    if !tempo.is_finite() || tempo <= 0.0 {
        return Err("tempo must be positive and finite".into());
    }
    let tempo = tempo.clamp(1.0, 960.0);
    state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .apply("ui", OpKind::TempoSet, "tempo", &tempo.to_string())
        .map_err(|e| e.to_string())?;
    sync_project_tempo(&state);
    push_graph_if_playing(&state);
    Ok(())
}

/// Current transport position in beats (lock-free read; 0 while stopped).
#[tauri::command]
fn engine_position(state: State<AppState>) -> f64 {
    state
        .transport
        .lock()
        .expect("transport lock")
        .stats()
        .position_beats
}

/// Ask the release feed (`plugins.updater.endpoints`, the GitHub
/// `latest.json`) whether a newer signed build exists. Returns a status
/// line for the shell (`update: 0.1.2 available (running 0.1.1)`).
#[tauri::command]
async fn app_update_check(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    let update = app
        .updater()
        .map_err(|e| format!("update: {e}"))?
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    Ok(match update {
        Some(u) => format!("update: {} available (running {current})", u.version),
        None => format!("update: up to date ({current})"),
    })
}

/// Download, verify (minisign signature against the bundled pubkey) and
/// install the newer build, then restart into it. Returns a status line
/// when there is nothing to install.
#[tauri::command]
async fn app_update_install(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    let update = app
        .updater()
        .map_err(|e| format!("update: {e}"))?
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    let Some(update) = update else {
        return Ok(format!("update: up to date ({current})"));
    };
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("update install failed: {e}"))?;
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            menu::install(app)?;
            let dir = app
                .path()
                .app_data_dir()
                .map_err(io_err)?
                .join("ccez-daw")
                .join("project");
            std::fs::create_dir_all(&dir).map_err(io_err)?;
            let engine = if dir.join("project.json").exists() {
                Engine::open(&dir)
            } else {
                Engine::create(&dir, Project::sample())
            }
            .map_err(io_err)?;
            let transport = TransportController::new();
            transport.set_tempo(engine.project().tempo);
            app.manage(AppState {
                engine: Mutex::new(engine),
                projects_root: dir.parent().expect("app project parent").join("projects"),
                transport: Mutex::new(transport),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            project_new,
            project_get,
            project_save,
            project_open,
            op_apply,
            op_undo,
            op_redo,
            track_add,
            clip_add,
            param_set,
            asset_store,
            asset_load,
            asset_list,
            engine_play,
            engine_record,
            engine_stop,
            link_toggle,
            engine_set_tempo,
            engine_position,
            app_update_check,
            app_update_install
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    struct Fixture {
        app: tauri::App<MockRuntime>,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "daw-command-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let dir = root.join("working");
            let engine = Engine::create(&dir, Project::sample()).unwrap();
            let app = mock_builder()
                .manage(AppState {
                    engine: Mutex::new(engine),
                    projects_root: dir.parent().expect("app project parent").join("projects"),
                    transport: Mutex::new(TransportController::new()),
                })
                .build(mock_context(noop_assets()))
                .unwrap();
            Self { app, root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.app
                .state::<AppState>()
                .transport
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .stop();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn adding_tracks_and_clips_while_running_completes() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let fixture = Fixture::new();
            let state = fixture.app.state::<AppState>();
            for recording in [false, true] {
                if recording {
                    state.transport.lock().unwrap().record_null();
                } else {
                    state.transport.lock().unwrap().play_null();
                }
                let id = track_add("Live".into(), state.clone()).unwrap();
                let clip = Clip {
                    id: format!("clip_{id}"),
                    name: "Live".into(),
                    track_id: id,
                    start_beats: 0.0,
                    length_beats: 1.0,
                    kind: ccez_core::model::ClipKind::Midi,
                    source: "take:live".into(),
                };
                clip_add(clip, state.clone()).unwrap();
                state.transport.lock().unwrap().stop();
            }
            tx.send(()).unwrap();
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("adding a track or clip must not deadlock the engine");
    }

    #[test]
    fn new_preserves_opened_project_history_and_assets() {
        let fixture = Fixture::new();
        let saved = fixture.root.join("saved");
        let mut original = Engine::create(&saved, Project::sample()).unwrap();
        original
            .apply("ui", OpKind::TempoSet, "tempo", "137")
            .unwrap();
        original
            .store_asset("keep.bin", "midi", &[1, 2, 3])
            .unwrap();
        original.snapshot().unwrap();
        let expected = serde_json::to_value(original.project()).unwrap();
        let log_before = std::fs::read(saved.join("ops.jsonl")).unwrap();
        let state = fixture.app.state::<AppState>();
        project_open(saved.to_string_lossy().into_owned(), state.clone()).unwrap();
        state.transport.lock().unwrap().play_null();
        let fresh = project_new("Fresh".into(), state.clone()).unwrap();
        assert_eq!(fresh.name, "Fresh");
        assert_eq!(
            state.transport.lock().unwrap().engine_state(),
            EngineState::Stopped
        );
        assert_eq!(std::fs::read(saved.join("ops.jsonl")).unwrap(), log_before);
        let reopened = Engine::open(&saved).unwrap();
        assert_eq!(serde_json::to_value(reopened.project()).unwrap(), expected);
        assert_eq!(reopened.load_asset("keep.bin").unwrap(), [1, 2, 3]);
    }

    #[test]
    fn tempo_command_updates_durable_project_and_undo() {
        let fixture = Fixture::new();
        let state = fixture.app.state::<AppState>();
        engine_set_tempo(90.0, state.clone()).unwrap();
        assert_eq!(project_get(state.clone()).unwrap().tempo, 90.0);
        assert_eq!(state.transport.lock().unwrap().tempo(), 90.0);
        let project = project_get(state.clone()).unwrap();
        let beats = ccez_core::audio::live::loop_beats(&project).unwrap();
        let (graph, _) =
            ccez_core::audio::live_graph(&project, 48000, &ccez_core::bounce::SampleBank::empty())
                .unwrap();
        match graph.proc_of("mix") {
            ccez_core::audio::render::Proc::Loop { buf } => {
                assert_eq!(buf.len(), (beats * 60.0 / 90.0 * 48000.0).round() as usize);
            }
            _ => panic!("live mix must carry the tempo-adjusted audio loop"),
        }
        op_undo(state.clone()).unwrap();
        assert_eq!(state.transport.lock().unwrap().tempo(), 120.0);
        op_redo(state.clone()).unwrap();
        assert_eq!(state.transport.lock().unwrap().tempo(), 90.0);
    }

    #[test]
    fn unrelated_edits_preserve_link_session_tempo() {
        let fixture = Fixture::new();
        let state = fixture.app.state::<AppState>();
        {
            let transport = state.transport.lock().unwrap();
            transport.set_link_enabled(true);
            transport.set_tempo(96.0);
        }
        let track_id = project_get(state.clone()).unwrap().tracks[0].id.clone();
        let op = Op {
            seq: 0,
            actor: "ui".into(),
            kind: OpKind::ParamSet,
            target: format!("{track_id}:volume"),
            value_json: "0.4".into(),
        };
        op_apply(op, state.clone()).unwrap();
        assert_eq!(state.transport.lock().unwrap().tempo(), 96.0);
        op_undo(state.clone()).unwrap();
        assert_eq!(state.transport.lock().unwrap().tempo(), 96.0);
        op_redo(state.clone()).unwrap();
        assert_eq!(state.transport.lock().unwrap().tempo(), 96.0);
    }

    #[test]
    fn tempo_command_rejects_invalid_values_without_persisting() {
        let fixture = Fixture::new();
        let state = fixture.app.state::<AppState>();
        for tempo in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(engine_set_tempo(tempo, state.clone()).is_err());
        }
        assert_eq!(project_get(state.clone()).unwrap().tempo, 120.0);
        assert_eq!(state.engine.lock().unwrap().next_seq(), 1);
    }

    #[test]
    fn opening_project_stops_previous_transport() {
        let fixture = Fixture::new();
        let saved = fixture.root.join("other");
        Engine::create(&saved, Project::new("other", "Other")).unwrap();
        let state = fixture.app.state::<AppState>();
        state.transport.lock().unwrap().record_null();
        project_open(saved.to_string_lossy().into_owned(), state.clone()).unwrap();
        assert_eq!(
            state.transport.lock().unwrap().engine_state(),
            EngineState::Stopped
        );
    }
}
