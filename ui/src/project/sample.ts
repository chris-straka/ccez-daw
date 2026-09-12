import { ProjectSchema } from "../generated/project";
import type { Project } from "../generated/project";

/** Headless mirror of the Rust `Project::sample()` shape. */
export function sampleProject(): Project {
  return {
    schema_version: 0,
    id: "proj_sample",
    name: "Untitled",
    tempo: 120,
    time_sig_num: 4,
    time_sig_den: 4,
    tracks: [
      {
        id: "trk_click",
        name: "Click",
        volume: 0.8,
        pan: 0,
        muted: false,
        solo: false,
        clip_ids: ["clip_a"],
        device_ids: [],
      },
      {
        id: "trk_music",
        name: "Music",
        volume: 0.8,
        pan: 0,
        muted: false,
        solo: false,
        clip_ids: ["clip_b"],
        device_ids: [],
      },
    ],
    clips: [
      {
        id: "clip_a",
        track_id: "trk_click",
        name: "Count-in",
        start_beats: 0,
        length_beats: 4,
        kind: "Audio",
        source: "builtin:click",
      },
      {
        id: "clip_b",
        track_id: "trk_music",
        name: "Sketch",
        start_beats: 4,
        length_beats: 8,
        kind: "Midi",
        source: "take:1",
      },
    ],
    devices: [],
    routing: [],
    automation: [],
  };
}

export function parseSample(): Project {
  return ProjectSchema.parse(sampleProject());
}
