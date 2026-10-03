import * as ipc from "../generated/ipc";
import type { Project } from "../generated/project";

/**
 * v0 action registry (Track K owner).
 *
 * Every entry maps an action id — the same ids the command palette, vim
 * layer, scripting API, and MCP tools use — to either the IPC command it
 * invokes (`kind: "ipc"`) or a UI-local handler (`kind: "local"`, e.g.
 * `palette.open`, `vim.mode.*`, `vim.motion.*`). Local ids are listed in
 * `contracts/action-registry.md` as `— (local)`; they never touch `core/`,
 * so adding one is additive and needs no typegen run.
 */
export type ActionKind = "ipc" | "local";

export interface ActionDef {
  id: string;
  title: string;
  /** IPC command name for `ipc` actions; `"local"` for UI-only actions. */
  ipc: string;
  kind: ActionKind;
  run: (args: Record<string, unknown>) => Promise<unknown>;
}

function invokeIpc(name: string, args: Record<string, unknown>): Promise<unknown> {
  const fn = (ipc as unknown as Record<string, (a: unknown) => Promise<unknown>>)[name];
  if (!fn) throw new Error(`unknown IPC command: ${name}`);
  return fn(args);
}

type LocalHandler = (args: Record<string, unknown>) => Promise<unknown> | unknown;

const localHandlers = new Map<string, LocalHandler>();

/**
 * Override what a UI-local action does (the App shell wires real behavior;
 * headless tests wire recorders). Unregistered ids resolve to a descriptor
 * so key-sequence replay stays pure and total.
 */
export function registerLocalActionHandler(id: string, fn: LocalHandler): void {
  localHandlers.set(id, fn);
}

export function unregisterLocalActionHandler(id: string): void {
  localHandlers.delete(id);
}

function runLocal(id: string, args: Record<string, unknown>): Promise<unknown> {
  const fn = localHandlers.get(id);
  if (fn) return Promise.resolve(fn(args));
  return Promise.resolve({ local: id, args });
}

function ipcAction(id: string, title: string, command: string): ActionDef {
  return { id, title, ipc: command, kind: "ipc", run: (args) => invokeIpc(command, args) };
}

/**
 * Shape one `AutomationPointSet` op from palette/vim/scripting args:
 * `{ lane, beat, value, node?, param?, laneExists?, actor? }`. Mirrors
 * `ui/src/automation/edit.ts` `pointSetOp` without importing the view
 * layer: existing lanes carry just beat+value; lane creation (`laneExists:
 * false`, the default when `node`+`param` are given) also carries the
 * `node:param` address, per `contracts/op-log-format.md`.
 */
function buildAutomationPointOp(args: Record<string, unknown>): Record<string, unknown> {
  const lane = args.lane;
  if (typeof lane !== "string" || lane.length === 0) {
    throw new Error("automation.point_set needs a non-empty lane id");
  }
  const beat = args.beat;
  const value = args.value;
  if (typeof beat !== "number" || !Number.isFinite(beat) || beat < 0) {
    throw new Error(`automation.point_set beat ${String(beat)} must be finite and >= 0`);
  }
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`automation.point_set value ${String(value)} must be finite`);
  }
  const node = args.node;
  const param = args.param;
  const laneExists = args.laneExists ?? !(typeof node === "string" && typeof param === "string");
  const payload: Record<string, number | string> = { beat, value };
  if (!laneExists) {
    if (typeof node !== "string" || !node || typeof param !== "string" || !param) {
      throw new Error("automation.point_set needs node+param to create a lane");
    }
    payload.node = node;
    payload.param = param;
  } else if (typeof node === "string" && node && typeof param === "string" && param) {
    payload.node = node;
    payload.param = param;
  }
  const actor = typeof args.actor === "string" && args.actor ? args.actor : "ui";
  return { seq: 0, actor, kind: "AutomationPointSet", target: lane, value_json: JSON.stringify(payload) };
}

function localAction(id: string, title: string): ActionDef {
  return { id, title, ipc: "local", kind: "local", run: (args) => runLocal(id, args) };
}

export const ACTIONS: ActionDef[] = [
  // --- IPC-backed (frozen v0 surface) ---
  ipcAction("transport.play", "Transport: Play", "engine_play"),
  ipcAction("transport.stop", "Transport: Stop", "engine_stop"),
  ipcAction("project.new", "Project: New", "project_new"),
  ipcAction("project.get", "Project: Get", "project_get"),
  ipcAction("project.open", "Project: Open", "project_open"),
  ipcAction("project.save", "Project: Save", "project_save"),
  ipcAction("project.undo", "Project: Undo", "op_undo"),
  ipcAction("project.redo", "Project: Redo", "op_redo"),
  ipcAction("op.apply", "Op: Apply", "op_apply"),
  ipcAction("track.add", "Track: Add", "track_add"),
  // `track.list` has no dedicated IPC command: it reads `project_get`.
  ipcAction("track.list", "Track: List", "project_get"),
  ipcAction("clip.add", "Clip: Add", "clip_add"),
  ipcAction("param.set", "Param: Set", "param_set"),
  ipcAction("engine.set_tempo", "Engine: Set Tempo", "engine_set_tempo"),
  // --- UI-local (Track K; frozen as `— (local)`) ---
  localAction("palette.open", "Palette: Open"),
  localAction("vim.mode.normal", "Vim: Normal Mode"),
  localAction("vim.mode.insert", "Vim: Insert Mode"),
  localAction("vim.mode.visual", "Vim: Visual Mode"),
  localAction("vim.motion.left", "Vim: Move Left"),
  localAction("vim.motion.down", "Vim: Move Down"),
  localAction("vim.motion.up", "Vim: Move Up"),
  localAction("vim.motion.right", "Vim: Move Right"),
  localAction("vim.motion.wordForward", "Vim: Next Clip"),
  localAction("vim.motion.wordBack", "Vim: Previous Clip"),
  localAction("vim.motion.lineStart", "Vim: Line Start"),
  localAction("vim.motion.lineEnd", "Vim: Line End"),
  localAction("vim.motion.first", "Vim: First Track / Step"),
  localAction("vim.motion.last", "Vim: Last Track / Step"),
  // --- UI-local (shortcut hardening; additive, still `— (local)`) ---
  localAction("view.focusArrangement", "View: Focus Arrangement"),
  localAction("view.focusPianoRoll", "View: Focus Piano Roll"),
  localAction("view.focusMixer", "View: Focus Mixer"),
  localAction("view.focusSession", "View: Focus Session"),
  localAction("section.goto.chorus", "Section: Go To Chorus"),
  localAction("section.goto.verse", "Section: Go To Verse"),
  localAction("mixer.muteSelected", "Mixer: Mute Selected Strip"),
  localAction("mixer.soloSelected", "Mixer: Solo Selected Strip"),
  // --- IPC-backed (additive post-v0 coverage; still frozen IPC surface) ---
  // `automation.point_set` builds one `AutomationPointSet` op (the single
  // additive `OpKind` past the v0 freeze) and sends it through the frozen
  // `op_apply`, so it is undoable and survives restart like every other op.
  {
    id: "automation.point_set",
    title: "Automation: Set Point",
    ipc: "op_apply",
    kind: "ipc",
    run: (args) => invokeIpc("op_apply", { op: buildAutomationPointOp(args) }),
  },
  // --- UI-local (post-v0 feature coverage; additive, still `— (local)`) ---
  // Session launch/jam-record (`ui/src/timeline/launch.ts`), comp commit
  // (`ui/src/comp/comp.ts`), groove apply (`ui/src/groove/model.ts`),
  // branch merge (`ui/src/branch/branch.ts`), record punch
  // (`ui/src/record/record.ts`), and Link session join are pure
  // computations or ordinary-op builders over the frozen v0 model — no new
  // IPC — so they ride here as local actions. Unregistered handlers resolve
  // to descriptors (see `runLocal`); Track K wires real behavior.
  localAction("session.launch", "Session: Launch Slot / Scene"),
  localAction("session.jam_record", "Session: Record Jam to Timeline"),
  localAction("comp.commit", "Comp: Commit Composite Take"),
  localAction("groove.apply", "Groove: Apply Template to Clip"),
  localAction("branch.merge", "Branch: Merge Into Target"),
  localAction("record.punch", "Record: Punch In / Out"),
  localAction("link.join", "Link: Join Session"),
  // --- UI-local (native menu gaps; additive, still `— (local)`) ---
  // Dispatched by `src-tauri/src/menu.rs` via the `menu-action` event with
  // the same ids the palette uses, so menu and palette stay in sync.
  // Unregistered handlers resolve to descriptors (see `runLocal`); Track K
  // wires real behavior (dialogs, zoom, fullscreen, edit fallbacks).
  localAction("app.about", "App: About"),
  localAction("app.quit", "App: Quit"),
  localAction("app.preferences", "App: Preferences"),
  localAction("edit.cut", "Edit: Cut"),
  localAction("edit.copy", "Edit: Copy"),
  localAction("edit.paste", "Edit: Paste"),
  localAction("edit.select_all", "Edit: Select All"),
  localAction("view.zoom.in", "View: Zoom In"),
  localAction("view.zoom.out", "View: Zoom Out"),
  localAction("view.zoom.reset", "View: Reset Zoom"),
  localAction("view.fullscreen", "View: Toggle Fullscreen"),
  localAction("help.open_docs", "Help: Documentation"),
  localAction("help.show_shortcuts", "Help: Keyboard Shortcuts"),
  localAction("track.delete", "Track: Delete Selected"),
  localAction("clip.delete", "Clip: Delete"),
  localAction("clip.duplicate", "Clip: Duplicate"),
];

export function findAction(id: string): ActionDef | undefined {
  return ACTIONS.find((a) => a.id === id);
}

/**
 * Guidance for actions no key, palette row, or menu item can supply
 * arguments (or UI) for. Both dispatch paths consult it: the shell
 * registers these as handlers that surface the text, and the palette
 * shows it inline instead of closing on an inert descriptor — so nothing
 * resolves silently anywhere.
 */
export const LOCAL_GUIDANCE: Record<string, string> = {
  "clip.add": "Add Clip needs clip details — import a WAV on the Timeline or commit a take instead",
  "clip.duplicate": "Duplicate Clip needs a selected clip in the Timeline",
  "clip.delete": "Delete Clip needs a selected clip in the Timeline",
  "track.delete": "Delete Track needs a selected track in the Timeline",
  "automation.point_set": "Set Automation Point needs a lane — click one in the Automation tab",
  "app.about": "A game-music DAW (native About panel not built yet)",
  "app.preferences": "No preferences panel yet — keybindings live under ? (Shortcuts editor)",
  "help.open_docs": "Docs live in docs/ of the repo checkout (no viewer wired yet)",
  "view.zoom.in": "Timeline zoom is not built yet",
  "view.zoom.out": "Timeline zoom is not built yet",
  "view.zoom.reset": "Timeline zoom is not built yet",
};

export type { Project };
