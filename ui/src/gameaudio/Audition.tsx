import { For, createSignal, onCleanup, onMount } from "solid-js";
import type {
  AdaptiveCue,
  GameStateParam,
  GameStateSnapshot,
  SfxBank,
} from "../generated/project";
import {
  advanceTransport,
  beatsToSeconds,
  effectiveKind,
  fireEvent,
  isDegraded,
  layersForState,
  makeFireRuntime,
  makeTransport,
  previewGains,
  snapshotAt,
  transitionFor,
  addTimelineEntry,
  resolveRtpc,
  valueFor,
  type FireRecord,
  type SimTransport,
  type TimelineEntry,
} from "./model";
import { defaultSnapshot, sampleBank, sampleCue, sampleParams } from "./sample";

/**
 * GA-3: game-state audition simulator view.
 *
 * A rehearsal room for the frozen v1 game-audio contracts: pick the named
 * state, drag live param sliders, keep the audition clock on the transport,
 * pin snapshots to a beat timeline, and fire SFX events — while a hot
 * canvas (rAF) shows per-layer preview gains. Nothing here writes ops or
 * edits the schema; snapshots are evaluated, never stored.
 */
export default function Audition(props: {
  cue?: AdaptiveCue;
  bank?: SfxBank;
  params?: GameStateParam[];
}) {
  const cue = () => props.cue ?? sampleCue();
  const bank = () => props.bank ?? sampleBank();
  const declared = () => props.params ?? sampleParams();

  const [stateName, setStateName] = createSignal(defaultSnapshot().state);
  const [values, setValues] = createSignal<Record<string, number>>({});
  const [transport, setTransport] = createSignal<SimTransport>(makeTransport(cue().tempo));
  const [entries, setEntries] = createSignal<TimelineEntry[]>([]);
  const [log, setLog] = createSignal<string[]>([]);
  const [fires, setFires] = createSignal<FireRecord[]>([]);

  const runtime = makeFireRuntime(2026);
  let canvas: HTMLCanvasElement | undefined;
  let raf = 0;
  let lastMs = 0;
  // Last posted transition, for the animated preview ramp.
  let trans = { from: cue().default_state, to: cue().default_state, atBeat: 0, fadeBeats: 0 };

  function snapshot(): GameStateSnapshot {
    const vals = declared().map((p) => ({
      param: p.id,
      value: values()[p.id] ?? p.default,
    }));
    return { state: stateName(), values: vals };
  }

  function pushLog(line: string) {
    setLog((prev) => [...prev.slice(-79), line]);
  }

  function postState(next: string) {
    const from = stateName();
    const rule = transitionFor(cue(), from, next);
    const kind = effectiveKind(cue(), from, next);
    setStateName(next);
    trans = {
      from,
      to: next,
      atBeat: transport().beat,
      fadeBeats: rule?.kind === "Fade" ? rule.fade_beats : 0,
    };
    pushLog(
      `${from} -> ${next}: ${kind}${isDegraded(kind) ? " (sim plays Cut)" : ""} @ beat ${transport().beat.toFixed(2)}`,
    );
  }

  function pinEntry() {
    const id = `pin_${entries().length + 1}`;
    setEntries((prev) => addTimelineEntry(prev, { id, beat: transport().beat, snapshot: snapshot() }));
    pushLog(`pinned ${id} @ beat ${transport().beat.toFixed(2)} (${stateName()})`);
  }

  function jumpTo(entry: TimelineEntry) {
    setTransport((t) => ({ ...t, beat: entry.beat }));
    setStateName(entry.snapshot.state);
    const next: Record<string, number> = {};
    for (const v of entry.snapshot.values) next[v.param] = v.value;
    setValues(next);
    trans = { from: entry.snapshot.state, to: entry.snapshot.state, atBeat: entry.beat, fadeBeats: 0 };
    pushLog(`jumped to ${entry.id} @ beat ${entry.beat.toFixed(2)}`);
  }

  function fire(eventId: string) {
    const nowMs = Date.now() % 1000000;
    const res = fireEvent(bank(), runtime, eventId, snapshot(), declared(), nowMs, transport().beat);
    if (res.ok) {
      setFires((prev) => [...prev.slice(-19), res.voice]);
      pushLog(`fired ${eventId} -> ${res.voice.clip_id} g=${res.voice.gain.toFixed(2)} p=${res.voice.pitch_semitones.toFixed(1)}st`);
    } else {
      pushLog(`dropped ${eventId}: ${res.kind}`);
    }
  }

  function draw() {
    const el = canvas;
    if (el) {
      const ctx = el.getContext("2d");
      if (ctx) {
        const t = transport();
        const gains = previewGains(cue(), trans.from, trans.to, trans.atBeat, trans.fadeBeats, t.beat);
        const W = el.width;
        const H = el.height;
        ctx.fillStyle = "#111";
        ctx.fillRect(0, 0, W, H);
        const layers = cue().layers;
        const bw = layers.length > 0 ? W / layers.length : W;
        layers.forEach((layer, i) => {
          const g = (gains[layer.id] ?? 0) * layer.volume;
          const h = Math.max(0, Math.min(1, g)) * (H - 28);
          ctx.fillStyle = g > 0.01 ? "#4af" : "#333";
          ctx.fillRect(i * bw + 6, H - 14 - h, bw - 12, h);
          ctx.fillStyle = "#ccc";
          ctx.font = "10px system-ui";
          ctx.fillText(`${layer.id} ${(gains[layer.id] ?? 0).toFixed(2)}`, i * bw + 6, H - 2);
        });
        ctx.fillStyle = "#8cf";
        ctx.font = "11px system-ui";
        ctx.fillText(
          `beat ${t.beat.toFixed(2)} (${beatsToSeconds(t.beat, t.tempo).toFixed(2)}s) ${t.playing ? "▶" : "■"} ${stateName()}`,
          6,
          12,
        );
      }
    }
  }

  onMount(() => {
    lastMs = performance.now();
    const tick = (nowMs: number) => {
      const dt = (nowMs - lastMs) / 1000;
      lastMs = nowMs;
      setTransport((t) => advanceTransport(t, Math.min(dt, 0.25)));
      draw();
    };
    const loop = (n: number) => {
      tick(n);
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    onCleanup(() => cancelAnimationFrame(raf));
  });

  const audible = () => layersForState(cue(), stateName()).map((l) => l.id);
  const atBeat = () => snapshotAt(entries(), defaultSnapshot(), transport().beat);

  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
      <canvas ref={canvas} width={480} height={140} style={{ border: "1px solid #555", width: "100%" }} />
      <div style={{ display: "flex", gap: "8px", "align-items": "center", "flex-wrap": "wrap" }}>
        <button onClick={() => setTransport((t) => ({ ...t, playing: !t.playing }))}>
          {transport().playing ? "Pause" : "Play"}
        </button>
        <label style={{ "font-size": "12px" }}>
          tempo
          <input
            type="range"
            min={60}
            max={180}
            step={1}
            value={transport().tempo}
            onInput={(e) => setTransport((t) => ({ ...t, tempo: Number(e.target.value) }))}
          />
          {transport().tempo.toFixed(0)} BPM
        </label>
        <span style={{ "font-size": "12px", color: "#8cf" }}>
          beat {transport().beat.toFixed(2)} = {beatsToSeconds(transport().beat, transport().tempo).toFixed(2)}s
          {" · "}audible: {audible().join(", ") || "(silent)"}
          {" · "}timeline @ beat: {atBeat().state}
        </span>
      </div>
      <div style={{ display: "flex", gap: "6px", "flex-wrap": "wrap", "align-items": "center" }}>
        <span style={{ "font-size": "12px", color: "#aaa" }}>State:</span>
        <For each={["explore", "combat", "menu"]}>
          {(s) => (
            <button
              style={{ background: stateName() === s ? "#4af" : undefined }}
              onClick={() => postState(s)}
            >
              {s}
            </button>
          )}
        </For>
        <button onClick={pinEntry}>Pin snapshot @ beat</button>
      </div>
      <div style={{ display: "flex", gap: "12px", "flex-wrap": "wrap" }}>
        <For each={declared()}>
          {(p) => (
            <label style={{ display: "block", "font-size": "12px", "min-width": "140px" }}>
              {p.label} ({p.id}{p.unit ? `, ${p.unit}` : ""})
              <input
                type="range"
                min={p.min}
                max={p.max}
                step={(p.max - p.min) / 100 || 0.01}
                value={values()[p.id] ?? p.default}
                onInput={(e) => setValues((v) => ({ ...v, [p.id]: Number(e.target.value) }))}
              />
              {(values()[p.id] ?? p.default).toFixed(2)}
              {" → "}
              {(() => {
                const b = bank().events.flatMap((e) => e.rtpc).find((r) => r.param === p.id);
                if (!b) return "—";
                const [hit] = resolveRtpc([b], snapshot(), declared());
                return `${b.target_node}:${b.target_param} = ${hit.value.toFixed(2)}`;
              })()}
              {" (default "}
              {valueFor(p, { state: stateName(), values: [] }).toFixed(2)}
              {")"}
            </label>
          )}
        </For>
      </div>
      <div style={{ display: "flex", gap: "6px", "flex-wrap": "wrap", "align-items": "center" }}>
        <span style={{ "font-size": "12px", color: "#aaa" }}>Fire:</span>
        <For each={bank().events}>
          {(e) => <button onClick={() => fire(e.id)}>{e.id}</button>}
        </For>
      </div>
      <div style={{ "font-size": "12px" }}>
        <div style={{ color: "#aaa" }}>
          Timeline ({entries().length} pinned; evaluated live, never stored as ops):
        </div>
        <For each={entries()}>
          {(e) => (
            <div>
              <button onClick={() => jumpTo(e)}>jump</button> beat {e.beat.toFixed(2)} — {e.snapshot.state}
            </div>
          )}
        </For>
      </div>
      <div style={{ "font-size": "11px", color: "#888", "max-height": "120px", overflow: "auto" }}>
        <For each={log()}>{(line) => <div>{line}</div>}</For>
      </div>
      <div style={{ "font-size": "11px", color: "#666" }}>
        Recent voices: {fires().map((f) => `${f.event_id}@${f.clip_id}`).join(", ") || "—"}
      </div>
    </div>
  );
}
