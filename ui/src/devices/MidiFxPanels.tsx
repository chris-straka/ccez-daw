import { Show, createSignal } from "solid-js";
import type { Node, Op } from "../generated/project";
import { op_apply } from "../tauri/commands";
import {
  ARP_MODE_OPTIONS,
  CHORD_TYPE_OPTIONS,
  CHORD_VOICING_OPTIONS,
  clampToNode,
  deviceClassOf,
  deviceParamOps,
  paramValue,
} from "./model";

/**
 * MIDI FX panels: arpeggiator (mode/rate/gate/octaves), chord
 * generator (type/inversion/voicing), and humanize (timing/velocity
 * amounts) over the existing frozen params — see
 * `core/src/devices/midifx.rs` for ids and ranges.
 *
 * Every edit lands as one frozen `ParamSet` op through `op_apply`
 * (undoable via `op_undo`, no schema change). Pass `applyOp` to override
 * the transport (tests).
 */

function useParamSet(node: () => Node | null, applyOp?: (op: Op) => Promise<unknown>) {
  const [status, setStatus] = createSignal("");
  const apply = (op: Op): Promise<unknown> =>
    applyOp ? applyOp(op) : op_apply({ op });

  async function setParam(param: string, raw: number): Promise<void> {
    const n = node();
    if (!n) return;
    try {
      const value = clampToNode(n, param, raw);
      const [op] = deviceParamOps("ui", n, { [param]: value });
      await apply(op);
      setStatus(`${param} → ${value}`);
    } catch (e) {
      setStatus(`refused: ${(e as Error).message}`);
    }
  }

  return { status, setParam };
}

function NumField(props: {
  testid: string;
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onCommit: (v: number) => void;
}) {
  return (
    <label>
      {props.label}{" "}
      <input
        data-testid={props.testid}
        type="number"
        min={props.min}
        max={props.max}
        step={props.step}
        value={props.value}
        onInput={(e) => props.onCommit(Number(e.currentTarget.value))}
        style={{ width: "72px", "margin-left": "4px" }}
      />
    </label>
  );
}

function ModeSelect(props: {
  testid: string;
  label: string;
  value: number;
  options: Array<{ code: number; label: string }>;
  onCommit: (v: number) => void;
}) {
  return (
    <label>
      {props.label}{" "}
      <select
        data-testid={props.testid}
        value={String(Math.round(props.value))}
        onInput={(e) => props.onCommit(Number(e.currentTarget.value))}
        style={{ "margin-left": "4px" }}
      >
        {props.options.map((o) => (
          <option value={String(o.code)}>{o.label}</option>
        ))}
      </select>
    </label>
  );
}

function wrongClass(name: string, node: Node, want: string): string {
  return `${name} is not a ${want} device (device_class ${paramValue(node, "device_class", 0)}).`;
}

export function ArpPanel(props: { node: Node | null; applyOp?: (op: Op) => Promise<unknown> }) {
  const current = () => props.node;
  const { status, setParam } = useParamSet(current, props.applyOp);
  const val = (id: string, fb: number) => (current() ? paramValue(current() as Node, id, fb) : fb);

  return (
    <div data-testid="arp-panel" class="session-view">
      <Show when={current()} fallback={<div class="session-dim">No arpeggiator device selected.</div>}>
        {(node) => (
          <Show when={deviceClassOf(node()) === "arpeggiator"} fallback={<div class="session-dim">{wrongClass(node().name, node(), "arpeggiator")}</div>}>
            <div class="view-label">Arp — {node().name} <span class="daw-numeric">({node().id})</span></div>
            <div class="form-row">
              <ModeSelect testid="arp-mode" label="Pattern" value={val("arp_mode", 0)} options={ARP_MODE_OPTIONS} onCommit={(v) => void setParam("arp_mode", v)} />
              <NumField testid="arp-rate" label="Rate (beats)" value={val("arp_rate", 0.25)} min={0.0625} max={4} step={0.0625} onCommit={(v) => void setParam("arp_rate", v)} />
            </div>
            <div class="form-row">
              <NumField testid="arp-gate" label="Gate" value={val("arp_gate", 0.8)} min={0.05} max={1} step={0.05} onCommit={(v) => void setParam("arp_gate", v)} />
              <NumField testid="arp-octaves" label="Octaves" value={val("arp_octaves", 1)} min={1} max={4} step={1} onCommit={(v) => void setParam("arp_octaves", Math.round(v))} />
              <NumField testid="arp-seed" label="Seed" value={val("arp_seed", 0)} min={0} max={4294967295} step={1} onCommit={(v) => void setParam("arp_seed", Math.round(v))} />
            </div>
            <Show when={status()}><div data-testid="arp-status" class="meter-readout">{status()}</div></Show>
          </Show>
        )}
      </Show>
    </div>
  );
}

export function ChordPanel(props: { node: Node | null; applyOp?: (op: Op) => Promise<unknown> }) {
  const current = () => props.node;
  const { status, setParam } = useParamSet(current, props.applyOp);
  const val = (id: string, fb: number) => (current() ? paramValue(current() as Node, id, fb) : fb);

  return (
    <div data-testid="chord-panel" class="session-view">
      <Show when={current()} fallback={<div class="session-dim">No chord device selected.</div>}>
        {(node) => (
          <Show when={deviceClassOf(node()) === "chord"} fallback={<div class="session-dim">{wrongClass(node().name, node(), "chord")}</div>}>
            <div class="view-label">Chord — {node().name} <span class="daw-numeric">({node().id})</span></div>
            <div class="form-row">
              <ModeSelect testid="chord-type" label="Type" value={val("chord_type", 0)} options={CHORD_TYPE_OPTIONS} onCommit={(v) => void setParam("chord_type", v)} />
              <NumField testid="chord-inversion" label="Inversion" value={val("chord_inversion", 0)} min={0} max={3} step={1} onCommit={(v) => void setParam("chord_inversion", Math.round(v))} />
              <ModeSelect testid="chord-voicing" label="Voicing" value={val("chord_voicing", 0)} options={CHORD_VOICING_OPTIONS} onCommit={(v) => void setParam("chord_voicing", v)} />
            </div>
            <Show when={status()}><div data-testid="chord-status" class="meter-readout">{status()}</div></Show>
          </Show>
        )}
      </Show>
    </div>
  );
}

export function HumanizePanel(props: { node: Node | null; applyOp?: (op: Op) => Promise<unknown> }) {
  const current = () => props.node;
  const { status, setParam } = useParamSet(current, props.applyOp);
  const val = (id: string, fb: number) => (current() ? paramValue(current() as Node, id, fb) : fb);

  return (
    <div data-testid="humanize-panel" class="session-view">
      <Show when={current()} fallback={<div class="session-dim">No humanize device selected.</div>}>
        {(node) => (
          <Show when={deviceClassOf(node()) === "humanize"} fallback={<div class="session-dim">{wrongClass(node().name, node(), "humanize")}</div>}>
            <div class="view-label">Humanize — {node().name} <span class="daw-numeric">({node().id})</span></div>
            <div class="form-row">
              <NumField testid="hum-timing" label="Timing (beats)" value={val("hum_timing", 0.01)} min={0} max={0.25} step={0.005} onCommit={(v) => void setParam("hum_timing", v)} />
              <NumField testid="hum-velocity" label="Velocity" value={val("hum_velocity", 8)} min={0} max={64} step={1} onCommit={(v) => void setParam("hum_velocity", v)} />
              <NumField testid="hum-seed" label="Seed" value={val("hum_seed", 0)} min={0} max={4294967295} step={1} onCommit={(v) => void setParam("hum_seed", Math.round(v))} />
            </div>
            <Show when={status()}><div data-testid="humanize-status" class="meter-readout">{status()}</div></Show>
          </Show>
        )}
      </Show>
    </div>
  );
}
