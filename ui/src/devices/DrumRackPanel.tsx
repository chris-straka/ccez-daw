import { For, Show, createSignal } from "solid-js";
import type { Node, Op } from "../generated/project";
import { previewNote } from "../audio/preview";
import { op_apply } from "../tauri/commands";
import {
  DEFAULT_PAD_NAMES,
  DRUM_PAD_COUNT,
  bindPadDevice,
  defaultDrumRack,
  padTrimOps,
  setPadNote,
  setPadTrack,
  type DrumPadState,
} from "./model";

/**
 * Drum rack panel: 4x4 pad grid over the note map.
 *
 * The pad→(track, note) map is UI-local sidecar state (the frozen `Node`
 * carries numeric params only, so routing rides alongside the project —
 * the `DrumRack` sidecar precedent in `core/src/devices/sampler.rs`).
 * Per-pad trim (gain, transpose) edits land as frozen `ParamSet` ops on
 * the pad's bound sampler device (undoable via `op_undo`); unbound pads
 * keep their trim local until bound. Pass `applyOp` to override the
 * transport (tests) and `initial` to share a map between mounts.
 */
export default function DrumRackPanel(props: {
  trackId?: string;
  devices?: Node[];
  initial?: DrumPadState[];
  applyOp?: (op: Op) => Promise<unknown>;
  onStrike?: (pad: DrumPadState) => void;
}) {
  const [rack, setRack] = createSignal<DrumPadState[]>(
    props.initial ?? defaultDrumRack(props.trackId ?? "trk_drums"),
  );
  const [selected, setSelected] = createSignal(0);
  const [status, setStatus] = createSignal("pick a pad, then retarget its note or trim");

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  const samplers = (): Node[] =>
    (props.devices ?? []).filter((d) =>
      d.params.some((p) => p.id === "device_class" && p.value === 7),
    );

  const sel = (): DrumPadState => rack()[selected()] ?? rack()[0];

  function strike(pad: number): void {
    setSelected(pad);
    const hit = rack()[pad];
    props.onStrike?.(hit);
    // Audible immediately through the UI-local voice (pad note + trim
    // transpose/gain); the engine mix only renders committed clips.
    previewNote(hit.note + hit.transpose, { gain: hit.gain, seconds: 0.18 });
    setStatus(
      `pad ${pad} → ${hit.trackId}:${hit.note} (${DEFAULT_PAD_NAMES[pad]})` +
        (hit.deviceId ? ` · trim → ${hit.deviceId}` : " · unbound (trim stays local)"),
    );
  }

  async function pushTrim(pads: DrumPadState[], pad: number): Promise<void> {
    const hit = pads[pad];
    try {
      const ops = padTrimOps("ui", hit);
      for (const op of ops) await apply(op);
      setStatus(
        ops.length === 0
          ? `pad ${pad} trim kept local (bind a sampler device first)`
          : `pad ${pad} trim → ${hit.deviceId} (gain ${hit.gain}, transpose ${hit.transpose})`,
      );
    } catch (e) {
      setStatus(`trim refused: ${(e as Error).message}`);
    }
  }

  function editNote(note: number): void {
    try {
      const next = setPadNote(rack(), selected(), note);
      setRack(next);
      setStatus(`pad ${selected()} → note ${note}`);
    } catch (e) {
      setStatus(`note refused: ${(e as Error).message}`);
    }
  }

  function editTrack(trackId: string): void {
    try {
      setRack(setPadTrack(rack(), selected(), trackId));
      setStatus(`pad ${selected()} → track ${trackId}`);
    } catch (e) {
      setStatus(`track refused: ${(e as Error).message}`);
    }
  }

  function editTrim(field: "gain" | "transpose", value: number): void {
    const clamped =
      field === "gain"
        ? Math.min(4, Math.max(0, value))
        : Math.min(48, Math.max(-48, value));
    const next = rack().map((p) =>
      p.pad === selected() ? { ...p, [field]: clamped } : p,
    );
    setRack(next);
    void pushTrim(next, selected());
  }

  return (
    <div data-testid="drumrack-panel" class="session-view">
      <div class="view-label">Drum rack — pad grid → note map (in-sidecar)</div>
      <div class="pad-grid" data-testid="drumrack-grid">
        <For each={rack()}>
          {(p) => (
            <button
              data-testid={`drumrack-pad-${p.pad}`}
              class="pad-btn"
              classList={{ selected: p.pad === selected() }}
              onClick={() => strike(p.pad)}
              title={`pad ${p.pad} → ${p.trackId}:${p.note}`}
            >
              <span class="pad-index daw-numeric">{p.pad}</span>
              <span class="pad-name">{DEFAULT_PAD_NAMES[p.pad] ?? ""}</span>
              <span class="pad-note daw-numeric">{p.note}</span>
            </button>
          )}
        </For>
      </div>
      <Show when={rack().length !== DRUM_PAD_COUNT}>
        <div class="transport-error">rack holds {rack().length} pads, want {DRUM_PAD_COUNT}</div>
      </Show>
      <div class="view-label">
        Pad {selected()} — {DEFAULT_PAD_NAMES[selected()]}
      </div>
      <div class="form-row">
        <label>
          Note{" "}
          <input
            data-testid="drumrack-note"
            type="number"
            min={0}
            max={127}
            step={1}
            value={sel().note}
            onInput={(e) => editNote(Number(e.currentTarget.value))}
            style={{ width: "64px", "margin-left": "4px" }}
          />
        </label>
        <label>
          Track{" "}
          <input
            data-testid="drumrack-track"
            value={sel().trackId}
            onInput={(e) => editTrack(e.currentTarget.value)}
            style={{ width: "120px", "margin-left": "4px" }}
          />
        </label>
        <label>
          Device{" "}
          <select
            data-testid="drumrack-device"
            value={sel().deviceId}
            onInput={(e) => {
              setRack(bindPadDevice(rack(), selected(), e.currentTarget.value));
              setStatus(
                e.currentTarget.value
                  ? `pad ${selected()} trim → ${e.currentTarget.value}`
                  : `pad ${selected()} unbound`,
              );
            }}
            style={{ "margin-left": "4px", "max-width": "160px" }}
          >
            <option value="">(unbound)</option>
            <For each={samplers()}>{(d) => <option value={d.id}>{d.name}</option>}</For>
          </select>
        </label>
      </div>
      <div class="form-row">
        <label>
          Trim gain{" "}
          <input
            data-testid="drumrack-gain"
            type="number"
            min={0}
            max={4}
            step={0.05}
            value={sel().gain}
            onInput={(e) => editTrim("gain", Number(e.currentTarget.value))}
            style={{ width: "64px", "margin-left": "4px" }}
          />
        </label>
        <label>
          Trim transpose{" "}
          <input
            data-testid="drumrack-transpose"
            type="number"
            min={-48}
            max={48}
            step={1}
            value={sel().transpose}
            onInput={(e) => editTrim("transpose", Number(e.currentTarget.value))}
            style={{ width: "64px", "margin-left": "4px" }}
          />
        </label>
      </div>
      <Show when={status()}>
        <div data-testid="drumrack-status" class="meter-readout">
          {status()}
        </div>
      </Show>
    </div>
  );
}
