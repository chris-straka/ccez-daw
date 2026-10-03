import { For, Show, createEffect, createSignal } from "solid-js";
import type { Clip, Op } from "../generated/project";
import { op_apply } from "../tauri/commands";
import type { MidiClip } from "../pianoroll/model";
import { samplePhrase } from "../repair/model";
import {
  GroovePool,
  applyGroove,
  extractGroove,
  grooveCommitOp,
  previewGroove,
  type ApplyParams,
  type GrooveTemplate,
} from "./model";

const STEP_CHOICES = [1, 2, 4];

/**
 * Groove pool panel: extract timing/velocity feel from any MIDI clip into a
 * named pool, then preview + apply it to another clip.
 *
 * Clips are edited as headless `MidiClip` values (the panel never touches
 * IPC itself): extraction previews on the source clip (demo phrase until
 * the asset loader lands — the same seam as the repair panel), and applying
 * reports the grooved clip through `onEdit` plus one ordinary frozen
 * `ClipAdded` op through `op_apply` (undoable via `op_undo`, no new IPC).
 * Pass `applyOp` to override the transport (tests) and `pool` to share a
 * library between mounts.
 */
export default function GroovePanel(props: {
  source: MidiClip | null;
  sourceName?: string;
  target: MidiClip | null;
  targetName?: string;
  targetClip?: Clip;
  pool?: GroovePool;
  applyOp?: (op: Op) => Promise<unknown>;
  onEdit?: (clip: MidiClip) => void;
  /** Bump to apply the current template from the global `groove.apply` action. */
  applyNonce?: number;
}) {
  const [pool] = createSignal(props.pool ?? new GroovePool());
  const [version, setVersion] = createSignal(0);
  const touch = () => setVersion((v) => v + 1);

  const [grooveName, setGrooveName] = createSignal("");
  const [steps, setSteps] = createSignal(4);
  const [selected, setSelected] = createSignal("");
  const [params, setParams] = createSignal<ApplyParams>({ quantize: 0, timing: 0.75, velocity: 0.5 });
  const [undo, setUndo] = createSignal<MidiClip[]>([]);
  const [message, setMessage] = createSignal("extract a feel, then preview it on the target");

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  const sourceClip = (): MidiClip => props.source ?? samplePhrase();
  const names = (): string[] => {
    version();
    return pool().names();
  };

  function describeOffsets(template: { offsets: number[] }): string {
    return template.offsets.map((o) => (o >= 0 ? "+" : "") + o.toFixed(3)).join(" ");
  }

  function extract(): void {
    const name = grooveName().trim();
    if (!name) {
      setMessage("groove needs a name");
      return;
    }
    try {
      const template = extractGroove(sourceClip(), steps());
      pool().set(name, template);
      setSelected(name);
      setGrooveName("");
      setMessage(
        `kept “${name}” (${steps()} steps/beats): ${describeOffsets(template)}`,
      );
      touch();
    } catch (e) {
      setMessage(`extract refused: ${(e as Error).message}`);
    }
  }

  function currentTemplate(): { name: string; template: GrooveTemplate } | null {
    const name = selected() || pool().names()[0] || "";
    if (!name) return null;
    const template = pool().get(name);
    return template ? { name, template } : null;
  }

  function preview(): void {
    const found = currentTemplate();
    if (!found) {
      setMessage("pool is empty: extract a groove first");
      return;
    }
    if (!props.target) {
      setMessage("no target clip: open a MIDI clip to preview on");
      return;
    }
    try {
      const p = previewGroove(props.target, found.template, params());
      setMessage(
        `preview “${found.name}”: ${p.moved}/${p.clip.notes.length} notes move, ` +
          `mean |Δt| ${p.meanAbsTimingDelta.toFixed(4)} beats, ` +
          `mean |Δv| ${p.meanAbsVelocityDelta.toFixed(1)}`,
      );
    } catch (e) {
      setMessage(`preview refused: ${(e as Error).message}`);
    }
  }

  // Global `groove.apply` action (palette, Clip menu): apply the current
  // template from anywhere. Runs only on nonce changes; the empty pool /
  // missing target reports honestly instead of throwing.
  const [handledApply, setHandledApply] = createSignal(0);
  createEffect(() => {
    const n = props.applyNonce ?? 0;
    if (n !== 0 && n !== handledApply()) {
      setHandledApply(n);
      void applyGrooveToTarget();
    }
  });

  async function applyGrooveToTarget(): Promise<void> {
    const found = currentTemplate();
    if (!found) {
      setMessage("pool is empty: extract a groove first");
      return;
    }
    if (!props.target) {
      setMessage("no target clip: open a MIDI clip to groove");
      return;
    }
    try {
      const out = applyGroove(props.target, found.template, params());
      setUndo((stack) => [...stack, props.target as MidiClip]);
      props.onEdit?.(out);
      if (props.targetClip) {
        await apply(grooveCommitOp("ui", props.targetClip, found.name));
      }
      setMessage(
        `applied “${found.name}” to ${props.targetName ?? "target"}: ` +
          `${out.notes.length} notes` +
          (props.targetClip ? " (ClipAdded op, undoable)" : ""),
      );
    } catch (e) {
      setMessage(`apply refused: ${(e as Error).message}`);
    }
  }

  function undoApply(): void {
    const stack = undo();
    if (stack.length === 0) {
      setMessage("nothing to undo");
      return;
    }
    const prev = stack[stack.length - 1];
    setUndo(stack.slice(0, -1));
    props.onEdit?.(prev);
    setMessage(`undid groove apply (${stack.length - 1} step${stack.length - 1 === 1 ? "" : "s"} left)`);
  }

  function removeGroove(name: string): void {
    pool().delete(name);
    if (selected() === name) setSelected("");
    setMessage(`forgot “${name}”`);
    touch();
  }

  function clamp01(v: number): number {
    return Math.min(1, Math.max(0, v));
  }

  return (
    <div data-testid="groove-panel" class="session-view">
      <div class="view-label">
        Groove — {props.sourceName ?? "demo phrase"} → {props.targetName ?? "target"} (in-timeline)
      </div>
      <div class="form-row">
        <label>
          Name
          <input
            data-testid="groove-name"
            value={grooveName()}
            onInput={(e) => setGrooveName(e.currentTarget.value)}
            placeholder="e.g. late-hats"
            style={{ width: "120px", "margin-left": "4px" }}
          />
        </label>
        <label>
          Steps/beat
          <select
            data-testid="groove-steps"
            value={String(steps())}
            onInput={(e) => setSteps(Number(e.currentTarget.value))}
            style={{ "margin-left": "4px" }}
          >
            {STEP_CHOICES.map((s) => (
              <option value={String(s)}>{s}</option>
            ))}
          </select>
        </label>
        <button data-testid="groove-extract" onClick={extract}>
          Extract feel
        </button>
      </div>
      <Show when={names().length > 0} fallback={<div class="session-dim">Pool is empty.</div>}>
        <div>
          <div class="view-label">Pool ({names().length})</div>
          <For each={names()}>
            {(n) => (
              <div class="lane">
                <span class="lane-name daw-numeric">
                  {pool().get(n)?.steps_per_beat ?? 0}/b
                </span>
                <span class="clip-chip clip-midi" title={describeOffsets(pool().get(n) ?? { offsets: [] })}>
                  {n}
                </span>
                <button
                  data-testid={`groove-delete-${n}`}
                  onClick={() => removeGroove(n)}
                  class="slot-btn silent"
                >
                  forget
                </button>
              </div>
            )}
          </For>
        </div>
      </Show>
      <div class="form-row">
        <label>
          Groove
          <select
            data-testid="groove-select"
            value={selected() || names()[0] || ""}
            onInput={(e) => setSelected(e.currentTarget.value)}
            style={{ "margin-left": "4px" }}
          >
            <For each={names()}>{(n) => <option value={n}>{n}</option>}</For>
          </select>
        </label>
        <label>
          Quantize
          <input
            data-testid="groove-quantize"
            type="number"
            min={0}
            max={1}
            step={0.05}
            value={params().quantize}
            onInput={(e) => setParams({ ...params(), quantize: clamp01(Number(e.currentTarget.value)) })}
            style={{ width: "56px", "margin-left": "4px" }}
          />
        </label>
        <label>
          Timing
          <input
            data-testid="groove-timing"
            type="number"
            min={0}
            max={1}
            step={0.05}
            value={params().timing}
            onInput={(e) => setParams({ ...params(), timing: clamp01(Number(e.currentTarget.value)) })}
            style={{ width: "56px", "margin-left": "4px" }}
          />
        </label>
        <label>
          Velocity
          <input
            data-testid="groove-velocity"
            type="number"
            min={0}
            max={1}
            step={0.05}
            value={params().velocity}
            onInput={(e) => setParams({ ...params(), velocity: clamp01(Number(e.currentTarget.value)) })}
            style={{ width: "56px", "margin-left": "4px" }}
          />
        </label>
      </div>
      <div class="form-row">
        <button data-testid="groove-preview" onClick={preview}>
          Preview
        </button>
        <button data-testid="groove-apply" onClick={() => void applyGrooveToTarget()}>
          Apply to target
        </button>
        <button
          data-testid="groove-undo"
          onClick={undoApply}
          disabled={undo().length === 0}
        >
          Undo ({undo().length})
        </button>
      </div>
      <Show when={message()}>
        <div data-testid="groove-status" class="meter-readout">
          {message()}
        </div>
      </Show>
    </div>
  );
}
