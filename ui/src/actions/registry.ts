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
];

export function findAction(id: string): ActionDef | undefined {
  return ACTIONS.find((a) => a.id === id);
}

export type { Project };
