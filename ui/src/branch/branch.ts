import { OpSchema, type Op, type OpKind, type Project } from "../generated/project";

/**
 * Branch workflow model: list branches/snapshots, compare two branches at
 * op granularity, preview merges (conflict list), and commit merges through
 * existing ops only.
 *
 * Exact TypeScript mirror of `core/src/branch.rs` semantics: a branch is
 * "shared prefix, then my extra ops"; compare is set difference on `seq`;
 * merge is three-way diff against the fork point with same-`target` edits
 * of different effect surfacing as conflicts. `ui/tests/branch.test.ts`
 * pins the same scenarios as the Rust `branch_*` tests, so drift on either
 * side shows up as red.
 *
 * Like `comp.ts`, this module adds no IPC or project-schema surface: merges
 * commit as ordinary ops (seq 0 placeholder, the engine assigns the real
 * sequence) through the frozen `op_apply` command.
 */

export const MAIN_BRANCH = "main";

export type OpCategory =
  | "added"
  | "moved"
  | "retargeted"
  | "tempo"
  | "automation"
  | "marker";

/** Total over OpKind: mirror of `classify_op` in `core/src/branch.rs`. */
export function classifyOp(op: Op): OpCategory {
  switch (op.kind) {
    case "TrackAdded":
    case "ClipAdded":
      return "added";
    case "ClipMoved":
      return "moved";
    case "ParamSet":
      return "retargeted";
    case "TempoSet":
      return "tempo";
    case "AutomationPointSet":
      return "automation";
    case "UndoMarker":
      return "marker";
  }
}

/** One-line row text. Mirror of `describe_op`: `#<seq> <Kind> <target> <value>`. */
export function describeOp(op: Op): string {
  return `#${op.seq} ${op.kind} ${op.target} ${op.value_json}`;
}

export interface Branch {
  name: string;
  parent: string | null;
  baseSeq: number;
  ops: Op[];
}

export interface BranchDiff {
  onlyInA: Op[];
  onlyInB: Op[];
}

export interface MergeConflict {
  target: string;
  sourceOp: Op;
  targetOp: Op;
}

export interface MergeOutcome {
  merged: Op[];
  conflicts: MergeConflict[];
}

export class BranchError extends Error {
  readonly code: "not-found" | "already-exists" | "cannot-delete-main" | "cycle";
  constructor(code: BranchError["code"], message: string) {
    super(message);
    this.code = code;
  }
}

/** Named project bookmark: full project at the panel's known tip `seq`. */
export interface SnapshotBookmark {
  name: string;
  seq: number;
  takenAt: string;
  summary: string;
  project: Project;
}

export function summarizeProject(project: Project): string {
  return `${project.tracks.length} tracks, ${project.clips.length} clips @ ${project.tempo} BPM`;
}

/** In-memory branch store. Mirror of Rust `BranchStore` (planning side). */
export class BranchStore {
  private branches = new Map<string, Branch>();
  private snapshots = new Map<string, SnapshotBookmark>();
  private nextSeq = 1;

  constructor() {
    this.branches.set(MAIN_BRANCH, {
      name: MAIN_BRANCH,
      parent: null,
      baseSeq: 0,
      ops: [],
    });
  }

  branchNames(): string[] {
    return [...this.branches.keys()].sort();
  }

  get(name: string): Branch {
    const b = this.branches.get(name);
    if (!b) throw new BranchError("not-found", `no such branch: ${name}`);
    return b;
  }

  tipSeq(name: string): number {
    const h = this.history(name);
    return h.length === 0 ? 0 : h[h.length - 1].seq;
  }

  /** Fork `name` from the current tip of `from`. */
  createBranch(name: string, from: string): void {
    if (this.branches.has(name)) {
      throw new BranchError("already-exists", `branch exists: ${name}`);
    }
    const baseSeq = this.tipSeq(from);
    // tipSeq throws not-found for a missing `from` via history().
    this.branches.set(name, { name, parent: from, baseSeq, ops: [] });
  }

  deleteBranch(name: string): void {
    if (name === MAIN_BRANCH) {
      throw new BranchError("cannot-delete-main", "cannot delete branch `main`");
    }
    if (!this.branches.delete(name)) {
      throw new BranchError("not-found", `no such branch: ${name}`);
    }
  }

  /** Full history: shared prefix (ops at/below each fork point), then own ops. */
  history(name: string): Op[] {
    const branch = this.get(name);
    const chain: Branch[] = [branch];
    const seen = new Set<string>([branch.name]);
    while (true) {
      const parentName = chain[chain.length - 1].parent;
      if (parentName === null) break;
      if (seen.has(parentName)) {
        throw new BranchError("cycle", `branch cycle via: ${parentName}`);
      }
      seen.add(parentName);
      chain.push(this.get(parentName));
      if (chain.length > this.branches.size + 1) {
        throw new BranchError("cycle", `branch cycle via: ${parentName}`);
      }
    }
    const out: Op[] = [];
    for (let i = 1; i < chain.length; i++) {
      const ceiling = chain[i - 1].baseSeq;
      for (const op of chain[i].ops) {
        if (op.seq <= ceiling) out.push(op);
      }
    }
    out.push(...branch.ops);
    out.sort((a, b) => a.seq - b.seq);
    return out.filter((op, i, arr) => i === 0 || arr[i - 1].seq !== op.seq);
  }

  /** Stage one op onto `branch`, assigning the next local `seq`. */
  stage(branch: string, actor: string, kind: OpKind, target: string, valueJson: string): Op {
    const b = this.get(branch);
    const op = OpSchema.parse({
      seq: this.nextSeq,
      actor,
      kind,
      target,
      value_json: valueJson,
    });
    this.nextSeq += 1;
    b.ops.push(op);
    return op;
  }

  /** Record already-committed ops onto `target` with fresh local seqs. */
  recordOps(target: string, ops: Op[]): Op[] {
    const b = this.get(target);
    return ops.map((op) => {
      const fresh = OpSchema.parse({ ...op, seq: this.nextSeq });
      this.nextSeq += 1;
      b.ops.push(fresh);
      return fresh;
    });
  }

  /** A/B compare at op granularity: ops reachable from one side only. */
  compare(a: string, b: string): BranchDiff {
    const ha = this.history(a);
    const hb = this.history(b);
    const seqsB = new Set(hb.map((op) => op.seq));
    const seqsA = new Set(ha.map((op) => op.seq));
    return {
      onlyInA: ha.filter((op) => !seqsB.has(op.seq)),
      onlyInB: hb.filter((op) => !seqsA.has(op.seq)),
    };
  }

  /**
   * Three-way merge preview of `source` into `target`. Each new source op
   * merges cleanly unless `target` added an op for the same `target` with a
   * different effect; identical same-target edits are idempotent, skipped.
   */
  mergePreview(source: string, target: string): MergeOutcome {
    const diff = this.compare(source, target);
    const byTarget = new Map<string, Op>();
    for (const op of diff.onlyInB) byTarget.set(op.target, op);
    const outcome: MergeOutcome = { merged: [], conflicts: [] };
    for (const op of diff.onlyInA) {
      const other = byTarget.get(op.target);
      if (!other) outcome.merged.push(op);
      else if (other.kind === op.kind && other.value_json === op.value_json) {
        // Same edit on both sides: already in effect, skip.
      } else {
        outcome.conflicts.push({ target: op.target, sourceOp: op, targetOp: other });
      }
    }
    return outcome;
  }

  /**
   * Record a merge: append the clean ops to `target` with fresh local seqs.
   * Conflicting ops are left out for a human. Returns the preview with
   * re-sequenced `merged` ops.
   */
  mergeApply(source: string, target: string): MergeOutcome {
    const preview = this.mergePreview(source, target);
    const resequenced = this.recordOps(target, preview.merged);
    return { merged: resequenced, conflicts: preview.conflicts };
  }

  // ---- snapshots (named project bookmarks bounding compare context) ----

  captureSnapshot(name: string, project: Project, seq?: number): SnapshotBookmark {
    if (this.snapshots.has(name)) {
      throw new BranchError("already-exists", `snapshot exists: ${name}`);
    }
    const tip = seq ?? this.tipSeq(MAIN_BRANCH);
    const mark: SnapshotBookmark = {
      name,
      seq: tip,
      takenAt: new Date().toISOString(),
      summary: summarizeProject(project),
      project: JSON.parse(JSON.stringify(project)) as Project,
    };
    this.snapshots.set(name, mark);
    return mark;
  }

  snapshotNames(): string[] {
    return [...this.snapshots.keys()].sort();
  }

  snapshot(name: string): SnapshotBookmark {
    const s = this.snapshots.get(name);
    if (!s) throw new BranchError("not-found", `no such snapshot: ${name}`);
    return s;
  }

  deleteSnapshot(name: string): void {
    if (!this.snapshots.delete(name)) {
      throw new BranchError("not-found", `no such snapshot: ${name}`);
    }
  }
}

/**
 * Commit a merge through the frozen `op_apply` surface: `seq: 0` is a
 * placeholder the engine replaces (the `compCommitOp` precedent). Returns
 * one transport-ready op per clean merged op; conflicts never ship.
 */
export function mergeCommitOps(outcome: MergeOutcome): Op[] {
  return outcome.merged.map((op) =>
    OpSchema.parse({ ...op, seq: 0 }),
  );
}
