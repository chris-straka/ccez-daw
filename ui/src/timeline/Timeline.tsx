import { For, Show } from "solid-js";
import type { Project } from "../generated/project";
import {
  clipsSortedOnTrack,
  launcherSlots,
  sampleTimelineDoc,
  sectionEnd,
} from "./model";

/**
 * Track E: linear + launcher views over one project model.
 *
 * The linear view lays each track's clips on a beat ruler; the launcher
 * view renders the same clips as a `(section, track)` slot grid. Both read
 * the same `Project` — sections come from the timeline sidecar
 * (`sampleTimelineDoc` seeds the demo Verse/Chorus until the doc has a
 * loader). Props are headless-only for now: pass `project`, nothing more.
 */
export default function TimelineView(props: { project: Project | null }) {
  const doc = sampleTimelineDoc();
  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
      <Show
        when={props.project}
        fallback={<div style={{ color: "#888" }}>No project open.</div>}
      >
        {/* Linear: one lane per track, clips positioned by start_beats. */}
        <div>
          <div style={{ color: "#aaa", "font-size": "12px", margin: "0 0 4px 0" }}>
            Linear
          </div>
          <For each={props.project?.tracks ?? []}>
            {(track) => (
              <div style={{ display: "flex", gap: "4px", "align-items": "center" }}>
                <span style={{ width: "64px", color: "#aaa", "font-size": "12px" }}>
                  {track.name}
                </span>
                <For each={clipsSortedOnTrack(props.project!, track.id)}>
                  {(clip) => (
                    <span
                      title={`${clip.name} @ ${clip.start_beats} (${clip.length_beats} beats)`}
                      style={{
                        border: "1px solid #6af",
                        "border-radius": "4px",
                        padding: "2px 6px",
                        "font-size": "12px",
                        "margin-left": `${clip.start_beats * 4}px`,
                      }}
                    >
                      {clip.name}
                    </span>
                  )}
                </For>
              </div>
            )}
          </For>
        </div>
        {/* Launcher: rows are sections, columns are tracks. */}
        <div>
          <div style={{ color: "#aaa", "font-size": "12px", margin: "0 0 4px 0" }}>
            Launcher
          </div>
          <For each={doc.sections}>
            {(section) => (
              <div style={{ display: "flex", gap: "4px", "align-items": "center" }}>
                <span style={{ width: "64px", color: "#aaa", "font-size": "12px" }}>
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
                      style={{
                        border: "1px solid #4a4",
                        "border-radius": "4px",
                        padding: "2px 6px",
                        "font-size": "12px",
                        "min-width": "48px",
                      }}
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
