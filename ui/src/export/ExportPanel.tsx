import { For, Show, createMemo, createSignal } from "solid-js";
import type { Project } from "../generated/project";
import MeterBridge, { type LoudnessTarget } from "../metering/MeterBridge";
import { checkTarget, gainForTarget } from "../metering/model";
import {
  DEFAULT_BOUNCE_CONFIG,
  describeBounceConfig,
  enqueueBounceItem,
  queueSummary,
  setItemStatus,
  validateBounceConfig,
  type ExportQueueItem,
} from "./bounceQueue";

/**
 * BOUNCE+EXPORT panel: offline bounce + loudness normalization queue.
 *
 * - Offline bounce: range + sample-rate config (validated like
 *   `core/src/bounce.rs::BounceConfig::new`) with a `MeterBridge` bottom
 *   half for normalize-to-target through the existing normalize path
 *   (`checkTarget` / `gainForTarget` preview, `onNormalize` commit).
 * - Queue: tracks staged normalized bounces with per-item status.
 */
export default function ExportPanel(props: {
  project: Project | null;
}) {
  // -- offline bounce -------------------------------------------------------
  const [startBeat, setStartBeat] = createSignal(DEFAULT_BOUNCE_CONFIG.startBeat);
  const [lengthBeats, setLengthBeats] = createSignal(DEFAULT_BOUNCE_CONFIG.lengthBeats);
  const [sampleRate, setSampleRate] = createSignal(DEFAULT_BOUNCE_CONFIG.sampleRate);
  const [measuredLufs, setMeasuredLufs] = createSignal<number | null>(null);
  const [measuredPeak, setMeasuredPeak] = createSignal<number | null>(null);
  const [lastGainDb, setLastGainDb] = createSignal<number | null>(null);
  const [lastLimited, setLastLimited] = createSignal<boolean | null>(null);
  const [bounceStatus, setBounceStatus] = createSignal("bounce: set a range, then normalize");

  const tempo = () => props.project?.tempo ?? 120;

  const bounceError = createMemo(() =>
    validateBounceConfig({ sampleRate: sampleRate(), startBeat: startBeat(), lengthBeats: lengthBeats() }),
  );

  const bounceSummary = createMemo(() => {
    if (bounceError()) return bounceError() as string;
    return describeBounceConfig(
      { sampleRate: sampleRate(), startBeat: startBeat(), lengthBeats: lengthBeats() },
      tempo(),
    );
  });

  function onNormalize(target: LoudnessTarget) {
    // The range config gates everything: normalizing an invalid range
    // would enqueue an item named after the validation error.
    const cfgErr = bounceError();
    if (cfgErr) {
      setBounceStatus(`normalize refused: ${cfgErr}`);
      return;
    }
    const err = checkTarget(target);
    if (err) {
      setBounceStatus(`normalize refused: ${err}`);
      return;
    }
    if (measuredLufs() == null || measuredPeak() == null) {
      setBounceStatus("normalize needs a measured mix: enter measured LUFS + true peak first");
      return;
    }
    const p = gainForTarget(measuredLufs() as number, measuredPeak() as number, target);
    setLastGainDb(p.gainDb);
    setLastLimited(p.limitedByCeiling);
    setItems((prev) => [
      ...prev,
      {
        ...enqueueBounceItem(bounceSummary()),
        status: "ready",
        detail:
          `normalized ${p.gainDb >= 0 ? "+" : ""}${p.gainDb.toFixed(1)} dB` +
          ` → ${target.targetLufs.toFixed(1)} LUFS` +
          (p.limitedByCeiling ? " (ceiling-limited)" : ""),
      },
    ]);
    setBounceStatus(
      `normalized ${p.gainDb >= 0 ? "+" : ""}${p.gainDb.toFixed(1)} dB → ${target.targetLufs.toFixed(1)} LUFS` +
        (p.limitedByCeiling ? " (ceiling-limited)" : ""),
    );
  }

  // -- shared queue ----------------------------------------------------------
  const [items, setItems] = createSignal<ExportQueueItem[]>([]);
  const summary = createMemo(() => queueSummary(items()));

  function clearQueue() {
    setItems([]);
  }

  return (
    <div data-testid="export-panel" class="session-view">
      <Show when={!props.project} fallback={null}>
        <div class="session-dim">No project open — bounce math uses 120 BPM until one loads.</div>
      </Show>

      <div>
        <div class="view-label">Offline bounce</div>
        <div class="export-grid">
          <label class="export-field">
            <span>start (beats)</span>
            <input
              data-testid="export-bounce-start"
              class="daw-numeric"
              type="number"
              min="0"
              step="0.5"
              value={startBeat()}
              onInput={(e) => setStartBeat(Number(e.target.value))}
            />
          </label>
          <label class="export-field">
            <span>length (beats)</span>
            <input
              data-testid="export-bounce-length"
              class="daw-numeric"
              type="number"
              min="0"
              step="0.5"
              value={lengthBeats()}
              onInput={(e) => setLengthBeats(Number(e.target.value))}
            />
          </label>
          <label class="export-field">
            <span>sample rate (Hz)</span>
            <input
              data-testid="export-bounce-rate"
              class="daw-numeric"
              type="number"
              min="1"
              step="100"
              value={sampleRate()}
              onInput={(e) => setSampleRate(Number(e.target.value))}
            />
          </label>
          <label class="export-field">
            <span>measured LUFS</span>
            <input
              data-testid="export-measured-lufs"
              class="daw-numeric"
              type="number"
              step="0.5"
              placeholder="e.g. -22"
              value={measuredLufs() ?? ""}
              onInput={(e) => setMeasuredLufs(e.target.value === "" ? null : Number(e.target.value))}
            />
          </label>
          <label class="export-field">
            <span>measured true peak</span>
            <input
              data-testid="export-measured-peak"
              class="daw-numeric"
              type="number"
              step="0.01"
              placeholder="e.g. 0.25"
              value={measuredPeak() ?? ""}
              onInput={(e) => setMeasuredPeak(e.target.value === "" ? null : Number(e.target.value))}
            />
          </label>
          <span data-testid="export-bounce-summary" class="session-dim daw-numeric export-summary">
            {bounceSummary()}
          </span>
        </div>
        <MeterBridge
          onNormalize={onNormalize}
          measuredLufs={measuredLufs()}
          measuredTruePeak={measuredPeak()}
          lastGainDb={lastGainDb()}
          lastLimited={lastLimited()}
        />
        <span data-testid="export-bounce-status" class="session-dim">
          {bounceStatus()}
        </span>
      </div>

      <div>
        <div class="view-label daw-numeric">
          Queue ({items().length}: {summary().queued} queued · {summary().ready} ready · {summary().errors} errors)
        </div>
        <Show when={items().length > 0} fallback={<div class="session-dim">Queue is empty.</div>}>
          <For each={items()}>
            {(it) => (
              <div class="lane" data-testid={`export-item-${it.id}`}>
                <span class="lane-name">{it.kind}</span>
                <span
                  class="clip-chip"
                  classList={{ "clip-audio": it.status === "ready", "clip-midi": it.status !== "ready" }}
                  title={it.detail}
                >
                  {it.label}
                </span>
                <span
                  class="export-status"
                  classList={{ ready: it.status === "ready", error: it.status === "error" }}
                  data-testid={`export-item-status-${it.id}`}
                >
                  {it.status}: {it.detail}
                </span>
                <Show when={it.status === "error"}>
                  <button data-testid={`export-requeue-${it.id}`}
                    class="slot-btn silent"
                    onClick={() => setItems((prev) => setItemStatus(prev, it.id, "queued", "re-queued"))}
                  >
                    re-queue
                  </button>
                </Show>
              </div>
            )}
          </For>
          <div class="session-controls">
            <button data-testid="export-queue-clear" onClick={clearQueue}>
              Clear queue
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
}
