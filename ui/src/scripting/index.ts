/** TS scripting API over the action registry (Track K). */
export { compileScript, runScript, describeScriptApi } from "./api";
export type { OpRunner, ScriptApi, ScriptResult } from "./api";
export {
  SCRIPT_ACTOR_PREFIX,
  clipAddedOp,
  clipMovedOp,
  paramSetOp,
  sanitizeScriptName,
  scriptActor,
  tempoSetOp,
  trackAddedOp,
} from "./ops";
export {
  MAX_OPS_PER_SCRIPT,
  MAX_SCRIPT_SOURCE_CHARS,
  ScriptSandboxError,
  checkOpBudget,
  validateScriptSource,
} from "./sandbox";
