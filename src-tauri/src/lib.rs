//! ccez-daw Tauri 2 shell. Thin by design: every command below maps 1:1 onto
//! the frozen v0 table in `ccez-core::ipc`. Track A/B own the real engines;
//! these stubs return well-typed values so the invoke round-trip works on day one.

mod menu;

use ccez_core::audio::TransportController;
use ccez_core::model::{Clip, EngineState, Op, ParamAddress, Project};
use std::sync::Mutex;
use tauri::State;

struct AppState {
    project: Mutex<Project>,
    transport: Mutex<TransportController>,
}

#[tauri::command]
fn project_new(name: String, state: State<AppState>) -> Project {
    let p = Project::new("proj_1", &name);
    *state.project.lock().expect("project lock") = p.clone();
    p
}

#[tauri::command]
fn project_get(state: State<AppState>) -> Project {
    state.project.lock().expect("project lock").clone()
}

#[tauri::command]
fn project_save(_path: String, _state: State<AppState>) {}

#[tauri::command]
fn project_open(_path: String, state: State<AppState>) -> Project {
    state.project.lock().expect("project lock").clone()
}

#[tauri::command]
fn op_apply(op: Op, _state: State<AppState>) -> u64 {
    op.seq
}

#[tauri::command]
fn op_undo(_state: State<AppState>) -> u64 {
    0
}

#[tauri::command]
fn op_redo(_state: State<AppState>) -> u64 {
    0
}

#[tauri::command]
fn track_add(name: String, state: State<AppState>) -> String {
    let _ = name;
    let _ = state;
    "trk_1".to_string()
}

#[tauri::command]
fn clip_add(clip: Clip, _state: State<AppState>) -> String {
    clip.id
}

#[tauri::command]
fn param_set(_target: ParamAddress, _value: f64) {}

/// Start the realtime transport: opens (or reuses) a cpal output stream
/// driving the deterministic renderer, degrading to the null device on
/// headless/CI machines with no audio hardware. Never panics — a
/// half-broken hardware layer reports `Stopped` instead.
#[tauri::command]
fn engine_play(state: State<AppState>) -> EngineState {
    let transport = state.transport.lock().expect("transport lock");
    match transport.play() {
        Ok(s) => s,
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

/// Set the transport tempo in BPM; maps to block scheduling from the next
/// rendered block.
#[tauri::command]
fn engine_set_tempo(tempo: f64, state: State<AppState>) {
    state
        .transport
        .lock()
        .expect("transport lock")
        .set_tempo(tempo);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            menu::install(app)?;
            Ok(())
        })
        .manage(AppState {
            project: Mutex::new(Project::sample()),
            transport: Mutex::new(TransportController::new()),
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
            engine_play,
            engine_stop,
            engine_set_tempo
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
