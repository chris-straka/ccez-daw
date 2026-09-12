/**
 * Track Q (agent 6): metering UI model (TypeScript side of `core/src/meter/`).
 *
 * The integrated-LUFS measurement lives in Rust (`meter::export`, the
 * BS.1770-style gate the report is re-measured with). This module does the
 * parts that belong near the DOM: per-block peak/RMS reduction for the live
 * bars, dBFS formatting, and the target/ceiling gain math the bounce panel
 * previews before it asks Rust to render. Nothing here changes the frozen
 * v0/v1 schema — all state is UI-local.
 */

export const SILENCE_DB = -120;
export const SILENCE_LUFS = -70;

export function gainToDb(gain: number): number {
  if (!Number.isFinite(gain) || gain <= 0) return SILENCE_DB;
  return 20 * Math.log10(gain);
}

export function dbToGain(db: number): number {
  if (!Number.isFinite(db) || db <= SILENCE_DB) return 0;
  return Math.pow(10, db / 20);
}

export interface MeterReading {
  peak: number;
  peakDb: number;
  rms: number;
  rmsDb: number;
  clipped: boolean;
}

/** Reduce one mono block to a meter reading (same reduction the Rust bridge reports). */
export function meterBlock(samples: ArrayLike<number>): MeterReading {
  if (samples.length === 0) {
    return { peak: 0, peakDb: SILENCE_DB, rms: 0, rmsDb: SILENCE_DB, clipped: false };
  }
  let peak = 0;
  let clipped = false;
  let sum = 0;
  for (let i = 0; i < samples.length; i++) {
    const a = Math.abs(samples[i]);
    if (a > peak) peak = a;
    if (a >= 1) clipped = true;
    sum += samples[i] * samples[i];
  }
  const rms = Math.sqrt(sum / samples.length);
  return { peak, peakDb: gainToDb(peak), rms, rmsDb: gainToDb(rms), clipped };
}

export interface LoudnessTarget {
  targetLufs: number;
  truePeakCeilingDbfs: number;
}

export const DEFAULT_TARGET: LoudnessTarget = { targetLufs: -16, truePeakCeilingDbfs: -1 };

/** Validate a loudness target (mirrors `meter::export::LoudnessTarget::new`). */
export function checkTarget(t: LoudnessTarget): string | null {
  if (!Number.isFinite(t.targetLufs) || t.targetLufs < -70 || t.targetLufs > -4) {
    return `targetLufs ${t.targetLufs} out of [-70, -4]`;
  }
  if (
    !Number.isFinite(t.truePeakCeilingDbfs) ||
    t.truePeakCeilingDbfs < -12 ||
    t.truePeakCeilingDbfs > 0
  ) {
    return `truePeakCeilingDbfs ${t.truePeakCeilingDbfs} out of [-12, 0]`;
  }
  return null;
}

/**
 * Preview gain for a measured mix (mirrors `meter::export::gain_for_target`).
 * Silence (at/below the absolute gate, or zero peak) is the safe no-op.
 */
export function gainForTarget(
  measuredLufs: number,
  truePeak: number,
  target: LoudnessTarget,
): { gain: number; gainDb: number; limitedByCeiling: boolean } {
  if (!Number.isFinite(measuredLufs) || measuredLufs <= SILENCE_LUFS || truePeak <= 0) {
    return { gain: 1, gainDb: 0, limitedByCeiling: false };
  }
  let gain = Math.pow(10, (target.targetLufs - measuredLufs) / 20);
  const ceilingLin = Math.pow(10, target.truePeakCeilingDbfs / 20);
  if (truePeak * gain > ceilingLin) {
    gain = ceilingLin / truePeak;
    return { gain, gainDb: gainToDb(gain), limitedByCeiling: true };
  }
  return { gain, gainDb: gainToDb(gain), limitedByCeiling: false };
}

/** Format LUFS for the readout (silence floor shows as `-70.0`). */
export function formatLufs(lufs: number): string {
  if (!Number.isFinite(lufs) || lufs <= SILENCE_LUFS) return "-70.0 LUFS (silence)";
  return `${lufs.toFixed(1)} LUFS`;
}
