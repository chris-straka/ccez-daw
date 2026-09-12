import { For, Show, createSignal } from "solid-js";
import { FILMSTRIP_THUMBS, applyOffsetDrag, type VideoClip, type VideoDoc } from "./model";

/**
 * Agent 2: filmstrip lane — one row per video clip on the beat ruler.
 *
 * Thumbnails are CSS placeholder cells (no decoder in the UI shell); a
 * real build swaps the cell background for `blob:` URLs from a thumbnail
 * service. Offset drag is a horizontal pointer drag: 4px = 1 beat, same
 * scale as the Track E linear lane (`margin-left: start*4px`), calling
 * back with the new `offset_beats` so the host owns the store.
 */
export default function FilmstripLane(props: {
  doc: VideoDoc;
  pxPerBeat?: number;
  onOffsetChange?: (clipId: string, offsetBeats: number) => void;
}) {
  const pxPerBeat = () => props.pxPerBeat ?? 4;
  const [dragging, setDragging] = createSignal<string | null>(null);

  function onPointerDown(e: PointerEvent, clip: VideoClip) {
    const startX = e.clientX;
    const startOffset = clip.offset_beats;
    setDragging(clip.id);
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    const move = (ev: PointerEvent) => {
      const deltaBeats = (ev.clientX - startX) / pxPerBeat();
      // Live preview via the callback; store write stays with the host.
      props.onOffsetChange?.(clip.id, applyOffsetDrag({ ...clip, offset_beats: startOffset }, deltaBeats));
    };
    const up = () => {
      setDragging(null);
      window.removeEventListener("pointermove", move as never);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move as never);
    window.addEventListener("pointerup", up);
  }

  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
      <div style={{ color: "#aaa", "font-size": "12px" }}>Video filmstrip</div>
      <Show when={props.doc.clips.length > 0} fallback={<div style={{ color: "#888" }}>No video clips.</div>}>
        <For each={props.doc.clips}>
          {(clip) => (
            <div style={{ display: "flex", gap: "4px", "align-items": "center" }}>
              <span style={{ width: "64px", color: "#aaa", "font-size": "12px" }}>{clip.name}</span>
              <div
                title={`${clip.name} @ ${clip.start_beats} (offset ${clip.offset_beats.toFixed(2)} beats)`}
                onPointerDown={(e) => onPointerDown(e, clip)}
                style={{
                  display: "flex",
                  gap: "2px",
                  cursor: "ew-resize",
                  border: dragging() === clip.id ? "1px solid #fa4" : "1px solid #84c",
                  "border-radius": "4px",
                  padding: "2px",
                  "margin-left": `${clip.start_beats * pxPerBeat()}px`,
                  width: `${clip.length_beats * pxPerBeat()}px`,
                  overflow: "hidden",
                  opacity: dragging() === clip.id ? "0.85" : "1",
                }}
              >
                <For each={Array.from({ length: FILMSTRIP_THUMBS })}>
                  {(_, i) => (
                    <span
                      style={{
                        flex: "1",
                        height: "22px",
                        "border-radius": "2px",
                        background: `linear-gradient(${120 + i() * 8}deg, #2a2a4a, #4a2a5a)`,
                        "min-width": "4px",
                      }}
                    />
                  )}
                </For>
              </div>
              <span style={{ color: "#666", "font-size": "11px" }}>off {clip.offset_beats.toFixed(2)}</span>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}
