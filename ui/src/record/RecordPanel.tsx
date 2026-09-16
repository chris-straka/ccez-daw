import { For, Show, createMemo, createSignal } from "solid-js";
import type { ClipKind, EngineState, Op, Project } from "../generated/project";
import { op_apply } from "../tauri/commands";
import { sampleTimelineDoc, type Section } from "../timeline/model";
import {
  availableInputDevices,
  countInClicks,
  countInSeconds,
  formatLevels,
  isArmed,
  isPunching,
  monitorLevels,
  punchRangeForSection,
  punchTakeClip,
  selectInputDevice,
  shouldMonitor,
  takeCommitOp,
  toggleArm,
  validatePunch,
  type InputDeviceInfo,
  type MonitorMode,
  type PunchRange,
} from "./record";

/**
 * Record panel: arm tracks, punch in/out at section boundaries or on
 * demand, count-in, and commit takes as new clips — wired to the
 * `record.ts` view model (`validatePunch` / `punchTakeClip` /
 * `takeCommitOp`).
 *
 * Arming and monitoring are control-room switches held in local signals,
 * never project state (the frozen v0 schema is untouched). Every take
 * lands through the existing `op_apply` surface as one frozen `ClipAdded`
 * op per armed track, so takes undo/redo like any other op. Pass
 * `applyOp` to override the transport (tests).
 */
export default function RecordPanel(props: {
  project: Project | null;
  engineState?: EngineState;
  sections?: Section[];
  /** cpal input devices from the host; absent/headless = null input only. */
  inputDevices?: InputDeviceInfo[] | null;
  /** Latest drained capture block driving the meter (hardware or null). */
  inputSamples?: ArrayLike<number>;
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const [armed, setArmed] = createSignal<string[]>([]);
  const [punchKind, setPunchKind] = createSignal<"manual" | "auto">("auto");
  const [punchStart, setPunchStart] = createSignal(0);
  const [punchEnd, setPunchEnd] = createSignal(4);
  const [sectionId, setSectionId] = createSignal<string | null>(null);
  const [playhead, setPlayhead] = createSignal(0);
  const [passStart, setPassStart] = createSignal<number | null>(null);
  const [passCount, setPassCount] = createSignal(1);
  const [countBars, setCountBars] = createSignal(1);
  const [countBeats, setCountBeats] = createSignal(4);
  const [monitor, setMonitor] = createSignal<MonitorMode>("auto");
  const [takeKind, setTakeKind] = createSignal<ClipKind>("Audio");
  const [inputDevice, setInputDevice] = createSignal<string | null>(null);
  const [status, setStatus] = createSignal("record: arm a track to begin");

  const devices = createMemo((): InputDeviceInfo[] =>
    availableInputDevices(props.inputDevices),
  );
  const levels = createMemo(() => monitorLevels(props.inputSamples ?? []));

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  const sections = createMemo((): Section[] => props.sections ?? sampleTimelineDoc().sections);
  const transport: () => EngineState = () => props.engineState ?? "Stopped";
  const recording = () => transport() === "Recording";

  const punchRange = createMemo((): { range: PunchRange | null; error: string | null } => {
    try {
      return { range: validatePunch(punchStart(), punchEnd()), error: null };
    } catch (e) {
      return { range: null, error: String(e) };
    }
  });

  const punching = createMemo((): boolean => {
    const range = punchRange().range;
    if (punchKind() === "manual") return isPunching(playhead(), { kind: "manual" }, recording());
    if (!range) return false;
    return isPunching(playhead(), { kind: "auto", range }, recording());
  });

  const countIn = createMemo(() => {
    try {
      const spec = { bars: countBars(), beats_per_bar: countBeats() };
      const clicks = countInClicks(spec);
      const tempo = props.project?.tempo ?? 120;
      return { text: `${clicks.map((c) => `${c.offset_beats}${c.accent ? "*" : ""}`).join(" ")} · ${countInSeconds(spec, tempo).toFixed(2)}s @ ${tempo} BPM`, error: null as string | null };
    } catch (e) {
      return { text: "", error: String(e) };
    }
  });

  function useSection(): void {
    const list = sections();
    const wanted = sectionId() ?? list[0]?.id ?? null;
    const section = list.find((s) => s.id === wanted);
    if (!section) {
      setStatus("record: no section to use");
      return;
    }
    try {
      const range = punchRangeForSection(section.start_beats, section.length_beats);
      setPunchKind("auto");
      setPunchStart(range.start_beats);
      setPunchEnd(range.end_beats);
      setStatus(`record: punch [${range.start_beats}, ${range.end_beats}) from section ${section.name}`);
    } catch (e) {
      setStatus(`record: bad section range: ${String(e)}`);
    }
  }

  function punchIn(): void {
    if (passStart() !== null) {
      setStatus("record: already punched in — punch out first");
      return;
    }
    setPassStart(playhead());
    setStatus(`record: punched in @ ${playhead()}`);
  }

  function punchOut(): void {
    if (passStart() === null) {
      setStatus("record: not punched in");
      return;
    }
    setStatus(`record: punched out @ ${playhead()} (pass [${passStart()}, ${playhead()}))`);
  }

  function takeRange(): { range: PunchRange | null; error: string } {
    if (punchKind() === "auto") {
      const { range, error } = punchRange();
      if (!range) return { range: null, error: `bad punch range: ${error}` };
      return { range, error: "" };
    }
    const start = passStart();
    if (start === null) return { range: null, error: "manual punch needs a punch-in first" };
    if (!(playhead() > start)) return { range: null, error: `manual pass needs playhead past punch-in (${start})` };
    try {
      return { range: validatePunch(start, playhead()), error: "" };
    } catch (e) {
      return { range: null, error: String(e) };
    }
  }

  async function commitTake(): Promise<void> {
    const project = props.project;
    if (!project) return;
    const trackIds = armed();
    if (trackIds.length === 0) {
      setStatus("record: arm at least one track");
      return;
    }
    const { range, error } = takeRange();
    if (!range) {
      setStatus(`record: cannot cut take — ${error}`);
      return;
    }
    const pass = passCount();
    try {
      const committed: string[] = [];
      for (const trackId of trackIds) {
        const id = `take_${trackId}_${range.start_beats}_${pass}`.replaceAll(".", "_");
        const take = punchTakeClip(id, trackId, `Take ${pass}`, takeKind(), range);
        await apply(takeCommitOp("ui", take));
        committed.push(id);
      }
      setPassCount(pass + 1);
      setPassStart(null);
      setStatus(`record: take ${pass} → ${committed.join(", ")} (${range.start_beats}–${range.end_beats}) via ${inputDevice() ?? "null input"}`);
    } catch (e) {
      setStatus(`record: take commit failed: ${String(e)}`);
    }
  }

  return (
    <div data-testid="record-panel" class="session-view">
      <Show when={props.project} fallback={<div class="session-dim">No project open.</div>}>
        <div class="view-label">Armed tracks</div>
        <Show when={(props.project?.tracks ?? []).length > 0} fallback={<div class="session-dim">No tracks.</div>}>
          <For each={props.project?.tracks ?? []}>
            {(track) => (
              <div class="lane">
                <button
                  data-testid={`record-arm-${track.id}`}
                  class="transport-play daw-numeric"
                  classList={{ playing: isArmed(armed(), track.id) }}
                  onClick={() => {
                    setArmed((prev) => toggleArm(prev, track.id));
                    setStatus(
                      isArmed(armed(), track.id)
                        ? `record: ${track.name} armed`
                        : `record: ${track.name} disarmed`,
                    );
                  }}
                >
                  {isArmed(armed(), track.id) ? "● ARM" : "○ arm"}
                </button>
                <span class="lane-name daw-numeric">{track.name}</span>
                <Show when={isArmed(armed(), track.id)}>
                  <span
                    class="clip-chip"
                    classList={{
                      "clip-audio": takeKind() === "Audio",
                      "clip-midi": takeKind() === "Midi",
                    }}
                  >
                    {shouldMonitor(monitor(), true, transport()) ? "monitoring" : "muted"}
                  </span>
                </Show>
              </div>
            )}
          </For>
        </Show>

        <div class="view-label">Punch</div>
        <div class="session-controls">
          <label>
            Mode{" "}
            <select
              data-testid="record-punch-kind"
              value={punchKind()}
              onInput={(e) => setPunchKind(e.currentTarget.value as "manual" | "auto")}
            >
              <option value="auto">auto</option>
              <option value="manual">manual</option>
            </select>
          </label>
          <label>
            In{" "}
            <input
              data-testid="record-punch-start"
              type="number"
              min={0}
              value={punchStart()}
              onInput={(e) => setPunchStart(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <label>
            Out{" "}
            <input
              data-testid="record-punch-end"
              type="number"
              value={punchEnd()}
              onInput={(e) => setPunchEnd(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <label>
            Section{" "}
            <select
              data-testid="record-section"
              value={sectionId() ?? ""}
              onInput={(e) => setSectionId(e.currentTarget.value || null)}
            >
              <For each={sections()}>{(s) => <option value={s.id}>{s.name}</option>}</For>
            </select>
          </label>
          <button data-testid="record-use-section" onClick={useSection}>
            Use section
          </button>
        </div>
        <Show when={punchRange().error}>
          <div class="transport-error daw-numeric">{punchRange().error}</div>
        </Show>

        <div class="view-label">Transport</div>
        <div class="session-controls">
          <label>
            Playhead{" "}
            <input
              data-testid="record-playhead"
              type="number"
              min={0}
              value={playhead()}
              onInput={(e) => setPlayhead(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <button data-testid="record-punch-in" onClick={punchIn}>
            Punch in
          </button>
          <button data-testid="record-punch-out" onClick={punchOut}>
            Punch out
          </button>
          <span class={`engine-badge${punching() ? " playing" : ""}`} data-testid="record-punch-state">
            {punching() ? "● punching" : "○ idle"}
          </span>
        </div>

        <div class="view-label">Count-in</div>
        <div class="session-controls">
          <label>
            Bars{" "}
            <input
              data-testid="record-count-bars"
              type="number"
              min={1}
              value={countBars()}
              onInput={(e) => setCountBars(Number(e.currentTarget.value) || 0)}
              style={{ width: "56px" }}
            />
          </label>
          <label>
            Beats/bar{" "}
            <input
              data-testid="record-count-beats"
              type="number"
              min={1}
              value={countBeats()}
              onInput={(e) => setCountBeats(Number(e.currentTarget.value) || 0)}
              style={{ width: "56px" }}
            />
          </label>
          <Show when={countIn().error} fallback={<span class="session-dim daw-numeric">{countIn().text}</span>}>
            <span class="transport-error daw-numeric">{countIn().error}</span>
          </Show>
        </div>

        <div class="view-label">Input</div>
        <div class="session-controls">
          <label>
            Device{" "}
            <select
              data-testid="record-input-device"
              value={inputDevice() ?? ""}
              onInput={(e) => {
                const next = selectInputDevice(
                  inputDevice(),
                  devices(),
                  e.currentTarget.value || null,
                );
                setInputDevice(next);
                setStatus(
                  next === null
                    ? "record: null input (headless-safe)"
                    : `record: input ${devices().find((d) => d.id === next)?.name ?? next}`,
                );
              }}
            >
              <option value="">null input</option>
              <For each={devices()}>{(d) => <option value={d.id}>{d.name}</option>}</For>
            </select>
          </label>
          <span class="session-dim daw-numeric" data-testid="record-input-level">
            {formatLevels(levels())}
          </span>
        </div>

        <div class="view-label">Monitoring &amp; take</div>
        <div class="session-controls">
          <label>
            Monitor{" "}
            <select
              data-testid="record-monitor"
              value={monitor()}
              onInput={(e) => setMonitor(e.currentTarget.value as MonitorMode)}
            >
              <option value="off">off</option>
              <option value="auto">auto</option>
              <option value="on">on</option>
            </select>
          </label>
          <label>
            Kind{" "}
            <select
              data-testid="record-kind"
              value={takeKind()}
              onInput={(e) => setTakeKind(e.currentTarget.value as ClipKind)}
            >
              <option value="Audio">Audio</option>
              <option value="Midi">Midi</option>
            </select>
          </label>
          <button data-testid="record-commit" onClick={() => void commitTake()}>
            Commit take (pass {passCount()})
          </button>
        </div>
        <div class="session-dim" data-testid="record-status">{status()}</div>
      </Show>
    </div>
  );
}
