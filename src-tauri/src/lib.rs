//! ccez-daw Tauri 2 shell. Thin by design: every command below maps 1:1 onto
//! the frozen v0 table in `ccez-core::ipc`. Track A/B own the real engines;
//! these stubs return well-typed values so the invoke round-trip works on day one.

use ccez_core::model::{Clip, EngineState, Op, ParamAddress, Project};
use std::sync::Mutex;
use tauri::State;

struct AppState {
    project: Mutex<Project>,
    engine: Mutex<EngineState>,
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

#[tauri::command]
fn engine_play(state: State<AppState>) -> EngineState {
    *state.engine.lock().expect("engine lock") = EngineState::Playing;
    EngineState::Playing
}

#[tauri::command]
fn engine_stop(state: State<AppState>) -> EngineState {
    *state.engine.lock().expect("engine lock") = EngineState::Stopped;
    EngineState::Stopped
}

#[tauri::command]
fn engine_set_tempo(_tempo: f64) {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            project: Mutex::new(Project::sample()),
            engine: Mutex::new(EngineState::Stopped),
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
