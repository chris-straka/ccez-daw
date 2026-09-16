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
    <div class="session-view">
      <Show
        when={props.project}
        fallback={<div class="session-dim">No project open.</div>}
      >
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
                      title={`${clip.name} @ ${clip.start_beats} (${clip.length_beats} beats)`}
                      class={`clip-chip ${clip.kind === "Audio" ? "clip-audio" : "clip-midi"}`}
                      style={{ "margin-left": `${clip.start_beats * 4}px` }}
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
