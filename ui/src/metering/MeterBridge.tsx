import { Show, createEffect, createSignal } from "solid-js";
import {
  DEFAULT_TARGET,
  checkTarget,
  formatLufs,
  gainForTarget,
  meterBlock,
  type LoudnessTarget,
} from "./model";

export type { LoudnessTarget };
export { DEFAULT_TARGET };

/**
 * Track Q (agent 6): meter bridge + loudness-normalized bounce panel.
 *
 * Two halves, one panel. The top half is the live bridge: fed `samples`
 * blocks by the host (e.g. the latest engine render slice), it reduces each
 * to peak/RMS bars via `meterBlock` with a decaying peak-hold and a clip lamp.
 * (Realtime taps live in `core/src/meter/` — this panel only observes.) The bottom
 * half is the bounce target: integrated LUFS + true-peak ceiling inputs, a
 * pre-render gain preview from the last measured mix, and one
 * `onNormalize` button that hands the target to Rust (`normalize_mix_to_target`),
 * which re-measures after applying the gain — the report carries proof.
 *
 * Props are values, not a store: the host owns transport/render state and
 * pushes blocks in; `measuredLufs`/`measuredTruePeak` are the Rust-side
 * measurement of the current mix (or null before the first measure).
 */
export default function MeterBridge(props: {
  onNormalize: (target: LoudnessTarget) => void;
  /** Latest render slice to feed the live bars (host passes a new array per slice). */
  samples?: ArrayLike<number> | null;
  measuredLufs?: number | null;
  measuredTruePeak?: number | null;
  lastGainDb?: number | null;
  lastLimited?: boolean | null;
}) {
  const [peak, setPeak] = createSignal(0);
  const [rmsDb, setRmsDb] = createSignal(-120);
  const [hold, setHold] = createSignal(0);
  const [clipped, setClipped] = createSignal(false);
  const [targetLufs, setTargetLufs] = createSignal(DEFAULT_TARGET.targetLufs);
  const [ceiling, setCeiling] = createSignal(DEFAULT_TARGET.truePeakCeilingDbfs);
  const [note, setNote] = createSignal("");

  /** Feed one mono block into the live bars (host calls per render slice). */
  function observe(samples: ArrayLike<number>) {
    const r = meterBlock(samples);
    setPeak(r.peak);
    setRmsDb(r.rmsDb);
    setHold((h) => Math.max(r.peak, h * 0.9));
    if (r.clipped) setClipped(true);
  }

  function clearClip() {
    setClipped(false);
    setNote("");
  }

  // The host pushes render slices through the `samples` prop; every new
  // array re-runs this effect and folds one block into the bars.
  createEffect(() => {
    const s = props.samples;
    if (s && s.length > 0) observe(s);
  });

  const target = (): LoudnessTarget => ({ targetLufs: targetLufs(), truePeakCeilingDbfs: ceiling() });

  const preview = () => {
    const err = checkTarget(target());
    if (err) return err;
    if (props.measuredLufs == null || props.measuredTruePeak == null) {
      return "Render the mix once to measure it, then preview the gain.";
    }
    const p = gainForTarget(props.measuredLufs, props.measuredTruePeak, target());
    return `Preview: ${p.gainDb >= 0 ? "+" : ""}${p.gainDb.toFixed(1)} dB${
      p.limitedByCeiling ? " (ceiling-limited)" : ""
    } → ${target().targetLufs.toFixed(1)} LUFS under ${target().truePeakCeilingDbfs.toFixed(1)} dBFS peak.`;
  };

  function normalize() {
    const t = target();
    const err = checkTarget(t);
    if (err) {
      setNote(err);
      return;
    }
    setNote("");
    props.onNormalize(t);
  }

  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
      <div style={{ display: "flex", gap: "8px", "align-items": "center" }}>
        <div
          title={`peak ${(20 * Math.log10(Math.max(peak(), 1e-6))).toFixed(1)} dBFS`}
          style={{
            width: "160px",
            height: "10px",
            background: "#111",
            border: "1px solid #555",
            position: "relative",
          }}
        >
          <div
            style={{
              width: `${Math.min(100, peak() * 100)}%`,
              height: "100%",
              background: clipped() ? "#c33" : "#4c4",
            }}
          />
          <div
            style={{
              position: "absolute",
              left: `${Math.min(100, hold() * 100)}%`,
              top: "0",
              bottom: "0",
              width: "2px",
              background: "#ff4",
            }}
          />
        </div>
        <span style={{ "font-size": "11px", color: "#888" }}>
          peak {peak().toFixed(2)} · rms {rmsDb().toFixed(1)} dBFS · hold {hold().toFixed(2)}
        </span>
        <Show when={clipped()}>
          <button onClick={clearClip} style={{ background: "#a33" }} title="A block hit full scale">
            CLIP — clear
          </button>
        </Show>
      </div>
      <Show when={props.measuredLufs != null}>
        <div style={{ "font-size": "12px", color: "#8cf" }}>
          Mix measures {formatLufs(props.measuredLufs as number)}
          {props.lastGainDb != null ? (
            <span>
              {" "}· applied {props.lastGainDb >= 0 ? "+" : ""}
              {props.lastGainDb.toFixed(1)} dB{props.lastLimited ? " (ceiling-limited)" : ""}
            </span>
          ) : null}
        </div>
      </Show>
      <div style={{ display: "flex", gap: "8px", "align-items": "center", "flex-wrap": "wrap" }}>
        <label style={{ "font-size": "11px" }}>
          target (LUFS)
          <input
            type="number"
            min="-70"
            max="-4"
            step="0.5"
            value={targetLufs()}
            onInput={(e) => setTargetLufs(Number(e.target.value))}
            style={{ width: "64px" }}
          />
        </label>
        <label style={{ "font-size": "11px" }}>
          true-peak ceiling (dBFS)
          <input
            type="number"
            min="-12"
            max="0"
            step="0.5"
            value={ceiling()}
            onInput={(e) => setCeiling(Number(e.target.value))}
            style={{ width: "64px" }}
          />
        </label>
        <button onClick={normalize}>Normalize bounce</button>
        <span style={{ "font-size": "11px", color: "#888" }}>{preview()}</span>
        <Show when={note()}>
          <span style={{ color: "#f88", "font-size": "11px" }}>{note()}</span>
        </Show>
      </div>
    </div>
  );
}

