import { For, Show, createSignal } from "solid-js";
import type { Node, Op, Project } from "../generated/project";
import { clampToNode, deviceParamOps, paramValue } from "../devices/model";
import { op_apply } from "../tauri/commands";
import {
  spatialDeviceForTrack,
  trackSpatialParams,
} from "./spatial";

/**
 * Spatial panel: per-track first-order ambisonic placement in the mixer.
 *
 * The track picker lists every track; the sliders bind the track's
 * `Spatial` device node (`azimuth`/`elevation`/`gain`, `device_class` 11).
 * Every edit lands as one frozen `ParamSet` op through `op_apply`
 * (undoable via `op_undo`, no schema change). Tracks without a `Spatial`
 * device show their stub text — the panel never invents devices. The
 * stereo image shown is the HRTF-free monitor decode (placement aid, not
 * a binaural render; Dolby Atmos is out of scope).
 */
export default function SpatialPanel(props: {
  project: Project;
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const withSpatial = () =>
    props.project.tracks.filter((t) => spatialDeviceForTrack(props.project, t.id));
  const [trackId, setTrackId] = createSignal<string>(withSpatial()[0]?.id ?? "");
  const [status, setStatus] = createSignal("");

  const node = (): Node | null => {
    const id = trackId() || withSpatial()[0]?.id;
    if (!id) return null;
    return spatialDeviceForTrack(props.project, id) ?? null;
  };

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  async function setParam(param: string, raw: number): Promise<void> {
    const dev = node();
    if (!dev) return;
    try {
      const value = clampToNode(dev, param, raw);
      const [op] = deviceParamOps("ui", dev, { [param]: value });
      await apply(op);
      setStatus(`${dev.id}:${param} → ${value}`);
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
    const dev = () => node();
    const val = () => (dev() ? paramValue(dev() as Node, param, min) : min);
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
          onInput={(e) => void setParam(param, Number(e.currentTarget.value))}
        />
      </label>
    );
  }

  const placement = () =>
    trackId() ? trackSpatialParams(props.project, trackId()) : null;

  return (
    <div data-testid="spatial-panel" class="session-view">
      <Show
        when={withSpatial().length > 0}
        fallback={
          <div class="session-dim">
            No track carries a Spatial device — add one (`device_class` 11 with
            azimuth/elevation/gain params) to place the track in the ambisonic field.
          </div>
        }
      >
        <div class="form-row">
          <label>
            Track{" "}
            <select
              data-testid="spatial-track"
              value={trackId()}
              onInput={(e) => setTrackId(e.currentTarget.value)}
              style={{ "margin-left": "4px", "max-width": "200px" }}
            >
              <For each={withSpatial()}>{(t) => <option value={t.id}>{t.name}</option>}</For>
            </select>
          </label>
        </div>
        <Show when={node()}>
          {(dev) => (
            <div>
              <div class="view-label">
                Spatial — {dev().name} <span class="daw-numeric">({dev().id})</span>
              </div>
              {slider("azimuth", "azimuth (deg, 0 = front, + = left)", -180, 180, 1, "spatial-azimuth")}
              {slider("elevation", "elevation (deg, 0 = horizon, + = up)", -90, 90, 1, "spatial-elevation")}
              {slider("gain", "gain (x)", 0, 4, 0.01, "spatial-gain")}
              <Show when={placement()}>
                {(p) => (
                  <div class="meter-readout">
                    field @ ({p().azimuth}°, {p().elevation}°, ×{p().gain}) — HRTF-free monitor, not binaural
                  </div>
                )}
              </Show>
              <Show when={status()}>
                <div data-testid="spatial-status" class="meter-readout">
                  {status()}
                </div>
              </Show>
            </div>
          )}
        </Show>
      </Show>
    </div>
  );
}
