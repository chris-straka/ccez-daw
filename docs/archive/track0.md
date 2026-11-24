# Track 0 primer: how a DAW is put together (and how this repo mirrors it)

A DAW has four jobs: store the musical idea (project document), turn it into
sound on time (audio engine), let instruments/effects plug in (plugin host),
and let the human edit fast (timeline, mixer, piano roll, keybindings).

## The ideas behind this scaffold

- **Project document as event-sourced op log** (`contracts/op-log-format.md`).
  Instead of saving "the current state", we save every edit as an op
  (`ClipMoved`, `ParamSet`, …). Current state = replay the log. Undo = step
  back; branches = alternate op sequences; crash recovery = the log is already
  on disk. This one decision buys autosave, infinite undo, and merge.
- **One routing graph** (`Edge` in `contracts/project-schema.md`). Sound,
  MIDI, and modulation all travel as edges between nodes. The mixer you see
  on screen is just a pretty view over the audio edges — there is only one
  graph to keep correct.
- **Universal param addressing** (`node:param`). Volume faders, plugin knobs,
  and LFO targets are all the same thing: an address plus a value. Automation
  lanes and modulators write to addresses; that is what makes
  anything-modulates-anything a generalization instead of a rewrite.
- **Rust owns truth, the UI renders it.** The audio thread can never wait on
  the UI. So all state lives in `core/` (Rust); the SolidJS frontend only
  holds a rendered copy fetched over IPC. Nothing crosses that boundary
  except the frozen table in `contracts/ipc-table.md` — which is why the
  TypeScript types are *generated from* the Rust types, and the build fails
  if they drift.
- **Every edit is an op, whoever makes it.** Mouse, vim keybinding, script,
  MCP tool, or AI sidecar — all produce ordinary ops from a named actor.
  That is why AI output is editable and undoable instead of flattened audio.
