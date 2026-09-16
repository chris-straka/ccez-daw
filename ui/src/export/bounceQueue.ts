/**
 * BOUNCE+EXPORT panel model (pure TS, UI-local).
 *
 * Client-side request planning over the frozen v0/v1 shapes: offline bounce
 * config validation (mirrors `core/src/bounce.rs::BounceConfig::new`),
 * sample-count math at the project tempo, and a batch/package queue whose
 * items carry per-item status. Byte rendering itself stays in core
 * (`bounce` / `batchexport` / `gaexport` / `meter::export`); this module
 * never synthesizes audio, edits contracts, or adds IPC — handoff goes
 * through the existing `project_save` surface and the shared normalize
 * preview in `metering/model`.
 */

export interface BounceConfigInput {
  sampleRate: number;
  startBeat: number;
  lengthBeats: number;
}

export const DEFAULT_BOUNCE_CONFIG: BounceConfigInput = {
  sampleRate: 44100,
  startBeat: 0,
  lengthBeats: 4,
};

/** Render rates the panel offers (CD-adjacent default + game-audio 48 kHz). */
export const SUPPORTED_SAMPLE_RATES = [8000, 44100, 48000] as const;

/**
 * Validate an offline bounce config (mirrors `BounceConfig::new`:
 * rate > 0, finite start >= 0, finite length > 0). Returns null when clean.
 */
export function validateBounceConfig(c: BounceConfigInput): string | null {
  if (!Number.isFinite(c.sampleRate) || c.sampleRate <= 0 || Math.floor(c.sampleRate) !== c.sampleRate) {
    return `sample_rate must be a positive integer, got ${c.sampleRate}`;
  }
  if (!Number.isFinite(c.startBeat) || c.startBeat < 0) {
    return `start_beat must be >= 0, got ${c.startBeat}`;
  }
  if (!Number.isFinite(c.lengthBeats) || c.lengthBeats <= 0) {
    return `length_beats must be positive, got ${c.lengthBeats}`;
  }
  return null;
}

/**
 * Whole-window sample count at `tempo` BPM (mirrors `sample_count`:
 * length * 60 / tempo * rate, rounded). Non-positive tempo → 0.
 */
export function bounceSampleCount(c: BounceConfigInput, tempo: number): number {
  if (!(tempo > 0) || !Number.isFinite(tempo)) return 0;
  return Math.round((c.lengthBeats * 60) / tempo * c.sampleRate);
}

/** One-line summary for queue rows and status lines. */
export function describeBounceConfig(c: BounceConfigInput, tempo: number): string {
  const n = bounceSampleCount(c, tempo);
  return `${c.startBeat}–${c.startBeat + c.lengthBeats} beats @ ${c.sampleRate} Hz (${n} samples @ ${tempo} BPM)`;
}

export type QueueStatus = "queued" | "ready" | "error";

export interface ExportQueueItem {
  id: string;
  kind: "bounce" | "batch-variant" | "package";
  label: string;
  status: QueueStatus;
  detail: string;
}

let queueSeq = 0;

/** Fresh queue id (panel-local; never serialized to the project). */
export function nextQueueId(): string {
  queueSeq += 1;
  return `export_${queueSeq}`;
}

/** Reset the queue id counter (tests). */
export function resetQueueIds(): void {
  queueSeq = 0;
}

/** Enqueue one (cue × state) batch-variant item per pair. */
export function enqueueBatchItems(
  cueIds: string[],
  states: string[],
): ExportQueueItem[] {
  const out: ExportQueueItem[] = [];
  for (const cueId of cueIds) {
    for (const state of states) {
      const id = nextQueueId();
      out.push({
        id,
        kind: "batch-variant",
        label: `${cueId}:${state}`,
        status: "queued",
        detail: "queued",
      });
    }
  }
  return out;
}

/** Enqueue a single offline-bounce item (already validated by the caller). */
export function enqueueBounceItem(summary: string): ExportQueueItem {
  return {
    id: nextQueueId(),
    kind: "bounce",
    label: `bounce ${summary}`,
    status: "queued",
    detail: "queued",
  };
}

/** Enqueue a single game-audio package item. */
export function enqueuePackageItem(name: string): ExportQueueItem {
  return {
    id: nextQueueId(),
    kind: "package",
    label: `package ${name}`,
    status: "queued",
    detail: "queued",
  };
}

/** Immutably set one item's status/detail. */
export function setItemStatus(
  items: ExportQueueItem[],
  id: string,
  status: QueueStatus,
  detail: string,
): ExportQueueItem[] {
  return items.map((it) => (it.id === id ? { ...it, status, detail } : it));
}

/** Queue counts for the header readout. */
export function queueSummary(items: ExportQueueItem[]): {
  queued: number;
  ready: number;
  errors: number;
} {
  return {
    queued: items.filter((i) => i.status === "queued").length,
    ready: items.filter((i) => i.status === "ready").length,
    errors: items.filter((i) => i.status === "error").length,
  };
}

/** Parse a comma/space-separated id list (cue ids, bank ids). */
export function parseIdList(raw: string): string[] {
  return raw
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}
