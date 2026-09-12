import { Show, createEffect, onCleanup } from "solid-js";
import { timecodeForTransport, videoTimeForTransport } from "./model";
import type { VideoDoc } from "./model";
import { applySyncDecision, syncDecision, type PlayState } from "./sync";

/**
 * Agent 2: video preview pane synced to the transport.
 *
 * Contract with the host: pass the engine clock (`transportBeats`, `tempo`,
 * `state`) every render/tick — the component samples `el.currentTime`,
 * runs the pure `syncDecision`, and executes it. No clock is read here;
 * no audio is touched. `src` comes from the active clip (`take:<key>`
 * sources render a placeholder until an asset URL is wired).
 */
export default function VideoPreview(props: {
  doc: VideoDoc;
  transportBeats: number;
  tempo: number;
  state: PlayState;
}) {
  let el: HTMLVideoElement | undefined;

  function activeSrc(): string | null {
    const t = videoTimeForTransport(props.doc, props.transportBeats, props.tempo);
    if (t === null) return null;
    const sorted = props.doc.clips.slice().sort((a, b) => a.start_beats - b.start_beats);
    for (const c of sorted) {
      if (
        props.transportBeats >= c.start_beats &&
        props.transportBeats < c.start_beats + c.length_beats
      )
        return c.src.startsWith("take:") ? "" : c.src;
    }
    return null;
  }

  function tick() {
    if (!el) return;
    const decision = syncDecision({
      doc: props.doc,
      transportBeats: props.transportBeats,
      tempo: props.tempo,
      state: props.state,
      elementTime: Number.isFinite(el.currentTime) ? el.currentTime : null,
      elementPaused: el.paused,
    });
    void applySyncDecision(el, decision, props.state === "Playing");
  }

  createEffect(() => {
    // Re-run on every prop change (transport tick re-renders the host).
    void props.transportBeats;
    void props.tempo;
    void props.state;
    tick();
  });

  onCleanup(() => {
    el?.pause();
  });

  const tc = () => timecodeForTransport(props.doc, props.transportBeats, props.tempo);

  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
      <div style={{ color: "#aaa", "font-size": "12px" }}>
        Video preview <span style={{ color: "#8cf", "font-family": "monospace" }}>{tc() ?? "--:--:--:--"}</span>
      </div>
      <Show
        when={activeSrc()}
        fallback={
          <div
            style={{
              background: "#000",
              color: "#666",
              height: "120px",
              display: "flex",
              "align-items": "center",
              "justify-content": "center",
              "border-radius": "4px",
              "font-size": "12px",
            }}
          >
            {tc() === null ? "No video under playhead" : `Preview: ${tc()} (take: source — no media wired)`}
          </div>
        }
      >
        {/* eslint-disable-next-line jsx-a11y/media-has-caption */}
        <video ref={el} src={activeSrc()!} style={{ width: "100%", "border-radius": "4px", background: "#000" }} />
      </Show>
    </div>
  );
}
