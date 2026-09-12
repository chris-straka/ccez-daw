/**
 * Track L (agent 2): NL commands layer — public entry point.
 *
 * NL compiles to ordinary undoable ops (sidecar pattern, models unpinned):
 * `compileNlCommand` turns an utterance into an `NlPlan` of `OpDraft`s;
 * the caller applies them with an `ai:<sidecar>` actor through `op_apply`
 * (production) or `NlOpStore` (tests). Nothing here invents an op kind —
 * every draft reuses a frozen `OpKind` from `contracts/op-log-format.md`.
 */
export type {
  ClipDraft,
  ClipKind,
  NlPlan,
  OpDraft,
  OpKind,
} from "./types.js";
export { nlActor, validNlActor, AI_ACTOR_PREFIX } from "./types.js";
export { parseNlCommand, NlParseError, type ParseContext } from "./parse.js";
export {
  LocalNlProvider,
  SidecarNlProvider,
  compileNlCommand,
  type NlProvider,
  type SidecarOptions,
} from "./sidecar.js";
export { NlOpStore, type OpRecord, type NlTestTrack } from "./store.js";
