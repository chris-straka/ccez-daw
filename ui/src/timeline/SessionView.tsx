import { For, Show, createSignal } from "solid-js";
import type { Op, Project } from "../generated/project";
import { op_apply } from "../tauri/commands";
import {
  jamRecordOps,
  planSceneLaunch,
  planSlotLaunch,
  type JamEvent,
  type LaunchQuant,
} from "./launch";
import { launcherSlots, sampleTimelineDoc } from "./model";

export type QuantKind = "none" | "beat" | "bar" | "scene";

export function quantForKind(kind: QuantKind): LaunchQuant {
  switch (kind) {
    case "none":
      return { kind: "none" };
    case "beat":
      return { kind: "beat" };
    case "bar":
      return { kind: "bar" };
    case "scene":
      return { kind: "scene" };
  }
}

/**
 * Session View: clip-slot grid + scene launch buttons + quantization
 * selector, wired to the `launch.ts` view model (`planSlotLaunch` /
 * `planSceneLaunch` / `jamRecordOps`).
 *
 * Presses quantize to the next grid boundary; every launch appends a jam
 * event, and "Record jam" writes the pending jam back to the arrangement as
 * frozen `ClipAdded` ops through the existing `op_apply` surface — no new
 * IPC, no schema change. Pass `applyOp` to override the transport (tests).
 */
export default function SessionView(props: {
  project: Project | null;
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const doc = sampleTimelineDoc();
  const [quantKind, setQuantKind] = createSignal<QuantKind>("bar");
  const [requestedBeat, setRequestedBeat] = createSignal(0);
  const [jam, setJam] = createSignal<JamEvent[]>([]);
  const [lastLaunch, setLastLaunch] = createSignal<string | null>(null);
  const [jamStatus, setJamStatus] = createSignal<string>("jam: 0 pending");

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  function launchSlot(sectionId: string, trackId: string): void {
    const project = props.project;
    if (!project) return;
    try {
      const plan = planSlotLaunch(
        project,
        doc,
        { section_id: sectionId, track_id: trackId },
        requestedBeat(),
        quantForKind(quantKind()),
      );
      setJam((prev) => [
        ...prev,
        { section_id: plan.section_id, launch_beat: plan.launch_beat },
      ]);
      setLastLaunch(
        `launched ${plan.section_id}:${plan.track_id} @ ${plan.launch_beat} (${plan.clip_ids.join(", ") || "silent"})`,
      );
      setJamStatus(`jam: ${jam().length} pending`);
    } catch (e) {
      setLastLaunch(`launch failed: ${String(e)}`);
    }
  }

  function launchScene(sectionId: string): void {
    const project = props.project;
    if (!project) return;
    try {
      const plan = planSceneLaunch(
        project,
        doc,
        sectionId,
        requestedBeat(),
        quantForKind(quantKind()),
      );
      setJam((prev) => [
        ...prev,
        { section_id: plan.section_id, launch_beat: plan.launch_beat },
      ]);
      setLastLaunch(
        `launched scene ${plan.section_id} @ ${plan.launch_beat} (${plan.clip_ids.join(", ") || "silent"})`,
      );
      setJamStatus(`jam: ${jam().length} pending`);
    } catch (e) {
      setLastLaunch(`launch failed: ${String(e)}`);
    }
  }

  async function recordJam(): Promise<void> {
    const project = props.project;
    if (!project) return;
    const events = jam();
    try {
      const ops = jamRecordOps("ui", project, doc, events);
      for (const op of ops) await apply(op);
      setJam([]);
      setJamStatus(`recorded ${ops.length} clips from ${events.length} launches`);
      setLastLaunch(null);
    } catch (e) {
      setJamStatus(`jam record failed: ${String(e)}`);
    }
  }

  return (
    <div
      id="session-view"
      data-testid="session-view"
      tabIndex={-1}
      class="session-view"
    >
      <Show when={props.project} fallback={<div class="session-dim">No project open.</div>}>
        <div class="session-controls">
          <label>
            Quant{" "}
            <select
              data-testid="session-quant"
              value={quantKind()}
              onInput={(e) => setQuantKind(e.currentTarget.value as QuantKind)}
            >
              <option value="none">None</option>
              <option value="beat">Beat</option>
              <option value="bar">Bar</option>
              <option value="scene">Scene</option>
            </select>
          </label>
          <label>
            Beat{" "}
            <input
              data-testid="session-beat"
              type="number"
              min={0}
              value={requestedBeat()}
              onInput={(e) => setRequestedBeat(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <button data-testid="jam-record" onClick={() => void recordJam()}>
            Record jam
          </button>
          <span data-testid="jam-status" class="session-dim">
            {jamStatus()}
          </span>
        </div>
        <Show when={lastLaunch()}>
          <div data-testid="session-last-launch" class="session-launch-note">
            {lastLaunch()}
          </div>
        </Show>
        <For each={doc.sections}>
          {(section) => (
            <div class="scene-row">
              <button
                data-testid={`scene-launch-${section.id}`}
                onClick={() => launchScene(section.id)}
                class="scene-btn"
              >
                ▶ {section.name}
              </button>
              <For
                each={launcherSlots(props.project!, doc).filter(
                  (s) => s.section_id === section.id,
                )}
              >
                {(slot) => (
                  <button
                    data-testid={`slot-launch-${slot.section_id}-${slot.track_id}`}
                    title={`${slot.track_id}: ${slot.clip_ids.join(", ") || "silent"}`}
                    onClick={() => launchSlot(slot.section_id, slot.track_id)}
                    class="slot-btn"
                    classList={{
                      filled: slot.clip_ids.length > 0,
                      silent: slot.clip_ids.length === 0,
                    }}
                  >
                    {slot.clip_ids.join(", ") || "—"}
                  </button>
                )}
              </For>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}
