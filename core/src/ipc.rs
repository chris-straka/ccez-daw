//! Frozen v0 IPC command/event table.
//!
//! Nothing crosses the UI/Rust boundary except these commands and events,
//! typed by `model` and mirrored to TypeScript by `emit`. The human-readable
//! table is `contracts/ipc-table.md`; this file is the machine truth.

/// One Tauri `invoke` command. `args_ts` / `returns_ts` name types emitted
/// into `ui/src/generated/project.ts` (or builtins like `void`, `string`).
pub struct CommandDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub args_ts: &'static str,
    pub returns_ts: &'static str,
}

/// One Tauri `listen` event. `payload_ts` names an emitted type.
pub struct EventDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub payload_ts: &'static str,
}

pub const COMMANDS: &[CommandDef] = &[
    CommandDef {
        name: "project_new",
        doc: "Create a new empty project with the given name.",
        args_ts: "{ name: string }",
        returns_ts: "Project",
    },
    CommandDef {
        name: "project_get",
        doc: "Return the currently open project.",
        args_ts: "Record<string, never>",
        returns_ts: "Project",
    },
    CommandDef {
        name: "project_save",
        doc: "Persist the project (instant autosave path, Track A owns the engine).",
        args_ts: "{ path: string }",
        returns_ts: "void",
    },
    CommandDef {
        name: "project_open",
        doc: "Open a project from disk.",
        args_ts: "{ path: string }",
        returns_ts: "Project",
    },
    CommandDef {
        name: "op_apply",
        doc: "Append one op-log entry and return its sequence number.",
        args_ts: "{ op: Op }",
        returns_ts: "number",
    },
    CommandDef {
        name: "op_undo",
        doc: "Undo one op; returns the undone sequence number.",
        args_ts: "Record<string, never>",
        returns_ts: "number",
    },
    CommandDef {
        name: "op_redo",
        doc: "Redo one op; returns the redone sequence number.",
        args_ts: "Record<string, never>",
        returns_ts: "number",
    },
    CommandDef {
        name: "track_add",
        doc: "Add a track and return its id.",
        args_ts: "{ name: string }",
        returns_ts: "string",
    },
    CommandDef {
        name: "clip_add",
        doc: "Add a clip to a track and return its id.",
        args_ts: "{ clip: Clip }",
        returns_ts: "string",
    },
    CommandDef {
        name: "param_set",
        doc: "Set one addressed parameter (universal param addressing).",
        args_ts: "{ target: ParamAddress; value: number }",
        returns_ts: "void",
    },
    CommandDef {
        name: "engine_play",
        doc: "Start the audio engine transport.",
        args_ts: "Record<string, never>",
        returns_ts: "EngineState",
    },
    CommandDef {
        name: "engine_stop",
        doc: "Stop the audio engine transport.",
        args_ts: "Record<string, never>",
        returns_ts: "EngineState",
    },
    CommandDef {
        name: "engine_set_tempo",
        doc: "Set the transport tempo in BPM.",
        args_ts: "{ tempo: number }",
        returns_ts: "void",
    },
];

pub const EVENTS: &[EventDef] = &[
    EventDef {
        name: "project_changed",
        doc: "Emitted after any applied/undone/redone op.",
        payload_ts: "Project",
    },
    EventDef {
        name: "engine_state_changed",
        doc: "Emitted whenever the transport state changes.",
        payload_ts: "EngineState",
    },
    EventDef {
        name: "param_changed",
        doc: "Emitted after param_set; target is `node:param`.",
        payload_ts: "ParamAddress",
    },
    EventDef {
        name: "op_applied",
        doc: "Emitted for every op-log append, with its sequence number.",
        payload_ts: "Op",
    },
];
