/**
 * Track S-3: platform loudness-conformance UI model (TypeScript side of
 * `core/src/meter/conform.rs`).
 *
 * The integrated-LUFS / true-peak measurement and the gain itself live in
 * Rust (`meter::export`, re-measured after every conform). This module does
 * the parts that belong near the DOM: the platform preset table the
 * bounce panel offers as one click, the preview gain math (mirrors
 * `gain_for_target`), and the pass/fail verdict + report-file naming the
 * panel shows next to the stems. Nothing here changes the frozen v0/v1
 * schema — all state is UI-local.
 */

import { gainToDb, SILENCE_LUFS, type LoudnessTarget } from "./model";

export const CONFORM_TOLERANCE_LU = 1.0;
export const CONFORM_CEILING_SLACK_DB = 0.1;
export const REPORT_SUFFIX = ".conform.json";

export interface ConformancePreset extends LoudnessTarget {
  id: string;
  label: string;
  source: string;
  /** True when no published platform-holder spec backs the numbers. */
  approximate: boolean;
}

/**
 * Platform preset table. Values mirror `core/src/meter/conform.rs`
 * PRESETS — keep the two in sync when a spec updates (one commit each).
 */
export const CONFORM_PRESETS: readonly ConformancePreset[] = [
  {
    id: "pc",
    label: "PC (desktop convention)",
    targetLufs: -16,
    truePeakCeilingDbfs: -1,
    source: "convention: desktop/streaming finish",
    approximate: true,
  },
  {
    id: "switch",
    label: "Switch (console convention)",
    targetLufs: -24,
    truePeakCeilingDbfs: -1,
    source: "convention: console target (Wwise mastering guidance)",
    approximate: true,
  },
  {
    id: "playstation",
    label: "PlayStation (ASWG-R001)",
    targetLufs: -24,
    truePeakCeilingDbfs: -2,
    source: "Sony ASWG-R001: -24 LKFS, max true peak -2 dBTP",
    approximate: false,
  },
  {
    id: "xbox",
    label: "Xbox (console convention)",
    targetLufs: -24,
    truePeakCeilingDbfs: -1,
    source: "convention: console target (Wwise mastering guidance)",
    approximate: true,
  },
  {
    id: "mobile",
    label: "Mobile (portable convention)",
    targetLufs: -18,
    truePeakCeilingDbfs: -1,
    source: "convention: portable target (Wwise mastering guidance)",
    approximate: true,
  },
];

export function presetById(id: string): ConformancePreset | undefined {
  return CONFORM_PRESETS.find((p) => p.id === id);
}

/**
 * Preview gain for a measured mix (mirrors `meter::export::gain_for_target`).
 * Silence (at/below the absolute gate, or zero peak) is the safe no-op.
 */
export function previewConformGain(
  measuredLufs: number,
  truePeak: number,
  preset: ConformancePreset,
): { gain: number; gainDb: number; limitedByCeiling: boolean } {
  if (!Number.isFinite(measuredLufs) || measuredLufs <= SILENCE_LUFS || truePeak <= 0) {
    return { gain: 1, gainDb: 0, limitedByCeiling: false };
  }
  let gain = Math.pow(10, (preset.targetLufs - measuredLufs) / 20);
  const ceilingLin = Math.pow(10, preset.truePeakCeilingDbfs / 20);
  if (truePeak * gain > ceilingLin) {
    gain = ceilingLin / truePeak;
    return { gain, gainDb: gainToDb(gain), limitedByCeiling: true };
  }
  return { gain, gainDb: gainToDb(gain), limitedByCeiling: false };
}

export interface ConformanceVerdict {
  targetMet: boolean;
  ceilingMet: boolean;
  passed: boolean;
}

/**
 * Pass/fail verdict over re-measured output (mirrors
 * `ConformanceReport::from_measurements`). A ceiling-limited bounce fails
 * the target on purpose: shipping quieter than the preset is a decision,
 * not a pass.
 */
export function conformVerdict(
  preset: ConformancePreset,
  output: { outputLufs: number; outputTruePeakDbfs: number; limitedByCeiling: boolean },
): ConformanceVerdict {
  const targetMet =
    !output.limitedByCeiling &&
    Math.abs(output.outputLufs - preset.targetLufs) <= CONFORM_TOLERANCE_LU;
  const ceilingMet =
    output.outputTruePeakDbfs <= preset.truePeakCeilingDbfs + CONFORM_CEILING_SLACK_DB;
  return { targetMet, ceilingMet, passed: targetMet && ceilingMet };
}

/** Artifact filename written next to the stems: `<stem-name>.conform.json`. */
export function conformReportFilename(stemName: string): string {
  return `${stemName}${REPORT_SUFFIX}`;
}
