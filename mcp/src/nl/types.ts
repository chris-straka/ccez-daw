/**
 * Track L (agent 2): NL command types.
 *
 * NL never writes audio or project state directly. It compiles to ordinary
 * op-log drafts (`OpDraft`) applied with an `ai:<sidecar>` actor, so every
 * NL result is undoable by design (`contracts/op-log-format.md`).
 * Field names mirror `Clip` / `Op` in `core/src/model.rs` so the three
 * surfaces (palette / vim / MCP) cannot diverge.
 */

export type OpKind =
  | "TrackAdded"
  | "ClipAdded"
  | "ClipMoved"
  | "ParamSet"
  | "TempoSet"
  | "UndoMarker";

export type ClipKind = "Audio" | "Midi";

/** One op-log entry minus `seq` (assigned by `op_apply`, never by NL). */
export interface OpDraft {
  kind: OpKind;
  /** Clip id, track id, `node:param`, or undone seq for markers. */
  target: string;
  /** JSON payload, e.g. a `Clip` JSON string or `"0.5"`. */
  valueJson: string;
}

/** Minimal `Clip` payload carried inside a `ClipAdded` draft. */
export interface ClipDraft {
  id: string;
  track_id: string;
  name: string;
  start_beats: number;
  length_beats: number;
  kind: ClipKind;
  source: string;
}

/** Result of compiling one NL utterance: 1+ ordinary ops. */
export interface NlPlan {
  /** Human-readable echo, e.g. "Add Midi clip 'Solo' at beat 8". */
  summary: string;
  ops: OpDraft[];
  warnings: string[];
}

export const AI_ACTOR_PREFIX = "ai:";

export function nlActor(sidecar: string): string {
  const name = sidecar.trim();
  if (name.length === 0) throw new Error("NL sidecar name must be non-empty");
  return `${AI_ACTOR_PREFIX}${name}`;
}

/** Frozen actor rule mirror (`engine::valid_actor`): NL must use `ai:<x>`. */
export function validNlActor(actor: string): boolean {
  const rest = actor.startsWith(AI_ACTOR_PREFIX)
    ? actor.slice(AI_ACTOR_PREFIX.length)
    : "";
  return rest.length > 0;
}
