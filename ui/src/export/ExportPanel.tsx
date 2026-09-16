import { For, Show, createMemo, createSignal } from "solid-js";
import type { Project } from "../generated/project";
import { project_save } from "../tauri/commands";
import MeterBridge, { type LoudnessTarget } from "../metering/MeterBridge";
import { checkTarget, gainForTarget } from "../metering/model";
import {
  BATCH_STATES,
  audibleLayerIds,
  expectedVariants,
  variantPath,
} from "../gameaudio/batch";
import {
  estimatePackage,
  previewPackage,
  stemSecondsFromBeats,
} from "../gameaudio/export";
import {
  FIXTURE_BANK_ID,
  FIXTURE_CUE_IDS,
  FIXTURE_PACKAGE_NAME,
  fixtureBank,
  fixtureClipIds,
  fixtureCues,
  validateFixture,
} from "../gameaudio/fixture";
import type { ExportPackage } from "../generated/project";
import {
  DEFAULT_BOUNCE_CONFIG,
  describeBounceConfig,
  enqueueBatchItems,
  enqueueBounceItem,
  enqueuePackageItem,
  parseIdList,
  queueSummary,
  setItemStatus,
  validateBounceConfig,
  type ExportQueueItem,
} from "./bounceQueue";

/**
 * BOUNCE+EXPORT panel: the user-facing export workflow.
 *
 * Three sections over frozen shapes, no new IPC:
 *
 * - Offline bounce: range + sample-rate config (validated like
 *   `core/src/bounce.rs::BounceConfig::new`) with a `MeterBridge` bottom
 *   half for normalize-to-target through the existing normalize path
 *   (`checkTarget` / `gainForTarget` preview, `onNormalize` commit).
 * - Batch queue: one item per (cue × state) with per-item status; items
 *   resolve their audible layer set via `gameaudio/batch` (the same
 *   state-gate math `core/src/batchexport.rs` renders from).
 * - Game-audio package: plan + trigger over the fixture cues/bank with
 *   `validateFixture` / `previewPackage` / `estimatePackage`; the trigger
 *   stages the request through the existing `project_save` surface (the
 *   frozen table carries no export command, so bytes keep rendering in
 *   core from the same request shape).
 *
 * Pass `saveProject` to override the persistence transport (tests).
 */
export default function ExportPanel(props: {
  project: Project | null;
  saveProject?: (path: string) => Promise<unknown>;
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

  // -- batch export -----------------------------------------------------------
  const [cueIdsRaw, setCueIdsRaw] = createSignal([...FIXTURE_CUE_IDS].join(", "));
  const [batchStates, setBatchStates] = createSignal<string[]>([...BATCH_STATES]);
  const [batchStatus, setBatchStatus] = createSignal("batch: enqueue one mix per cue × state");

  function toggleBatchState(state: string) {
    setBatchStates((prev) =>
      prev.includes(state) ? prev.filter((s) => s !== state) : [...prev, state],
    );
  }

  function enqueueBatch() {
    const cueIds = parseIdList(cueIdsRaw());
    const states = batchStates();
    if (cueIds.length === 0) {
      setBatchStatus("batch needs at least one cue id");
      return;
    }
    if (states.length === 0) {
      setBatchStatus("batch needs at least one state");
      return;
    }
    const expected = expectedVariants(cueIds, states);
    if (expected.length === 0) {
      setBatchStatus("batch needs at least one cue × state pair");
      return;
    }
    setItems((prev) => [...prev, ...enqueueBatchItems(cueIds, states)]);
    setBatchStatus(`enqueued ${expected.length} variant${expected.length === 1 ? "" : "s"}`);
  }

  /** Resolve every queued batch-variant against the fixture cues' state gates. */
  function processBatchQueue() {
    const cues = fixtureCues();
    const byId = new Map(cues.map((c) => [c.id, c]));
    setItems((prev) =>
      prev.map((it) => {
        if (it.kind !== "batch-variant" || it.status !== "queued") return it;
        const [cueId, state] = it.label.split(":");
        const cue = byId.get(cueId);
        if (!cue) return { ...it, status: "error" as const, detail: `unknown cue \`${cueId}\`` };
        const layers = audibleLayerIds(cue, state);
        const want = variantPath(cueId, state);
        return {
          ...it,
          status: "ready" as const,
          detail: `${want} · layers [${layers.join(", ") || "silent"}]`,
        };
      }),
    );
    setBatchStatus("batch queue processed against cue state gates");
  }

  // -- game-audio package ------------------------------------------------------
  const [packageName, setPackageName] = createSignal<string>(FIXTURE_PACKAGE_NAME);
  const [packageCuesRaw, setPackageCuesRaw] = createSignal([...FIXTURE_CUE_IDS].join(", "));
  const [packageBankRaw, setPackageBankRaw] = createSignal<string>(FIXTURE_BANK_ID);
  const [packageStatus, setPackageStatus] = createSignal("package: plan, then stage the export");

  const save = (path: string): Promise<unknown> =>
    props.saveProject ? props.saveProject(path) : project_save({ path });

  const packagePlan = createMemo(() => {
    const cues = fixtureCues();
    const bank = fixtureBank();
    const cueIds = parseIdList(packageCuesRaw());
    const bankIds = parseIdList(packageBankRaw());
    const projectClips = new Set((props.project?.clips ?? []).map((c) => c.id));
    const problems = validateFixture(cues, bank, undefined, {
      clipIds: [...new Set([...fixtureClipIds(), ...projectClips])],
    });
    const cueById = new Map(cues.map((c) => [c.id, c]));
    const stems: ExportPackage["stems"] = [];
    for (const id of cueIds) {
      const cue = cueById.get(id);
      if (!cue) continue;
      for (const layer of cue.layers) {
        stems.push({
          path: `stems/${id}_${layer.id}.wav`,
          source_id: id,
          source_layer_id: layer.id,
          kind: "MusicLayer",
          loop_start_beats: 0,
          loop_end_beats: 4,
        });
      }
    }
    for (const event of bank.events) {
      event.clip_ids.forEach((_clip, n) => {
        stems.push({
          path: `sfx/${event.id}_${n}.wav`,
          source_id: event.id,
          source_layer_id: "",
          kind: "SfxClip",
          loop_start_beats: 0,
          loop_end_beats: 0,
        });
      });
    }
    const pkg: ExportPackage = {
      schema_version: 1,
      name: packageName(),
      cue_ids: cueIds,
      bank_ids: bankIds,
      stems,
      event_bank_path: bankIds.length > 0 ? `bank_${bankIds[0]}.json` : "",
      validator_version: "1",
    };
    const preview = previewPackage(pkg, [bank]);
    const durations: Record<string, number> = {};
    for (const s of stems) {
      const cue = cueById.get(s.source_id);
      durations[s.path] = s.kind === "MusicLayer" && cue
        ? stemSecondsFromBeats(s, cue.tempo)
        : 1.0;
    }
    const estimate = estimatePackage(pkg, [bank], durations);
    return { problems, preview, estimate, cueIds, bankIds };
  });

  function stagePackage() {
    const plan = packagePlan();
    if (plan.problems.length > 0) {
      setPackageStatus(`package blocked: ${plan.problems.length} fixture problem${plan.problems.length === 1 ? "" : "s"} — fix the bank first`);
      return;
    }
    if (plan.cueIds.length === 0) {
      setPackageStatus("package needs at least one cue id");
      return;
    }
    const item = {
      ...enqueuePackageItem(plan.preview.name),
      status: "ready" as const,
      detail: `${plan.preview.counts.stems} stems · ${plan.estimate.totalHuman} on disk`,
    };
    setItems((prev) => [...prev, item]);
    setPackageStatus(`staged ${plan.preview.name}: ${plan.preview.counts.stems} stems, ${plan.estimate.totalHuman}`);
  }

  async function saveThenStage() {
    try {
      await save(`${packageName()}.ccez`);
      stagePackage();
    } catch (e) {
      setPackageStatus(`save failed: ${String(e)}`);
    }
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
        <div class="view-label">Batch export queue</div>
        <div class="session-controls">
          <input
            data-testid="export-batch-cues"
            value={cueIdsRaw()}
            onInput={(e) => setCueIdsRaw(e.currentTarget.value)}
            placeholder="cue ids, comma-separated"
            style={{ width: "220px" }}
          />
          <For each={[...BATCH_STATES]}>
            {(s) => (
              <button
                data-testid={`export-batch-state-${s}`}
                class="state-btn"
                classList={{ active: batchStates().includes(s) }}
                onClick={() => toggleBatchState(s)}
              >
                {s}
              </button>
            )}
          </For>
          <button data-testid="export-batch-enqueue" onClick={enqueueBatch}>
            Enqueue batch
          </button>
          <button data-testid="export-batch-process" onClick={processBatchQueue}>
            Process queue
          </button>
        </div>
        <span data-testid="export-batch-status" class="session-dim">
          {batchStatus()}
        </span>
      </div>

      <div>
        <div class="view-label">Game-audio package</div>
        <div class="session-controls">
          <input
            data-testid="export-package-name"
            value={packageName()}
            onInput={(e) => setPackageName(e.currentTarget.value)}
            placeholder="package name"
            style={{ width: "160px" }}
          />
          <input
            data-testid="export-package-cues"
            value={packageCuesRaw()}
            onInput={(e) => setPackageCuesRaw(e.currentTarget.value)}
            placeholder="cue ids"
            style={{ width: "220px" }}
          />
          <input
            data-testid="export-package-banks"
            value={packageBankRaw()}
            onInput={(e) => setPackageBankRaw(e.currentTarget.value)}
            placeholder="bank ids"
            style={{ width: "140px" }}
          />
          <button data-testid="export-package-stage" onClick={saveThenStage}>
            Save project &amp; stage export
          </button>
        </div>
        <div class="session-dim daw-numeric" data-testid="export-package-preview">
          {packagePlan().preview.counts.stems} stems · {packagePlan().preview.counts.cues} cues ·{" "}
          {packagePlan().preview.counts.banks} banks · {packagePlan().estimate.totalHuman} on disk
        </div>
        <Show when={packagePlan().problems.length > 0}>
          <div class="export-problems" data-testid="export-package-problems">
            <For each={packagePlan().problems}>
              {(p) => <div class="meter-note">{p}</div>}
            </For>
          </div>
        </Show>
        <span data-testid="export-package-status" class="session-dim">
          {packageStatus()}
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
                  <button
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
