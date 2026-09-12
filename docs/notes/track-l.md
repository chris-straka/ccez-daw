# Track L primer: MCP server tools

New to MCP (Model Context Protocol)? Start here. This note teaches the one
big idea behind Track L — **AI output is just ops in the log** — then lists
exactly what Track L added so later tracks can extend it without forking it.

## 1. The idea

An MCP tool is a function an AI agent can call: list the tracks, add a clip,
set a parameter, press play. The danger is a parallel universe — agent edits
that bypass undo, don't autosave, and can't be inspected. Track L avoids that
by construction: **every mutating tool appends an ordinary op with actor
`"mcp"`** (`contracts/op-log-format.md`). An agent-added clip is a
`ClipAdded` op like any other, so `op_undo` removes it, instant autosave
persists it, and the op log shows who did it. Compare Track K: scripts funnel
through `op_apply` with `script:<name>` actors for the same reason. One log,
many writers.

Devices, routing, and automation need no extra tools. The universal node
model (`contracts/project-schema.md`) makes them views over data the six
frozen tools already touch: `project_get` returns `devices`, `routing`, and
`automation` lists, and `param_set` addresses anything via `node:param`
(`trk_1:volume`, a device cutoff, a modulator depth — same shape). New
surfaces reuse addresses; they don't mint new verbs.

## 2. The pieces

- `mcp/src/backend.ts` — the seam. `InMemoryBackend` holds a `ProjectView`
  (snake_case field names copied from the generated project shapes, so the
  MCP surface can't drift from the UI) plus the op log. `addClip` / `setParam`
  validate (finite numbers, `lengthBeats > 0`, known track) and append
  `ClipAdded` / `ParamSet` ops; `undo()` mirrors `op_undo` (reverts the
  `ClipAdded`, records an `UndoMarker`, returns the undone seq). Swap this
  class for a Tauri/core-backed one without touching tool shapes.
- `mcp/src/tools.ts` — unchanged frozen `TOOLS` table (six names mirroring
  `contracts/mcp-tools.md` → `contracts/action-registry.md`), plus the
  additive handler layer: `TOOL_NAMES`, `createToolHandlers(backend)` (plain
  args in, plain JSON out), input guards (`requireString`; numbers/kinds are
  re-checked in the backend so both entry points are safe).
- `mcp/src/index.ts` — `createServer(backend)` registers the six tools with
  the official TS SDK and wraps handler values into MCP text content blocks.
  Two transports: stdio by default (`bun start`, for Claude Code / Claude
  Desktop), Streamable HTTP with `bun start:http`
  (`--http`, `--port=` or `PORT`, default 3001). HTTP is stateless, and
  stateless transports are single-use — each request gets a fresh
  transport + server over the one shared backend, so the op log persists
  across requests.
- `mcp/tests/mcp-client.test.ts` — the contract validation as a real test:
  a genuine MCP `Client` over a linked-pair transport lists the six tools,
  lists tracks, adds a clip, sees it in `project_get`, finds the `ClipAdded`
  op with actor `mcp`, and undoes it (clip gone, `UndoMarker` with actor
  `mcp`). Plus transport/param round-trip and rejection of bad clip input.

## 3. Extend it (rules)

- Never rename a tool or change its input shape — that's the frozen
  `contracts/mcp-tools.md`. A breaking change needs a new contract version
  plus a migration note, never silent drift.
- New behavior = new op kinds in `core/` + a new tool row in the contract,
  then a handler here. Additive only.
- The sibling `mcp/src/nl/` (NL commands) shares this package; its tests
  (`tests/nl.test.ts`) must keep passing — run the full `bun test`.

## 4. NL commands (agent 2): English in, ordinary ops out

New to this layer? The one big idea: **the NL layer never touches project
state — it compiles an utterance into 1+ ordinary op drafts**, and the
caller applies them with an `ai:<sidecar>` actor through the same op log
every other surface uses. An AI-added clip is a `ClipAdded` op like any
other, so undo, redo, autosave, and inspection work for free.

The pieces (`mcp/src/nl/`, all new this track, no frozen contract touched):

- `types.ts` — `OpDraft` (one op minus `seq`, which `op_apply` assigns —
  NL never mints seqs), `ClipDraft` (snake_case `Clip` payload mirroring
  `core/src/model.rs`), `NlPlan` (`summary` echo + `ops[]` + `warnings[]`).
  `OpKind` reuses exactly the six frozen kinds from
  `contracts/op-log-format.md` — NL invents no op kinds, ever.
  `nlActor("jam")` → `"ai:jam"`; `validNlActor` mirrors
  `engine::valid_actor` (non-empty after the `ai:` prefix).
- `parse.ts` — deterministic rule-based parser (the fallback, not the
  ceiling): add-clip (`add [audio|midi] clip called <name> on track <id>
  at beat N length N`, bars auto-convert ×4), `set tempo to N`,
  `set <node>:<param> to <v>`, `move clip <id> to beat N`. Payload shapes
  match what the engine parses: `ClipMoved` → `{"startBeats": N}`,
  `ParamSet` → bare number with `node:param` target, `TempoSet` → bare
  number. Gibberish throws `NlParseError` (surfaced as "say it
  differently", never as an op); unknown tracks produce a `warnings[]`
  entry and the op log rejects them with `UnknownTarget`.
- `sidecar.ts` — the sidecar pattern (models unpinned). `NlProvider` is
  the only seam a neural model touches: `LocalNlProvider` (default, runs
  the deterministic parser offline) vs `SidecarNlProvider` (POSTs
  `{ model, text, ctx }` to any HTTP sidecar — the model id is a plain
  string like `"qwen2.5:7b"`, nothing pins a version). Swapping models
  changes plans, never op shapes or UI. `compileNlCommand(text,
  provider?, ctx?)` is the entry point.
- `store.ts` — test-scale op-log harness mirroring the frozen semantics:
  monotonic `seq`, `undo` appends an `UndoMarker`, `redo` appends a
  `{"redo": true}` marker, live state replays non-undone ops (the same
  fold the Rust engine does). Rejects bad actors (only `ai:*`/`ui`/`mcp`)
  and unknown-track clips, like the engine's `UnknownTarget`.
- `mcp/tests/nl.test.ts` — the track validation: `"add a midi clip called
  Solo on track trk_music at beat 8 length 4"` compiles to one `ClipAdded`
  op, applies under actor `ai:jam`, undoes via `UndoMarker` (clip gone),
  redoes (clip back); plus tempo/param/move coverage, gibberish rejection,
  and unknown-track/bad-actor rejection.

Rules: NL adds no MCP tool (the six frozen tools are untouched — a seventh
tool would be a contract break needing a version + migration note). New
utterances = new parse rules emitting existing op kinds; new behavior =
new op kinds in `core/` first, then a parse rule.
