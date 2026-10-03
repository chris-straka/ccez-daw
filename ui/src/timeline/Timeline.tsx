import { For, Show, createSignal } from "solid-js";
import type { Op, Project } from "../generated/project";
import { asset_store, op_apply } from "../tauri/commands";
import {
  beatsForFrames,
  buildImportClip,
  importClipOp,
  parseWavInfo,
  sanitizeAssetKey,
  ImportError,
} from "./import";
import {
  clipsSortedOnTrack,
  launcherSlots,
  moveClipOp,
  sampleTimelineDoc,
  sectionEnd,
} from "./model";

/** Pixels per beat on the linear lanes (single source for layout + drag math). */
export const PX_PER_BEAT = 4;

/**
 * Track E: linear + launcher views over one project model.
 *
 * The linear view lays each track's clips on a beat ruler; the launcher
 * view renders the same clips as a `(section, track)` slot grid. Both read
 * the same `Project` — sections come from the timeline sidecar
 * (`sampleTimelineDoc` seeds the demo Verse/Chorus until the doc has a
 * loader).
 *
 * Linear clips drag horizontally: the chip follows the pointer locally and
 * a frozen `ClipMoved` op commits on release (undoable, 1-beat snap).
 */
export default function TimelineView(props: {
  project: Project | null;
  applyOp?: (op: Op) => Promise<unknown>;
  onChanged?: () => void;
}) {
  const doc = sampleTimelineDoc();
  const [drag, setDrag] = createSignal<{ clipId: string; orig: number; cur: number; x0: number } | null>(null);
  const [status, setStatus] = createSignal<string | null>(null);
  const [importTrack, setImportTrack] = createSignal<string | null>(null);
  const [importBeat, setImportBeat] = createSignal(0);

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  // WAV import: store the bytes as an `audio` asset, then place an
  // `asset:` Audio clip through a frozen `ClipAdded` op (undoable like a
  // take). The header is validated the way the engine decoder will, so an
  // accepted file always renders.
  async function importFile(file: File): Promise<void> {
    const project = props.project;
    if (!project) {
      setStatus("import: no project open");
      return;
    }
    const tracks = project.tracks;
    if (tracks.length === 0) {
      setStatus("import: no tracks — add one first");
      return;
    }
    const trackId = importTrack() ?? tracks[0]!.id;
    if (!tracks.some((t) => t.id === trackId)) {
      setStatus("import: track is gone — pick another");
      return;
    }
    const startBeats = Math.max(0, importBeat() || 0);
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const info = parseWavInfo(bytes);
      const key = sanitizeAssetKey(file.name || "import.wav");
      await asset_store({ key, kind: "audio", bytes: Array.from(bytes) });
      const tempo = project.tempo > 0 ? project.tempo : 120;
      const clip = buildImportClip(
        trackId,
        startBeats,
        key,
        key,
        beatsForFrames(info.numFrames, info.sampleRate, tempo),
      );
      await apply(importClipOp("ui", clip));
      setStatus(`imported ${key} → ${clip.id} @ ${startBeats} (${clip.length_beats} beats, undoable)`);
      props.onChanged?.();
    } catch (e) {
      setStatus(
        e instanceof ImportError
          ? `import refused: ${e.message}`
          : `import failed: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }

  function shownStart(clipId: string, committed: number): number {
    const d = drag();
    return d && d.clipId === clipId ? d.cur : committed;
  }

  function onPointerDown(e: PointerEvent, clipId: string, start: number): void {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    setDrag({ clipId, orig: start, cur: start, x0: e.clientX });
  }

  function onPointerMove(e: PointerEvent): void {
    const d = drag();
    if (!d) return;
    setDrag({ ...d, cur: Math.max(0, d.orig + Math.round((e.clientX - d.x0) / PX_PER_BEAT)) });
  }

  async function onPointerUp(): Promise<void> {
    const d = drag();
    setDrag(null);
    if (!d || d.cur === d.orig) return;
    try {
      await apply(moveClipOp("ui", d.clipId, d.cur));
      setStatus(`Moved ${d.clipId} → beat ${d.cur} (undoable)`);
      props.onChanged?.();
    } catch (err) {
      setStatus(`move refused: ${String(err)}`);
    }
  }

  return (
    <div id="timeline-view" data-testid="timeline-view" tabIndex={-1} class="session-view">
      <Show
        when={props.project}
        fallback={<div class="session-dim">No project open.</div>}
      >
        <div class="session-controls">
          <span class="view-label">Import WAV</span>
          <label>
            Track{" "}
            <select
              data-testid="timeline-import-track"
              value={importTrack() ?? props.project?.tracks[0]?.id ?? ""}
              onInput={(e) => setImportTrack(e.currentTarget.value || null)}
            >
              <For each={props.project?.tracks ?? []}>
                {(t) => <option value={t.id}>{t.name}</option>}
              </For>
            </select>
          </label>
          <label>
            Beat{" "}
            <input
              data-testid="timeline-import-beat"
              type="number"
              min={0}
              value={importBeat()}
              onInput={(e) => setImportBeat(Number(e.currentTarget.value) || 0)}
              style={{ width: "64px" }}
            />
          </label>
          <label>
            File{" "}
            <input
              data-testid="timeline-import-file"
              type="file"
              accept="audio/wav,.wav"
              onChange={(e) => {
                const f = e.currentTarget.files?.[0];
                e.currentTarget.value = "";
                if (f) void importFile(f);
              }}
            />
          </label>
        </div>
        {/* Linear: one lane per track, clips positioned by start_beats. */}
        <div>
          <div class="view-label">
            Linear
          </div>
          <For each={props.project?.tracks ?? []}>
            {(track) => (
              <div class="lane">
                <span class="lane-name">
                  {track.name}
                </span>
                <For each={clipsSortedOnTrack(props.project!, track.id)}>
                  {(clip) => (
                    <span
                      data-testid={`clip-${clip.id}`}
                      title={`${clip.name} @ ${shownStart(clip.id, clip.start_beats)} (${clip.length_beats} beats)`}
                      class={`clip-chip ${clip.kind === "Audio" ? "clip-audio" : "clip-midi"}`}
                      classList={{ dragging: drag()?.clipId === clip.id }}
                      style={{ "margin-left": `${shownStart(clip.id, clip.start_beats) * PX_PER_BEAT}px` }}
                      onPointerDown={(e) => onPointerDown(e, clip.id, clip.start_beats)}
                      onPointerMove={onPointerMove}
                      onPointerUp={() => void onPointerUp()}
                    >
                      {clip.name}
                    </span>
                  )}
                </For>
              </div>
            )}
          </For>
          <Show when={status()}>
            <div class="meter-readout" data-testid="timeline-status">{status()}</div>
          </Show>
        </div>
        {/* Launcher: rows are sections, columns are tracks. */}
        <div>
          <div class="view-label">
            Launcher
          </div>
          <For each={doc.sections}>
            {(section) => (
              <div class="lane">
                <span class="lane-name">
                  {section.name} {section.start_beats}&ndash;
                  {sectionEnd(section)}
                </span>
                <For
                  each={launcherSlots(props.project!, doc).filter(
                    (s) => s.section_id === section.id,
                  )}
                >
                  {(slot) => (
                    <span
                      title={`${slot.track_id}: ${slot.clip_ids.join(", ") || "silent"}`}
                      class={`slot-chip ${slot.clip_ids.length > 0 ? "filled" : "silent"}`}
                    >
                      {slot.clip_ids.join(", ") || "—"}
                    </span>
                  )}
                </For>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
