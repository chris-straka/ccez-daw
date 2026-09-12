/**
 * Track L (agent 2): minimal in-memory op-log harness for NL tests.
 *
 * Mirrors the frozen semantics in `contracts/op-log-format.md` at test
 * scale: `apply` appends one op with a monotonic `seq`, `undo` appends an
 * `UndoMarker` naming the undone seq, `redo` appends a `{"redo": true}`
 * marker cancelling it. State (`clips`) replays the live (non-undone,
 * non-marker) ops — the same fold the Rust engine does (`fold_undone`).
 */
import type { ClipDraft, OpDraft, OpKind } from "./types.js";
import { validNlActor } from "./types.js";

export interface OpRecord extends OpDraft {
  seq: number;
  actor: string;
}

export interface NlTestTrack {
  id: string;
  name: string;
}

function isRedoMarker(op: OpRecord): boolean {
  try {
    const v = JSON.parse(op.valueJson) as unknown;
    return (
      typeof v === "object" &&
      v !== null &&
      (v as { redo?: unknown }).redo === true
    );
  } catch {
    return false;
  }
}

export class NlOpStore {
  private log: OpRecord[] = [];
  private nextSeq = 1;
  readonly tracks: NlTestTrack[];

  constructor(tracks: NlTestTrack[] = [{ id: "trk_music", name: "Music" }]) {
    this.tracks = [...tracks];
  }

  /** Append one op; `seq` assigned here, never by the caller. */
  apply(actor: string, op: OpDraft): number {
    if (!validNlActor(actor) && actor !== "ui" && actor !== "mcp") {
      throw new Error(`bad actor \`${actor}\` (want ai:<sidecar>)`);
    }
    if (op.kind === "ClipAdded") {
      const clip = JSON.parse(op.valueJson) as ClipDraft;
      if (!this.tracks.some((t) => t.id === clip.track_id)) {
        throw new Error(`unknown op target \`${clip.track_id}\``);
      }
      if (this.liveClips().some((c) => c.id === clip.id)) {
        throw new Error(`duplicate op target \`${clip.id}\``);
      }
    }
    const seq = this.nextSeq++;
    this.log.push({ ...op, seq, actor });
    return seq;
  }

  applyPlan(
    actor: string,
    plan: { ops: OpDraft[] },
  ): number[] {
    return plan.ops.map((op) => this.apply(actor, op));
  }

  private undoneSeqs(): Set<number> {
    const undone = new Set<number>();
    for (const op of this.log) {
      if (op.kind !== "UndoMarker") continue;
      const seq = Number(op.target);
      if (!Number.isInteger(seq)) continue;
      if (isRedoMarker(op)) undone.delete(seq);
      else undone.add(seq);
    }
    return undone;
  }

  /** Undo the last live (non-marker) op; returns the undone seq. */
  undo(actor = "ui"): number {
    const undone = this.undoneSeqs();
    const last = [...this.log]
      .reverse()
      .find((op) => op.kind !== "UndoMarker" && !undone.has(op.seq));
    if (!last) throw new Error("nothing to undo");
    const seq = this.nextSeq++;
    this.log.push({
      kind: "UndoMarker",
      target: String(last.seq),
      valueJson: "{}",
      seq,
      actor,
    });
    return last.seq;
  }

  /** Redo the last undone op; returns the marker seq. */
  redo(actor = "ui"): number {
    const undone = this.undoneSeqs();
    const lastUndone = [...this.log]
      .reverse()
      .find((op) => op.kind === "UndoMarker" && !isRedoMarker(op));
    if (!lastUndone || !undone.has(Number(lastUndone.target))) {
      throw new Error("nothing to redo");
    }
    const seq = this.nextSeq++;
    this.log.push({
      kind: "UndoMarker",
      target: lastUndone.target,
      valueJson: JSON.stringify({ redo: true }),
      seq,
      actor,
    });
    return seq;
  }

  liveOps(): OpRecord[] {
    const undone = this.undoneSeqs();
    return this.log.filter(
      (op) => op.kind !== "UndoMarker" && !undone.has(op.seq),
    );
  }

  clips(): ClipDraft[] {
    return this.liveClips();
  }

  private liveClips(): ClipDraft[] {
    const out: ClipDraft[] = [];
    for (const op of this.liveOps()) {
      if (op.kind === "ClipAdded") out.push(JSON.parse(op.valueJson));
    }
    return out;
  }

  ops(): readonly OpRecord[] {
    return this.log;
  }

  opKinds(): OpKind[] {
    return this.log.map((op) => op.kind);
  }
}
