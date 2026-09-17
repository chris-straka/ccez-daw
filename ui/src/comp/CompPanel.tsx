import { For, Show, createMemo, createSignal } from "solid-js";
import type { Clip, Op, Project } from "../generated/project";
import { op_apply } from "../tauri/commands";
import { assignTakeLanes } from "../record/record";
import {
  auditionTake,
  buildComp,
  compCommitOp,
  takesForRegion,
  type CompSection,
} from "./comp";

/**
 * Comp panel: take lanes + audition/switch + commit over one track region.
 *
 * Takes are ordinary clips (`take:<id>` sources from loop/punch recording);
 * this view lists the lanes overlapping `[start, start + length)`, lets the
 * player audition (select) a take per comp section, then commits the
 * composite as one frozen `ClipAdded` op through the existing `op_apply`
 * surface — no new IPC, no schema change. Pass `applyOp` to override the
 * transport (tests).
 */
export default function CompPanel(props: {
  project: Project | null;
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const [trackId, setTrackId] = createSignal<string | null>(null);
  const [startBeats, setStartBeats] = createSignal(0);
  const [lengthBeats, setLengthBeats] = createSignal(4);
  const [splitBeats, setSplitBeats] = createSignal(2);
  const [picks, setPicks] = createSignal<string[]>([]);
  const [auditionId, setAuditionId] = createSignal<string | null>(null);
  const [status, setStatus] = createSignal("");

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  const activeTrackId = createMemo(() => {
    const project = props.project;
    if (!project || project.tracks.length === 0) return null;
    const wanted = trackId();
    return wanted && project.tracks.some((t) => t.id === wanted)
      ? wanted
      : project.tracks[0].id;
  });

  const regionTakes = createMemo((): Clip[] => {
    const project = props.project;
    const track = activeTrackId();
    if (!project || !track) return [];
    const start = startBeats();
    return takesForRegion(project.clips, track, start, start + lengthBeats());
  });

  const lanes = createMemo(() => {
    const takes = regionTakes();
    if (takes.length === 0) return [];
    try {
      const order = assignTakeLanes(takes);
      const laneOf = new Map(order.map((l) => [l.take_id, l.lane]));
      return takes.map((take) => ({ take, lane: laneOf.get(take.id) ?? 0 }));
    } catch {
      return takes.map((take) => ({ take, lane: 0 }));
    }
  });

  const sectionRanges = createMemo((): Array<[number, number]> => {
    const start = startBeats();
    const end = start + lengthBeats();
    const split = splitBeats();
    if (!(split > start && split < end)) return [[start, end]];
    return [
      [start, split],
      [split, end],
    ];
  });

  function pickFor(section: number): string | null {
    const takes = regionTakes();
    if (takes.length === 0) return null;
    const pick = picks()[section];
    return pick && takes.some((t) => t.id === pick) ? pick : takes[0].id;
  }

  function setPick(section: number, takeId: string): void {
    setPicks((prev) => {
      const next = [...prev];
      next[section] = takeId;
      return next;
    });
  }

  function audition(takeId: string): void {
    const project = props.project;
    const track = activeTrackId();
    if (!project || !track) return;
    try {
      const take = auditionTake(project.clips, track, takeId);
      setAuditionId(take.id);
      setStatus(
        `auditioning ${take.id} (${take.start_beats}–${take.start_beats + take.length_beats}) — route to monitor`,
      );
    } catch (e) {
      setStatus(`audition failed: ${String(e)}`);
    }
  }

  function preview(): { text: string; ok: boolean } {
    const track = activeTrackId();
    if (!track) return { text: "No project open.", ok: false };
    const takes = regionTakes();
    if (takes.length === 0) return { text: "No takes overlap this region.", ok: false };
    try {
      const sections: CompSection[] = sectionRanges().map(([s, e], i) => ({
        take_id: pickFor(i) ?? takes[0].id,
        start_beats: s,
        end_beats: e,
      }));
      const comp = buildComp("preview", track, "preview", sections, takes);
      return {
        text: `${comp.start_beats}–${comp.start_beats + comp.length_beats} from ${comp.source}`,
        ok: true,
      };
    } catch (e) {
      return { text: `comp invalid: ${String(e)}`, ok: false };
    }
  }

  async function commit(): Promise<void> {
    const project = props.project;
    const track = activeTrackId();
    if (!project || !track) return;
    const takes = regionTakes();
    try {
      const sections: CompSection[] = sectionRanges().map(([s, e], i) => ({
        take_id: pickFor(i) ?? takes[0].id,
        start_beats: s,
        end_beats: e,
      }));
      const start = startBeats();
      const id = `clip_comp_${track}_${start}_${start + lengthBeats()}`.replaceAll(".", "_");
      const comp = buildComp(id, track, "Comp", sections, takes);
      await apply(compCommitOp("ui", comp));
      setStatus(`committed ${comp.id} (${comp.source})`);
    } catch (e) {
      setStatus(`comp commit failed: ${String(e)}`);
    }
  }

  const previewText = createMemo(preview);

  return (
    <div data-testid="comp-panel" class="session-view">
      <Show
        when={props.project}
        fallback={<div class="session-dim">No project open.</div>}
      >
        <div class="session-controls">
          <label>
            Track{" "}
            <select
              data-testid="comp-track"
              value={activeTrackId() ?? ""}
              onInput={(e) => setTrackId(e.currentTarget.value)}
            >
              <For each={props.project?.tracks ?? []}>
                {(track) => <option value={track.id}>{track.name}</option>}
              </For>
            </select>
          </label>
          <label>
            Start{" "}
            <input
              data-testid="comp-start"
              type="number"
              min={0}
              value={startBeats()}
              onInput={(e) => setStartBeats(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <label>
            Length{" "}
            <input
              data-testid="comp-length"
              type="number"
              min={1}
              value={lengthBeats()}
              onInput={(e) => setLengthBeats(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <label>
            Split{" "}
            <input
              data-testid="comp-split"
              type="number"
              value={splitBeats()}
              onInput={(e) => setSplitBeats(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <button data-testid="comp-commit" onClick={() => void commit()}>
            Commit comp
          </button>
        </div>
        <Show
          when={lanes().length > 0}
          fallback={
            <div class="session-dim">
              No takes overlap this region — record a take first, then pick
              one per section to build the comp.
            </div>
          }
        >
          <div>
            <div class="view-label">Takes</div>
            <For each={lanes()}>
              {({ take, lane }) => (
                <div class="lane">
                  <span class="lane-name daw-numeric">lane {lane}</span>
                  <span
                    title={`${take.name} @ ${take.start_beats} (${take.length_beats} beats)`}
                    class={`clip-chip ${take.kind === "Audio" ? "clip-audio" : "clip-midi"}`}
                  >
                    {take.id}
                  </span>
                  <button
                    data-testid={`comp-audition-${take.id}`}
                    onClick={() => audition(take.id)}
                    class="slot-btn"
                    classList={{ filled: auditionId() === take.id }}
                  >
                    {auditionId() === take.id ? "◉ auditioning" : "◎ audition"}
                  </button>
                </div>
              )}
            </For>
          </div>
          <div>
            <div class="view-label">Sections</div>
            <For each={sectionRanges()}>
              {([s, e], i) => (
                <div class="lane">
                  <span class="lane-name daw-numeric">
                    {s}&ndash;{e}
                  </span>
                  <select
                    data-testid={`comp-section-${i()}`}
                    value={pickFor(i()) ?? ""}
                    onInput={(ev) => setPick(i(), ev.currentTarget.value)}
                  >
                    <For each={regionTakes()}>
                      {(take) => <option value={take.id}>{take.id}</option>}
                    </For>
                  </select>
                </div>
              )}
            </For>
          </div>
          <div
            data-testid="comp-preview"
            class={previewText().ok ? "session-launch-note" : "session-dim"}
          >
            {previewText().text}
          </div>
        </Show>
        <Show when={status()}>
          <span data-testid="comp-status" class="session-dim">
            {status()}
          </span>
        </Show>
      </Show>
    </div>
  );
}
