# Track A primer: the project engine (history that survives `kill -9`)

New to event-sourced projects? Start here. This note teaches the four ideas
behind `core/src/engine.rs`, then maps each one to the exact function that
implements it. The frozen rules it obeys live in
`contracts/op-log-format.md`; the data shapes live in `core/src/model.rs`.

## 1. State is history, not a snapshot

Most apps save "the current arrangement". We save every edit as an *op* —
`TrackAdded`, `ClipMoved`, `ParamSet`, … — in an append-only log. Current
state = replay the log from the beginning.

Why? One decision buys three features: **autosave** (the log is already on
disk), **infinite undo** (step back through the log), and **branch/merge**
(compare op sequences). The price is replay cost, which idea 3 pays down.

The map: [`Engine::apply`](../../core/src/engine.rs) validates an op,
applies it, appends it to `ops.jsonl`, and returns its `seq` (sequence
number, assigned here — never by the caller). [`Engine::live_ops`] and
[`Engine::ops_since`] expose the log at op granularity for future merge
(compare with `core/src/branch.rs`).

```rust
let seq = engine.apply("ai:jam", OpKind::ClipMoved, "clip_b", "{\"startBeats\": 8}")?;
```

Every edit — mouse, keybinding, script, MCP tool, AI sidecar — flows through
`apply` with a named actor (`ui`, `mcp`, `ai:<sidecar>`, `script:<name>`,
checked by `valid_actor`). AI output is therefore ordinary undoable history,
never flattened audio.

## 2. Undo is also history (so it survives restart)

Undo does **not** delete anything. It appends an `UndoMarker` op naming the
undone `seq`; redo appends a second marker (`{"redo": true}`) cancelling the
first. Both directions are just more log entries, so quitting mid-undo loses
nothing: reopening folds the markers (`fold_undone`) and replays.

The map: [`Engine::undo`] returns the undone seq, [`Engine::redo`] returns
the new marker seq. New edits never wipe the redo stack — undone branches
stay addressable, which is what branch compare/merge builds on. There is
deliberately no `close()`: every mutation is fsynced before it returns, so
dropping the engine *is* a clean shutdown.

## 3. Snapshots bound replay

Replaying 100k ops on every open would be slow, so every 64 ops (tunable via
`set_snapshot_every`, `0` = manual) the engine writes `snapshot.json`: the
full project at the current tip. Opens replay only the tail after it.

The map: [`Engine::snapshot`], [`Engine::snapshot_seq`]. Recovery still
never trusts `project.json` (a crash can tear it mid-write): it rebuilds
from snapshot + log and skips a torn trailing line in `ops.jsonl`.

## 4. Heavy bytes are lazy, the package is portable

Audio, MIDI, presets, and frozen plugin states are opaque files under
`assets/`, indexed by `manifest.json` and read only via `load_asset` —
opening a project never touches them (lazy load). Clips point at them
through their `source` field. `export_bundle` copies the whole directory
(project + log + snapshot + manifest + assets) as one portable package;
`import_bundle` verifies every manifest entry (present, size matches) and
opens it. Automation travels inside the project doc, so it is in the bundle
for free. "Frozen" plugin state means verbatim bytes: stored and returned
untouched, never parsed.

The map: [`Engine::store_asset`], [`Engine::load_asset`],
[`Engine::asset_keys`], [`Engine::export_bundle`], [`import_bundle`].

## 5. On-disk layout

```text
<project-dir>/
  project.json   last materialized state (convenience only, never trusted)
  ops.jsonl      the truth: one JSON op per line, append-only, fsynced
  snapshot.json  { seq, project } bounding replay (optional)
  manifest.json  asset index { key, kind, size }
  assets/<key>   opaque blobs (audio, midi, preset, plugin)
```

## 6. How to verify

- `cargo test --manifest-path core/Cargo.toml` — includes the `kill -9`
  test (drop without close, delete `project.json`, reopen, state identical),
  the cross-session undo test, the torn-tail test, snapshot-bounding, and
  bundle round-trip/rejection tests. All live in the `tests` module at the
  bottom of `engine.rs`.
- `bun run typegen -- --check` — unaffected (Track A adds no types; the
  drift gate still passes).

## 7. What Track A deliberately leaves out

- `src-tauri/src/lib.rs` still holds stub commands (`op_apply` echoes the
  seq, `op_undo` returns 0). Wiring those stubs to `Engine` (open on
  startup, `apply`/`undo`/`redo` per invoke, emit `project_changed`) is the
  integration follow-up — the engine API already matches the IPC shapes
  (`op_apply` returns `u64`, `op_undo`/`op_redo` return `u64`).
- No merge UI and no `.ccez` single-file zip: the bundle is a directory
  package (copy = portable), and merge lives with the branch track.
- `model.rs` untouched: the engine uses the frozen `Op`/`OpKind`/`Project`
  as-is, so no typegen drift was possible.
