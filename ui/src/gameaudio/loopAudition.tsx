import { createMemo, createSignal, onCleanup } from "solid-js";
import type { AdaptiveCue } from "../generated/project";
import {
  DEFAULT_CLICK_THRESHOLD,
  analyzeSeam,
  applyEdgeFade,
  clickErrorText,
  synthesizeLoopTone,
  waiverFileJson,
  wrapBeatIntoLoop,
  type LoopWaiver,
} from "./loop";

/**
 * S-2: gapless loop preview for any cue in the simulator.
 *
 * Pick a layer, press Play, and the preview tone loops through a real
 * `AudioBufferSourceNode` with `loop = true` — the same wrap the game
 * performs — while the seam meter shows the boundary step against the
 * ship-gate threshold. A click offers the same two exits as the validator:
 * Fix (edge-fade the preview and re-measure) or Waive (record a reasoned
 * entry for `loop-waivers.json`, shown below as copy-ready JSON).
 *
 * The preview tone is a whole-cycle stand-in, not the mix: this panel
 * auditions the *seam*, not the arrangement. Real stem bytes are judged by
 * `scripts/validate-export.sh` (rule 6).
 */
export default function LoopAudition(props: {
  cue: AdaptiveCue;
  sampleRate?: number;
}) {
  const sr = () => props.sampleRate ?? 48_000;
  // 2 s of preview at the render rate, pitched like the export reference
  // tone (220 Hz at 60 BPM, scaled by the cue tempo). The raw cut clicks at
  // the endpoint by one sample of slope — press Fix to hear/measure the
  // edge-faded version the gate approves.
  const previewLen = () => sr() * 2;
  const previewCycles = () => Math.max(1, Math.round(220 * (props.cue.tempo / 60) * (previewLen() / sr())));
  const [layerId, setLayerId] = createSignal(props.cue.layers[0]?.id ?? "");
  const [threshold, setThreshold] = createSignal(DEFAULT_CLICK_THRESHOLD);
  const [fixed, setFixed] = createSignal(false);
  const [playing, setPlaying] = createSignal(false);
  const [waivers, setWaivers] = createSignal<LoopWaiver[]>([]);
  const [reason, setReason] = createSignal("");

  let ctx: AudioContext | null = null;
  let src: AudioBufferSourceNode | null = null;

  function stop() {
    try {
      src?.stop();
    } catch {
      /* already stopped */
    }
    src = null;
    setPlaying(false);
  }

  onCleanup(() => {
    stop();
    void ctx?.close();
    ctx = null;
  });

  /** Current preview buffer: raw cut, or edge-faded after Fix. */
  const buffer = createMemo(() => {
    const tone = synthesizeLoopTone(previewLen(), previewCycles(), 0.5);
    return fixed() ? applyEdgeFade(tone, Math.floor(0.005 * sr())) : tone;
  });

  const report = createMemo(() => analyzeSeam(buffer(), threshold()));

  /** Audition-clock demo: where beat 9.5 of a 0–8 loop actually plays. */
  const wrapped = createMemo(() => wrapBeatIntoLoop(9.5, 0, 8));

  function toggle() {
    if (playing()) {
      stop();
      return;
    }
    if (!ctx) ctx = new AudioContext({ sampleRate: sr() });
    void ctx.resume();
    const data = buffer();
    const ab = ctx.createBuffer(1, data.length, sr());
    ab.getChannelData(0).set(data);
    src = ctx.createBufferSource();
    src.buffer = ab;
    src.loop = true;
    src.connect(ctx.destination);
    src.start();
    setPlaying(true);
  }

  function waive() {
    const r = reason().trim();
    if (r.length === 0) return;
    const path = `stems/${props.cue.id}_${layerId()}.wav`;
    setWaivers((prev) =>
      prev.some((w) => w.path === path) ? prev : [...prev, { path, reason: r }],
    );
    setReason("");
  }

  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
      <div style={{ display: "flex", gap: "6px", "align-items": "center", "flex-wrap": "wrap" }}>
        <span style={{ "font-size": "12px", color: "#aaa" }}>Loop layer:</span>
        {props.cue.layers.map((l) => (
          <button
            style={{ background: layerId() === l.id ? "#4af" : undefined }}
            onClick={() => {
              stop();
              setLayerId(l.id);
              setFixed(false);
            }}
          >
            {l.id}
          </button>
        ))}
        <button onClick={toggle}>{playing() ? "Stop" : "Play loop"}</button>
        <span style={{ "font-size": "12px", color: "#8cf" }}>
          gapless wrap demo: beat 9.5 of 0–8 plays at {wrapped().toFixed(2)}
        </span>
      </div>
      <div style={{ display: "flex", gap: "12px", "align-items": "center", "flex-wrap": "wrap" }}>
        <label style={{ "font-size": "12px" }}>
          click threshold
          <input
            type="range"
            min={0.001}
            max={0.1}
            step={0.001}
            value={threshold()}
            onInput={(e) => setThreshold(Number(e.target.value))}
          />
          {threshold().toFixed(3)}
        </label>
        <span
          style={{
            "font-size": "12px",
            color: report().click ? "#f66" : "#6d6",
          }}
        >
          seam step {report().step.toFixed(4)} {report().click ? "— CLICKS" : "— clean"}
          {" "}(peak {report().peak.toFixed(3)})
        </span>
      </div>
      {report().click && (
        <div style={{ "font-size": "12px", color: "#f99" }}>
          {clickErrorText(`stems/${props.cue.id}_${layerId()}.wav`, report())}
        </div>
      )}
      <div style={{ display: "flex", gap: "6px", "align-items": "center", "flex-wrap": "wrap" }}>
        <button onClick={() => setFixed(true)} disabled={fixed() || !report().click}>
          Fix: fade edges
        </button>
        <input
          type="text"
          placeholder="waiver reason (required)"
          value={reason()}
          onInput={(e) => setReason(e.target.value)}
          style={{ width: "240px" }}
        />
        <button onClick={waive} disabled={reason().trim().length === 0}>
          Waive with reason
        </button>
      </div>
      {waivers().length > 0 && (
        <pre style={{ "font-size": "11px", color: "#aaa" }}>{waiverFileJson(waivers())}</pre>
      )}
    </div>
  );
}
