import type { Clip, Op } from "../generated/project";
import { checkOpBudget, validateScriptSource } from "./sandbox";
import {
  automationPointSetOp,
  clipAddedOp,
  clipMovedOp,
  paramSetOp,
  sanitizeScriptName,
  scriptActor,
  tempoSetOp,
  trackAddedOp,
  type OpRunner,
} from "./ops";

export type { OpRunner };

/** What a script body receives: the only host surface scripts can touch. */
export interface ScriptApi {
  readonly scriptName: string;
  readonly actor: string;
  addTrack(name: string): void;
  addClip(clip: Clip): void;
  moveClip(clipId: string, startBeats: number): void;
  setParam(node: string, param: string, value: number): void;
  setTempo(tempo: number): void;
  /**
   * Upsert one automation point (`AutomationPointSet` op through
   * `op_apply`). Pass `node`+`param` when the lane may not exist yet
   * (`laneExists: false` creates it); updates need just the lane id.
   * Session/comp/groove/branch/punch flows need no new script surface:
   * they commit ordinary clips via `addClip`/`moveClip` (comp, jam-record,
   * punch takes, grooved clips) or read state via the host.
   */
  setAutomationPoint(
    laneId: string,
    beat: number,
    value: number,
    node?: string,
    param?: string,
    laneExists?: boolean,
  ): void;
  readonly pendingOps: readonly Op[];
}

/** Result of running one script: the ops plus their assigned seq numbers. */
export interface ScriptResult {
  scriptName: string;
  actor: string;
  ops: Op[];
  seqs: number[];
}

function createCollectorApi(scriptName: string): ScriptApi & { ops: Op[] } {
  const actor = scriptActor(scriptName);
  const ops: Op[] = [];
  const push = (op: Op): void => {
    checkOpBudget(ops.length + 1);
    ops.push(op);
  };
  return {
    scriptName: sanitizeScriptName(scriptName),
    actor,
    addTrack: (name) => push(trackAddedOp(actor, name)),
    addClip: (clip) => push(clipAddedOp(actor, clip)),
    moveClip: (clipId, startBeats) => push(clipMovedOp(actor, clipId, startBeats)),
    setParam: (node, param, value) => push(paramSetOp(actor, node, param, value)),
    setTempo: (tempo) => push(tempoSetOp(actor, tempo)),
    setAutomationPoint: (laneId, beat, value, node?, param?, laneExists = false) =>
      push(automationPointSetOp(actor, laneId, beat, value, node, param, laneExists)),
    pendingOps: ops,
    ops,
  };
}

/** Compile a script source to its op list without applying anything. */
export async function compileScript(scriptName: string, source: string): Promise<Op[]> {
  validateScriptSource(source);
  const api = createCollectorApi(scriptName);
  // `new Function` scopes the body to exactly the api keys below: no
  // closure over host modules, no imports, no Tauri invoke. The body may
  // use top-level await (wrapped in an async function).
  const keys = ["addTrack", "addClip", "moveClip", "setParam", "setTempo", "setAutomationPoint"];
  const values = [
    api.addTrack,
    api.addClip,
    api.moveClip,
    api.setParam,
    api.setTempo,
    api.setAutomationPoint,
  ];
  const fn = new Function(...keys, `"use strict";\n${source}`);
  await (fn as (...a: unknown[]) => unknown)(...values);
  return [...api.ops];
}

/**
 * Run a script end to end: compile to ordinary ops, then apply each one
 * through the injected `op_apply` runner. Every op is undoable via
 * `op_undo` because scripts never write state any other way.
 */
export async function runScript(
  scriptName: string,
  source: string,
  runner: OpRunner,
): Promise<ScriptResult> {
  const ops = await compileScript(scriptName, source);
  const seqs: number[] = [];
  for (const op of ops) {
    seqs.push(await runner(op));
  }
  return { scriptName: sanitizeScriptName(scriptName), actor: scriptActor(scriptName), ops, seqs };
}

/**
 * Action-registry entry point: run a script through one named registry
 * action's `run` (kept for palette/MCP parity) is out of scope here;
 * scripts always funnel through `op_apply`-shaped runners instead.
 */
export function describeScriptApi(): string[] {
  return ["addTrack", "addClip", "moveClip", "setParam", "setTempo", "setAutomationPoint"];
}
