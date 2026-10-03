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
    dir: Mutex<PathBuf>,
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
    let project = engine.project().clone();
    // Decoded `audio` assets ride along so `asset:` clips sound live;
    // engines without audio assets render byte-identical procedural audio.
    let bank = engine.sample_bank();
    drop(engine);
    let rate = state
        .transport
        .lock()
        .expect("transport lock")
        .sample_rate();
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

#[tauri::command]
fn project_new(name: String, state: State<AppState>) -> Result<Project, String> {
    let dir = state.dir.lock().map_err(|_| poisoned("dir"))?.clone();
    let mut engine = state.engine.lock().map_err(|_| poisoned("engine"))?;
    *engine = Engine::create(&dir, Project::new("proj_1", &name)).map_err(|e| e.to_string())?;
    Ok(engine.project().clone())
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
    *state.engine.lock().map_err(|_| poisoned("engine"))? = engine;
    *state.dir.lock().map_err(|_| poisoned("dir"))? = PathBuf::from(&path);
    Ok(project)
}

#[tauri::command]
fn op_apply(op: Op, state: State<AppState>) -> Result<u64, String> {
    let seq = state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .apply(&op.actor, op.kind, &op.target, &op.value_json)
        .map_err(|e| e.to_string())?;
    push_graph_if_playing(&state);
    Ok(seq)
}

#[tauri::command]
fn op_undo(state: State<AppState>) -> Result<u64, String> {
    let seq = state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .undo()
        .map_err(|e| e.to_string())?;
    push_graph_if_playing(&state);
    Ok(seq)
}

#[tauri::command]
fn op_redo(state: State<AppState>) -> Result<u64, String> {
    let seq = state
        .engine
        .lock()
        .map_err(|_| poisoned("engine"))?
        .redo()
        .map_err(|e| e.to_string())?;
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
fn asset_store(key: String, kind: String, bytes: Vec<u8>, state: State<AppState>) -> Result<(), String> {
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
fn engine_set_tempo(tempo: f64, state: State<AppState>) {
    state
        .transport
        .lock()
        .expect("transport lock")
        .set_tempo(tempo);
    // The live loop is rendered at the push-time tempo: re-render so the
    // loop follows (restarts at the loop start — a v1 tradeoff the live
    // module documents).
    push_graph_if_playing(&state);
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
            app.manage(AppState {
                engine: Mutex::new(engine),
                dir: Mutex::new(dir),
                transport: Mutex::new(TransportController::new()),
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
            engine_position
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
