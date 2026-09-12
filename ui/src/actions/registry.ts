import * as ipc from "../generated/ipc";
import type { Project } from "../generated/project";

/**
 * v0 action registry stub. Every entry maps an action id (the same ids the
 * command palette, vim layer, scripting API, and MCP tools will use) to the
 * IPC command it invokes. Track K mirrors this registry; do not fork it.
 */
export interface ActionDef {
  id: string;
  title: string;
  ipc: string;
  run: (args: Record<string, unknown>) => Promise<unknown>;
}

function invokeStub(name: string, args: Record<string, unknown>): Promise<unknown> {
  const fn = (ipc as unknown as Record<string, (a: unknown) => Promise<unknown>>)[name];
  if (!fn) throw new Error(`unknown IPC command: ${name}`);
  return fn(args);
}

function action(id: string, title: string, ipc: string): ActionDef {
  return { id, title, ipc, run: (args) => invokeStub(ipc, args) };
}

export const ACTIONS: ActionDef[] = [
  action("transport.play", "Transport: Play", "engine_play"),
  action("transport.stop", "Transport: Stop", "engine_stop"),
  action("project.new", "Project: New", "project_new"),
  action("project.open", "Project: Open", "project_open"),
  action("project.save", "Project: Save", "project_save"),
  action("project.undo", "Project: Undo", "op_undo"),
  action("project.redo", "Project: Redo", "op_redo"),
  action("track.add", "Track: Add", "track_add"),
  action("clip.add", "Clip: Add", "clip_add"),
  action("param.set", "Param: Set", "param_set"),
];

export function findAction(id: string): ActionDef | undefined {
  return ACTIONS.find((a) => a.id === id);
}

export type { Project };
