import { Show, createSignal } from "solid-js";
import type { Node, Op } from "../generated/project";
import { op_apply } from "../tauri/commands";
import { clampToNode, deviceClassOf, deviceParamOps, paramValue } from "./model";

/**
 * Sampler panel: sample select, pitch, envelope, and filter over the
 * existing frozen params (`transpose`, `gain`, `attack`, `release`,
 * `cutoff` — see `core/src/devices/class.rs`).
 *
 * Audio lives *beside* the project (the `SampleBank` sidecar over
 * engine `audio` assets), so the sample picker only names the asset key
 * for the selected device; a missing key renders silence, never an
 * error. Every param edit lands as one frozen `ParamSet` op through
 * `op_apply` (undoable via `op_undo`). Pass `applyOp` to override the
 * transport (tests).
 */
export default function SamplerPanel(props: {
  node: Node | null;
  sampleKeys?: string[];
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const [sampleKey, setSampleKey] = createSignal("");
  const [status, setStatus] = createSignal("");
  // Same drag-local pattern as the mixer strips: the thumb follows the
  // pointer, the op commits on release.
  const [drafts, setDrafts] = createSignal<Record<string, number>>({});

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  async function setParam(param: string, raw: number): Promise<void> {
    const node = props.node;
    if (!node) return;
    try {
      const value = clampToNode(node, param, raw);
      const [op] = deviceParamOps("ui", node, { [param]: value });
      await apply(op);
      setStatus(`${param} → ${value}`);
    } catch (e) {
      setStatus(`refused: ${(e as Error).message}`);
    }
  }

  function slider(
    param: string,
    label: string,
    min: number,
    max: number,
    step: number,
    testid: string,
  ) {
    const node = () => props.node;
    const committed = () => (node() ? paramValue(node() as Node, param, min) : min);
    const val = () => drafts()[param] ?? committed();
    function release(raw: number): void {
      setDrafts((d) => {
        const next = { ...d };
        delete next[param];
        return next;
      });
      void setParam(param, raw);
    }
    return (
      <label class="mixer-label">
        {label}{" "}
        <span class="mixer-values daw-numeric">{Number(val()).toFixed(step < 0.1 ? 3 : 2)}</span>
        <input
          data-testid={testid}
          type="range"
          min={min}
          max={max}
          step={step}
          value={val()}
          onInput={(e) => setDrafts((d) => ({ ...d, [param]: Number(e.currentTarget.value) }))}
          onChange={(e) => release(Number(e.currentTarget.value))}
        />
      </label>
    );
  }

  return (
    <div data-testid="sampler-panel" class="session-view">
      <Show
        when={props.node}
        fallback={<div class="session-dim">No sampler device selected.</div>}
      >
        {(node) => (
          <Show
            when={deviceClassOf(node()) === "sampler"}
            fallback={
              <div class="session-dim">
                {node().name} is not a sampler (device_class {String(paramValue(node(), "device_class", 0))}).
              </div>
            }
          >
            <div class="view-label">
              Sampler — {node().name} <span class="daw-numeric">({node().id})</span>
            </div>
            <div class="form-row">
              <label>
                Sample{" "}
                <select
                  data-testid="sampler-sample"
                  value={sampleKey()}
                  onInput={(e) => {
                    setSampleKey(e.currentTarget.value);
                    setStatus(
                      e.currentTarget.value
                        ? `sample key “${e.currentTarget.value}” (asset lives beside the project; missing renders silence)`
                        : "no sample — renders silence",
                    );
                  }}
                  style={{ "margin-left": "4px", "max-width": "200px" }}
                >
                  <option value="">(no sample — silence)</option>
                  {(props.sampleKeys ?? []).map((k) => (
                    <option value={k}>{k}</option>
                  ))}
                </select>
              </label>
            </div>
            {slider("transpose", "pitch (st)", -48, 48, 1, "sampler-transpose")}
            {slider("gain", "gain (x)", 0, 4, 0.01, "sampler-gain")}
            {slider("attack", "attack (s)", 0, 10, 0.005, "sampler-attack")}
            {slider("release", "release (s)", 0, 10, 0.01, "sampler-release")}
            {slider("cutoff", "filter cutoff (Hz)", 20, 20000, 1, "sampler-cutoff")}
            <Show when={status()}>
              <div data-testid="sampler-status" class="meter-readout">
                {status()}
              </div>
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );
}
