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

  /** Canvas palette mirrors theme.css tokens (canvas cannot read CSS vars per-frame cheaply). */
  const PAL = { bg: "#0d1117", idle: "#2a333e", gain: "#3fc1a5", text: "#9aa3ad", accent: "#f5a623" };

  function draw() {
    const el = canvas;
    if (el) {
      const ctx = el.getContext("2d");
      if (ctx) {
        const t = transport();
        const gains = previewGains(cue(), trans.from, trans.to, trans.atBeat, trans.fadeBeats, t.beat);
        const W = el.width;
        const H = el.height;
        ctx.fillStyle = PAL.bg;
        ctx.fillRect(0, 0, W, H);
        const layers = cue().layers;
        const bw = layers.length > 0 ? W / layers.length : W;
        layers.forEach((layer, i) => {
          const g = (gains[layer.id] ?? 0) * layer.volume;
          const h = Math.max(0, Math.min(1, g)) * (H - 28);
          ctx.fillStyle = g > 0.01 ? PAL.gain : PAL.idle;
          ctx.fillRect(i * bw + 6, H - 14 - h, bw - 12, h);
          ctx.fillStyle = PAL.text;
          ctx.font = "10px system-ui";
          ctx.fillText(`${layer.id} ${(gains[layer.id] ?? 0).toFixed(2)}`, i * bw + 6, H - 2);
        });
        ctx.fillStyle = PAL.accent;
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
    <div class="session-view">
      <canvas ref={canvas} width={480} height={140} class="audition-canvas" style={{ width: "100%" }} />
      <div class="session-controls">
        <button onClick={() => setTransport((t) => ({ ...t, playing: !t.playing }))}>
          {transport().playing ? "Pause" : "Play"}
        </button>
        <label class="mixer-label" style={{ "min-width": "140px" }}>
          tempo
          <input
            type="range"
            min={60}
            max={180}
            step={1}
            value={transport().tempo}
            onInput={(e) => setTransport((t) => ({ ...t, tempo: Number(e.target.value) }))}
          />
          <span class="mixer-values daw-numeric">{transport().tempo.toFixed(0)} BPM</span>
        </label>
        <span class="meter-readout daw-numeric">
          beat {transport().beat.toFixed(2)} = {beatsToSeconds(transport().beat, transport().tempo).toFixed(2)}s
          {" · "}audible: {audible().join(", ") || "(silent)"}
          {" · "}timeline @ beat: {atBeat().state}
        </span>
      </div>
      <div class="session-controls">
        <span class="session-dim">State:</span>
        <For each={["explore", "combat", "menu"]}>
          {(s) => (
            <button
              class="state-btn"
              classList={{ active: stateName() === s }}
              onClick={() => postState(s)}
            >
              {s}
            </button>
          )}
        </For>
        <button onClick={pinEntry}>Pin snapshot @ beat</button>
      </div>
      <div class="session-controls">
        <For each={declared()}>
          {(p) => (
            <label class="mixer-label" style={{ "min-width": "140px" }}>
              {p.label} ({p.id}{p.unit ? `, ${p.unit}` : ""})
              <input
                type="range"
                min={p.min}
                max={p.max}
                step={(p.max - p.min) / 100 || 0.01}
                value={values()[p.id] ?? p.default}
                onInput={(e) => setValues((v) => ({ ...v, [p.id]: Number(e.target.value) }))}
              />
              <span class="mixer-values daw-numeric">
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
              </span>
            </label>
          )}
        </For>
      </div>
      <div class="session-controls">
        <span class="session-dim">Fire:</span>
        <For each={bank().events}>
          {(e) => <button onClick={() => fire(e.id)}>{e.id}</button>}
        </For>
      </div>
      <div class="session-dim">
        <div>
          Timeline ({entries().length} pinned; evaluated live, never stored as ops):
        </div>
        <For each={entries()}>
          {(e) => (
            <div class="daw-numeric">
              <button onClick={() => jumpTo(e)}>jump</button> beat {e.beat.toFixed(2)} — {e.snapshot.state}
            </div>
          )}
        </For>
      </div>
      <div class="mixer-foot audition-log">
        <For each={log()}>{(line) => <div class="daw-numeric">{line}</div>}</For>
      </div>
      <div class="mixer-foot">
        Recent voices: {fires().map((f) => `${f.event_id}@${f.clip_id}`).join(", ") || "—"}
      </div>
    </div>
  );
}
