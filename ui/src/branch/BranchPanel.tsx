import { For, Show, createMemo, createSignal } from "solid-js";
import type { Op, OpKind, Project } from "../generated/project";
import { op_apply } from "../tauri/commands";
import {
  BranchStore,
  classifyOp,
  describeOp,
  mergeCommitOps,
} from "./branch";

const STAGE_KINDS: OpKind[] = ["ParamSet", "TempoSet", "ClipMoved", "AutomationPointSet"];

function kindHint(kind: OpKind): string {
  switch (kind) {
    case "ParamSet":
      return "node:param = value, e.g. trk_music:volume / 0.9";
    case "TempoSet":
      return "transport = BPM, e.g. transport / 128";
    case "ClipMoved":
      return "clip id = position, e.g. clip_b / {\"startBeats\": 8}";
    case "AutomationPointSet":
      return "lane id = point, e.g. lane_vol / {\"beat\": 4, \"value\": 0.5}";
    default:
      return "";
  }
}

/**
 * Branch panel: list branches/snapshots, compare two branches at op
 * granularity (added/moved/retargeted/tempo/automation/marker rows),
 * preview merges as a conflict list, and commit merges through the existing
 * `op_apply` surface — no new IPC, no schema change. Pass `applyOp` to
 * override the transport (tests).
 *
 * Branches are panel-local planning state over the frozen op model (the
 * `core/src/branch.rs` semantics mirrored in `branch.ts`): stage ops onto a
 * branch, compare, preview, then commit the clean ops as ordinary engine
 * ops. Conflicting ops are left out for a human to resolve by hand.
 */
export default function BranchPanel(props: {
  project: Project | null;
  applyOp?: (op: Op) => Promise<unknown>;
  store?: BranchStore;
}) {
  const [store] = createSignal(props.store ?? new BranchStore());
  const [version, setVersion] = createSignal(0);
  const touch = () => setVersion((v) => v + 1);

  const [branchA, setBranchA] = createSignal("main");
  const [branchB, setBranchB] = createSignal("main");
  const [newName, setNewName] = createSignal("");
  const [forkFrom, setForkFrom] = createSignal("main");
  const [stageBranch, setStageBranch] = createSignal("main");
  const [stageKind, setStageKind] = createSignal<OpKind>("ParamSet");
  const [stageTarget, setStageTarget] = createSignal("");
  const [stageValue, setStageValue] = createSignal("");
  const [snapName, setSnapName] = createSignal("");
  const [status, setStatus] = createSignal("branches: compare, preview, then merge");

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  const names = createMemo(() => {
    version();
    return store().branchNames();
  });
  const snaps = createMemo(() => {
    version();
    return store().snapshotNames();
  });

  function safeA(): string {
    const n = names();
    return n.includes(branchA()) ? branchA() : (n[0] ?? "main");
  }
  function safeB(): string {
    const n = names();
    return n.includes(branchB()) ? branchB() : (n[0] ?? "main");
  }

  const diff = createMemo(() => {
    version();
    try {
      return store().compare(safeA(), safeB());
    } catch {
      return null;
    }
  });
  const preview = createMemo(() => {
    version();
    try {
      return store().mergePreview(safeA(), safeB());
    } catch {
      return null;
    }
  });

  function createBranch(): void {
    const name = newName().trim();
    if (!name) {
      setStatus("branch needs a name");
      return;
    }
    try {
      store().createBranch(name, forkFrom());
      setNewName("");
      setBranchA(name);
      setStatus(`forked ${name} from ${forkFrom()} @ ${store().tipSeq(forkFrom())}`);
      touch();
    } catch (e) {
      setStatus(`branch refused: ${String(e)}`);
    }
  }

  function removeBranch(name: string): void {
    try {
      store().deleteBranch(name);
      setStatus(`deleted branch ${name}`);
      touch();
    } catch (e) {
      setStatus(`delete refused: ${String(e)}`);
    }
  }

  function stageOp(): void {
    const target = stageTarget().trim();
    if (!target) {
      setStatus("stage needs a target");
      return;
    }
    try {
      const op = store().stage(
        stageBranch(),
        "ui",
        stageKind(),
        target,
        stageValue().trim() || "0",
      );
      setStageTarget("");
      setStatus(`staged ${describeOp(op)} on ${stageBranch()}`);
      touch();
    } catch (e) {
      setStatus(`stage refused: ${String(e)}`);
    }
  }

  async function commitMerge(): Promise<void> {
    const out = preview();
    if (!out) return;
    if (out.merged.length === 0 && out.conflicts.length === 0) {
      setStatus("nothing to merge: branches agree");
      return;
    }
    try {
      const ops = mergeCommitOps(out);
      let n = 0;
      for (const op of ops) {
        await apply(op);
        n += 1;
      }
      // Record what the engine accepted with fresh local seqs.
      store().mergeApply(safeA(), safeB());
      setStatus(
        `merged ${n} op${n === 1 ? "" : "s"} ${safeA()} → ${safeB()}` +
          (out.conflicts.length > 0 ? `, ${out.conflicts.length} conflict${out.conflicts.length === 1 ? "" : "s"} left out` : ""),
      );
      touch();
    } catch (e) {
      setStatus(`merge commit failed: ${String(e)}`);
    }
  }

  function capture(): void {
    const project = props.project;
    if (!project) return;
    const name = snapName().trim() || `snap-${snaps().length + 1}`;
    try {
      const mark = store().captureSnapshot(name, project);
      setSnapName("");
      setStatus(`snapshot ${mark.name} @ ${mark.seq} (${mark.summary})`);
      touch();
    } catch (e) {
      setStatus(`snapshot refused: ${String(e)}`);
    }
  }

  function removeSnapshot(name: string): void {
    try {
      store().deleteSnapshot(name);
      setStatus(`deleted snapshot ${name}`);
      touch();
    } catch (e) {
      setStatus(`delete refused: ${String(e)}`);
    }
  }

  return (
    <div data-testid="branch-panel" class="session-view">
      <Show
        when={props.project}
        fallback={<div class="session-dim">No project open.</div>}
      >
        <div>
          <div class="view-label">Branches</div>
          <div class="session-controls">
            <input
              data-testid="branch-new-name"
              value={newName()}
              onInput={(e) => setNewName(e.currentTarget.value)}
              placeholder="new branch"
              style={{ width: "120px" }}
            />
            <label>
              from{" "}
              <select
                data-testid="branch-fork-from"
                value={forkFrom()}
                onInput={(e) => setForkFrom(e.currentTarget.value)}
              >
                <For each={names()}>{(n) => <option value={n}>{n}</option>}</For>
              </select>
            </label>
            <button data-testid="branch-create" onClick={createBranch}>
              Fork branch
            </button>
          </div>
          <For each={names()}>
            {(n) => (
              <div class="lane">
                <span class="lane-name daw-numeric">
                  @{(() => {
                    version();
                    try {
                      return store().tipSeq(n);
                    } catch {
                      return 0;
                    }
                  })()}
                </span>
                <span class="clip-chip clip-midi" title={`branch ${n}`}>
                  {n}
                </span>
                <button
                  data-testid={`branch-delete-${n}`}
                  onClick={() => removeBranch(n)}
                  class="slot-btn silent"
                >
                  delete
                </button>
              </div>
            )}
          </For>
        </div>

        <div>
          <div class="view-label">Stage op onto branch</div>
          <div class="session-controls">
            <select
              data-testid="branch-stage-target-branch"
              value={stageBranch()}
              onInput={(e) => setStageBranch(e.currentTarget.value)}
            >
              <For each={names()}>{(n) => <option value={n}>{n}</option>}</For>
            </select>
            <select
              data-testid="branch-stage-kind"
              value={stageKind()}
              onInput={(e) => setStageKind(e.currentTarget.value as OpKind)}
            >
              <For each={STAGE_KINDS}>{(k) => <option value={k}>{k}</option>}</For>
            </select>
            <input
              data-testid="branch-stage-target"
              value={stageTarget()}
              onInput={(e) => setStageTarget(e.currentTarget.value)}
              placeholder="target"
              style={{ width: "140px" }}
            />
            <input
              data-testid="branch-stage-value"
              value={stageValue()}
              onInput={(e) => setStageValue(e.currentTarget.value)}
              placeholder="value_json"
              style={{ width: "120px" }}
            />
            <button data-testid="branch-stage" onClick={stageOp}>
              Stage
            </button>
          </div>
          <div class="session-dim">{kindHint(stageKind())}</div>
        </div>

        <div>
          <div class="view-label">Compare (op granularity)</div>
          <div class="session-controls">
            <select
              data-testid="branch-compare-a"
              value={safeA()}
              onInput={(e) => setBranchA(e.currentTarget.value)}
            >
              <For each={names()}>{(n) => <option value={n}>{n}</option>}</For>
            </select>
            <span class="session-dim">vs</span>
            <select
              data-testid="branch-compare-b"
              value={safeB()}
              onInput={(e) => setBranchB(e.currentTarget.value)}
            >
              <For each={names()}>{(n) => <option value={n}>{n}</option>}</For>
            </select>
          </div>
          <Show
            when={diff()}
            fallback={<div class="session-dim">Pick two branches.</div>}
          >
            <div class="view-label daw-numeric">
              only in {safeA()} ({diff()!.onlyInA.length}) · only in {safeB()} (
              {diff()!.onlyInB.length})
            </div>
            <For each={diff()!.onlyInA}>
              {(op) => (
                <div class="lane">
                  <span class="lane-name">{classifyOp(op)}</span>
                  <span class="clip-chip clip-audio" title={`only in ${safeA()}`}>
                    {describeOp(op)}
                  </span>
                </div>
              )}
            </For>
            <For each={diff()!.onlyInB}>
              {(op) => (
                <div class="lane">
                  <span class="lane-name">{classifyOp(op)}</span>
                  <span class="clip-chip clip-midi" title={`only in ${safeB()}`}>
                    {describeOp(op)}
                  </span>
                </div>
              )}
            </For>
          </Show>
        </div>

        <div>
          <div class="view-label">Merge preview {safeA()} → {safeB()}</div>
          <Show when={preview()}>
            <div class="session-dim daw-numeric" data-testid="branch-preview-counts">
              {preview()!.merged.length} clean · {preview()!.conflicts.length}{" "}
              conflicts
            </div>
            <For each={preview()!.merged}>
              {(op) => (
                <div class="lane">
                  <span class="lane-name">{classifyOp(op)}</span>
                  <span class="clip-chip clip-audio">{describeOp(op)}</span>
                </div>
              )}
            </For>
            <For each={preview()!.conflicts}>
              {(c) => (
                <div class="lane" data-testid={`branch-conflict-${c.target}`}>
                  <span class="lane-name">conflict</span>
                  <span
                    class="clip-chip clip-midi"
                    title={`source: ${describeOp(c.sourceOp)} / target: ${describeOp(c.targetOp)}`}
                  >
                    {c.target}: {c.sourceOp.value_json} vs {c.targetOp.value_json}
                  </span>
                </div>
              )}
            </For>
            <div class="session-controls">
              <button data-testid="branch-merge-commit" onClick={() => void commitMerge()}>
                Merge commit
              </button>
            </div>
          </Show>
        </div>

        <div>
          <div class="view-label">Snapshots</div>
          <div class="session-controls">
            <input
              data-testid="branch-snap-name"
              value={snapName()}
              onInput={(e) => setSnapName(e.currentTarget.value)}
              placeholder="snapshot name"
              style={{ width: "120px" }}
            />
            <button data-testid="branch-snap-capture" onClick={capture}>
              Capture project
            </button>
          </div>
          <Show
            when={snaps().length > 0}
            fallback={<div class="session-dim">No snapshots yet.</div>}
          >
            <For each={snaps()}>
              {(n) => {
                version();
                const s = (() => {
                  try {
                    return store().snapshot(n);
                  } catch {
                    return null;
                  }
                })();
                return (
                  <div class="lane">
                    <span class="lane-name daw-numeric">@{s?.seq ?? 0}</span>
                    <span class="clip-chip clip-audio" title={s?.takenAt ?? ""}>
                      {n} — {s?.summary ?? ""}
                    </span>
                    <button
                      data-testid={`branch-snap-delete-${n}`}
                      onClick={() => removeSnapshot(n)}
                      class="slot-btn silent"
                    >
                      delete
                    </button>
                  </div>
                );
              }}
            </For>
          </Show>
        </div>

        <span data-testid="branch-status" class="session-dim">
          {status()}
        </span>
      </Show>
    </div>
  );
}
