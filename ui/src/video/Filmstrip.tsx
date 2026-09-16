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
    <div class="session-view" style={{ gap: "4px" }}>
      <div class="view-label">Video filmstrip</div>
      <Show when={props.doc.clips.length > 0} fallback={<div class="session-dim">No video clips.</div>}>
        <For each={props.doc.clips}>
          {(clip) => (
            <div class="lane">
              <span class="lane-name">{clip.name}</span>
              <div
                title={`${clip.name} @ ${clip.start_beats} (offset ${clip.offset_beats.toFixed(2)} beats)`}
                onPointerDown={(e) => onPointerDown(e, clip)}
                class={`filmstrip-cell${dragging() === clip.id ? " dragging" : ""}`}
                style={{
                  "margin-left": `${clip.start_beats * pxPerBeat()}px`,
                  width: `${clip.length_beats * pxPerBeat()}px`,
                }}
              >
                <For each={Array.from({ length: FILMSTRIP_THUMBS })}>
                  {(_, i) => (
                    <span
                      class="filmstrip-thumb"
                      style={{
                        background: `linear-gradient(${120 + i() * 8}deg, #1c2733, #33475e)`,
                      }}
                    />
                  )}
                </For>
              </div>
              <span class="mixer-foot daw-numeric">off {clip.offset_beats.toFixed(2)}</span>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}
