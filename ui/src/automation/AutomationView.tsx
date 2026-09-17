import { For, Show, createSignal } from "solid-js";
import type { Op, Project } from "../generated/project";
import { op_apply } from "../tauri/commands";
import { laneIdFor, pointSetOp } from "./edit";

const PARAMS = ["volume", "pan"] as const;
const LANE_BEATS = 32;

/**
 * Automation lanes view: one SVG strip per track `volume`/`pan` lane with
 * existing points as circles, click-to-add points through `AutomationPointSet`
 * ops (undoable, like every other edit). Pass `applyOp` to override the
 * transport (tests).
 */
export default function AutomationView(props: {
  project: Project | null;
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const [status, setStatus] = createSignal<string | null>(null);

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  function lanePoints(laneId: string) {
    return props.project?.automation.find((l) => l.id === laneId)?.points ?? [];
  }

  async function addPoint(node: string, param: string, beat: number, value: number) {
    const laneId = laneIdFor(node, param);
    const exists = (props.project?.automation ?? []).some((l) => l.id === laneId);
    try {
      const op = pointSetOp({ laneId, node, param }, beat, value, exists);
      await apply(op);
      setStatus(`set ${laneId} @ ${beat} = ${value}`);
    } catch (e) {
      setStatus(`automation refused: ${String(e)}`);
    }
  }

  function onStripClick(e: MouseEvent, node: string, param: string) {
    const el = e.currentTarget as SVGSVGElement;
    const rect = el.getBoundingClientRect();
    const beat = Math.min(
      LANE_BEATS,
      Math.max(0, ((e.clientX - rect.left) / rect.width) * LANE_BEATS),
    );
    const max = param === "pan" ? 1 : 1.5;
    const min = param === "pan" ? -1 : 0;
    const t = Math.min(1, Math.max(0, (e.clientY - rect.top) / rect.height));
    const value = Math.round((max - t * (max - min)) * 1000) / 1000;
    void addPoint(node, param, Math.round(beat * 100) / 100, value);
  }

  function pathFor(laneId: string, min: number, max: number): string {
    const pts = [...lanePoints(laneId)].sort((a, b) => a.beat - b.beat);
    const x = (b: number) => (Math.min(LANE_BEATS, Math.max(0, b)) / LANE_BEATS) * 300;
    const y = (v: number) => 44 - ((Math.min(max, Math.max(min, v)) - min) / (max - min)) * 40;
    const ends =
      pts.length === 0
        ? [
            { beat: 0, value: min },
            { beat: LANE_BEATS, value: min },
          ]
        : [{ beat: 0, value: pts[0].value }, ...pts, { beat: LANE_BEATS, value: pts[pts.length - 1].value }];
    return ends.map((p) => `${x(p.beat).toFixed(1)},${y(p.value).toFixed(1)}`).join(" ");
  }

  return (
    <div class="session-view" data-testid="automation-view">
      <Show when={props.project} fallback={<div class="session-dim">No project open.</div>}>
        <div class="session-dim">Click a lane to set a point (undoable op).</div>
        <For each={props.project?.tracks ?? []}>
          {(track) => (
            <For each={PARAMS}>
              {(param) => {
                const laneId = laneIdFor(track.id, param);
                const min = param === "pan" ? -1 : 0;
                const max = param === "pan" ? 1 : 1.5;
                return (
                  <div>
                    <div class="view-label daw-numeric">
                      {track.name}:{param} ({lanePoints(laneId).length} pts)
                    </div>
                    <svg
                      viewBox="0 0 300 48"
                      preserveAspectRatio="none"
                      role="img"
                      aria-label={`Automation lane ${laneId}`}
                      data-testid={`lane-${laneId}`}
                      onClick={(e) => onStripClick(e, track.id, param)}
                      style={{ display: "block", cursor: "crosshair", width: "100%", height: "48px" }}
                    >
                      <rect x="0" y="0" width="300" height="48" class="lane-canvas" />
                      <polyline points={pathFor(laneId, min, max)} class="lane-curve" />
                      <For each={lanePoints(laneId)}>
                        {(p) => (
                          <circle
                            cx={(Math.min(LANE_BEATS, Math.max(0, p.beat)) / LANE_BEATS) * 300}
                            cy={44 - ((Math.min(max, Math.max(min, p.value)) - min) / (max - min)) * 40}
                            r="3"
                            class="lane-point"
                          />
                        )}
                      </For>
                    </svg>
                  </div>
                );
              }}
            </For>
          )}
        </For>
        <Show when={status()}>
          <div data-testid="automation-status" class="session-launch-note">
            {status()}
          </div>
        </Show>
      </Show>
    </div>
  );
}
