// Export preview + profiler (GA-4, agent 2).
//
// The engine deliverable is a directory (`contracts/export-package.md`):
// rendered WAV stems plus verbatim `bank_<id>.json` files plus a
// `package.json` manifest. This module is a pure view over the frozen v1
// shapes — it never edits contracts or generated code. Three jobs:
//
// - `previewPackage`: what is inside this package? (contents listing)
// - `estimatePackage`: how big is it on disk / in memory? (size estimate)
// - `profileVoices`: will it fit the voice budget? (profiler over audition logs)
//
// Renders are 48 kHz / 24-bit WAV (the game-audio default in
// `contracts/export-package.md`); every byte estimate below derives from
// that, not from a magic constant.

import type { ExportPackage, ExportStem, SfxBank } from "../generated/project";

/** Game-audio render format: 48 kHz / 24-bit WAV (see export-package.md). */
export const EXPORT_SAMPLE_RATE_HZ = 48_000;
/** 24-bit = 3 bytes per sample per channel. */
export const EXPORT_BYTES_PER_SAMPLE = 3;
/** Canonical WAV header size; kept separate so header math stays visible. */
export const WAV_HEADER_BYTES = 44;

/** One row of the contents preview. */
export interface StemPreviewRow {
  path: string;
  kind: string;
  /** Human source: cue layer (`cue/layer`) or event (`event`). */
  source: string;
  /** `"one-shot"` for `0, 0`, else `"loop <start>–<end> beats"`. */
  loop: string;
}

/** What is inside the package: stems, bank files, counts. */
export interface PackagePreview {
  name: string;
  stemRows: StemPreviewRow[];
  /** Verbatim bank files the package ships: `bank_<id>.json` per bank id. */
  bankFiles: string[];
  manifestPath: string;
  counts: { stems: number; cues: number; banks: number; events: number };
}

/** Loop label for one stem: `0, 0` is a one-shot, anything else a loop. */
export function stemLoopLabel(stem: ExportStem): string {
  if (stem.loop_start_beats === 0 && stem.loop_end_beats === 0) return "one-shot";
  return `loop ${stem.loop_start_beats}\u2013${stem.loop_end_beats} beats`;
}

/** Human source for one stem: `cue/layer` for music, `event` for SFX. */
export function stemSource(stem: ExportStem): string {
  if (stem.kind === "MusicLayer") return `${stem.source_id}/${stem.source_layer_id}`;
  return stem.source_id;
}

/** Contents preview: stems, bank files, counts. Pure view, no I/O. */
export function previewPackage(pkg: ExportPackage, banks: SfxBank[]): PackagePreview {
  const events = new Map(banks.map((b) => [b.id, b.events.length]));
  return {
    name: pkg.name,
    stemRows: pkg.stems.map((s) => ({
      path: s.path,
      kind: s.kind,
      source: stemSource(s),
      loop: stemLoopLabel(s),
    })),
    bankFiles: pkg.bank_ids.map((id) => `bank_${id}.json`),
    manifestPath: "package.json",
    counts: {
      stems: pkg.stems.length,
      cues: pkg.cue_ids.length,
      banks: pkg.bank_ids.length,
      events: pkg.bank_ids.reduce((n, id) => n + (events.get(id) ?? 0), 0),
    },
  };
}

/** Estimated disk bytes for one WAV stem: header + PCM at the export format. */
export function estimateWavBytes(seconds: number, channels = 1): number {
  const s = Math.max(0, seconds);
  const ch = Math.max(1, Math.floor(channels));
  return WAV_HEADER_BYTES + Math.ceil(s * EXPORT_SAMPLE_RATE_HZ) * EXPORT_BYTES_PER_SAMPLE * ch;
}

/** Beats of loop range at a cue tempo → seconds. One-shots (`0, 0`) carry
 *  no beat length, so they read as 0 here — pass real durations instead. */
export function stemSecondsFromBeats(stem: ExportStem, tempoBpm: number): number {
  if (stem.loop_start_beats === 0 && stem.loop_end_beats === 0) return 0;
  if (!(tempoBpm > 0)) return 0;
  const beats = stem.loop_end_beats - stem.loop_start_beats;
  return Math.max(0, beats) * (60 / tempoBpm);
}

/** One stem's share of the estimate. */
export interface StemEstimate {
  path: string;
  seconds: number;
  bytes: number;
}

/** Disk + memory estimate for a whole package. */
export interface PackageEstimate {
  stems: StemEstimate[];
  stemBytes: number;
  /** Verbatim `bank_<id>.json` bytes (UTF-8 length of the serialized bank). */
  bankBytes: number;
  /** `package.json` manifest bytes (UTF-8 length of the serialized manifest). */
  manifestBytes: number;
  totalBytes: number;
  /** Rough engine RSS for the package: stems decoded to float32 mono plus
   *  ~2x bank JSON as parsed heap. Documented heuristic, not a promise. */
  memoryBytes: number;
  totalHuman: string;
  memoryHuman: string;
}

/**
 * Estimate a package. Stem durations come from the caller (loop stems can
 * use `stemSecondsFromBeats` with the cue tempo; one-shots need measured
 * clip lengths — beats say nothing about a one-shot). `channels` is the
 * render channel count (mono in v1).
 */
export function estimatePackage(
  pkg: ExportPackage,
  banks: SfxBank[],
  stemDurations: Record<string, number>,
  channels = 1,
): PackageEstimate {
  const stems = pkg.stems.map((s) => {
    const seconds = Math.max(0, stemDurations[s.path] ?? 0);
    return { path: s.path, seconds, bytes: estimateWavBytes(seconds, channels) };
  });
  const stemBytes = stems.reduce((n, s) => n + s.bytes, 0);
  const bankBytes = pkg.bank_ids.reduce((n, id) => {
    const bank = banks.find((b) => b.id === id);
    const json = bank ? JSON.stringify(bank) : "null";
    return n + new TextEncoder().encode(json).length;
  }, 0);
  const manifestBytes = new TextEncoder().encode(JSON.stringify(pkg)).length;
  const totalBytes = stemBytes + bankBytes + manifestBytes;
  const decodedStems = stems.reduce(
    (n, s) => n + Math.ceil(s.seconds * EXPORT_SAMPLE_RATE_HZ) * 4 * Math.max(1, Math.floor(channels)),
    0,
  );
  const memoryBytes = decodedStems + bankBytes * 2;
  return {
    stems,
    stemBytes,
    bankBytes,
    manifestBytes,
    totalBytes,
    memoryBytes,
    totalHuman: formatBytes(totalBytes),
    memoryHuman: formatBytes(memoryBytes),
  };
}

/** Human byte count (`1.5 MiB`); bytes stay exact underneath. */
export function formatBytes(bytes: number): string {
  const n = Math.max(0, Math.floor(bytes));
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let v = n / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v.toFixed(1)} ${units[u]}`;
}

/** One audition-log entry: an accepted trigger plus how long it sounds.
 *  (Mirrors the GA-2 `Voice`: `started_ms` onset, `durationMs` audible
 *  length — audition renders are 1 s by default, so cap there unless the
 *  caller measured a real envelope.) */
export interface AuditionLogEntry {
  eventId: string;
  startedMs: number;
  durationMs: number;
}

/** Voice-budget report from an audition log. */
export interface VoiceProfile {
  /** Peak simultaneous voices across all events. */
  peakTotal: number;
  /** Peak simultaneous voices per event id. */
  peakByEvent: Record<string, number>;
  /** Events whose peak exceeds their `max_polyphony` (engine would steal). */
  overPolyphony: { eventId: string; peak: number; max: number }[];
}

/**
 * Sweep-line profiler over audition logs. Ends are exclusive: a voice
 * ending exactly when another starts does not count as overlap (matching
 * the runtime, which steals the *oldest* live voice). Zero/negative
 * durations never sound, so they are skipped.
 */
export function profileVoices(
  log: AuditionLogEntry[],
  polyphonyByEvent: Record<string, number> = {},
): VoiceProfile {
  type Edge = { at: number; delta: number; eventId: string };
  const edges: Edge[] = [];
  for (const e of log) {
    if (!(e.durationMs > 0)) continue;
    edges.push({ at: e.startedMs, delta: 1, eventId: e.eventId });
    edges.push({ at: e.startedMs + e.durationMs, delta: -1, eventId: e.eventId });
  }
  // Ends (-1) sort before starts (+1) at the same timestamp: exclusive ends.
  edges.sort((a, b) => a.at - b.at || a.delta - b.delta);
  let live = 0;
  let peakTotal = 0;
  const liveBy = new Map<string, number>();
  const peakBy = new Map<string, number>();
  for (const e of edges) {
    live += e.delta;
    const lv = (liveBy.get(e.eventId) ?? 0) + e.delta;
    liveBy.set(e.eventId, lv);
    peakTotal = Math.max(peakTotal, live);
    peakBy.set(e.eventId, Math.max(peakBy.get(e.eventId) ?? 0, lv));
  }
  const peakByEvent: Record<string, number> = {};
  for (const [k, v] of peakBy) peakByEvent[k] = v;
  const overPolyphony = Object.entries(polyphonyByEvent)
    .filter(([id, max]) => (peakBy.get(id) ?? 0) > max)
    .map(([id, max]) => ({ eventId: id, peak: peakBy.get(id) ?? 0, max }))
    .sort((a, b) => a.eventId.localeCompare(b.eventId));
  return { peakTotal, peakByEvent, overPolyphony };
}
