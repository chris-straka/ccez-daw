/**
 * Agent 2 (video track): UI-local sidecar model for video against music.
 *
 * Video is NOT part of the frozen v0 project schema (`contracts/`,
 * `core/src/model.rs`) and never crosses the Tauri IPC boundary — the
 * engine renders audio, the `<video>` element renders picture, and this
 * module is the ruler that keeps them on the same beat grid. A `VideoClip`
 * pins one media file to the timeline by `start_beats` + `offset_beats`:
 * transport beat `b` shows media time `(b - start_beats - offset_beats)`
 * converted at the project tempo. Dragging the clip edits `offset_beats`
 * only; the audio project is untouched.
 */

export interface VideoClip {
  id: string;
  name: string;
  /** Media source: object URL, asset path, or `take:<key>`. Never resolved here. */
  src: string;
  /** Timeline position of the clip head, in beats. */
  start_beats: number;
  /** Clip length on the timeline, in beats. */
  length_beats: number;
  /** Slip offset in beats: + shifts picture later (transport leads). */
  offset_beats: number;
  /** Frames per second used for timecode display. */
  fps: number;
  /** Media duration in seconds (0 = unknown; validation skips duration checks). */
  duration_seconds: number;
}

export interface VideoDoc {
  clips: VideoClip[];
}

export const DEFAULT_FPS = 30;
/** Thumbnails per filmstrip lane render; pure display constant. */
export const FILMSTRIP_THUMBS = 12;

/** Beats -> seconds at one tempo. Mirrors `beatsToSeconds` in timeline/model. */
export function beatsToSeconds(beats: number, tempo: number): number {
  if (!(tempo > 0 && Number.isFinite(tempo))) throw new Error(`tempo ${tempo} must be > 0`);
  return (beats * 60) / tempo;
}

/** Seconds -> beats at one tempo. */
export function secondsToBeats(seconds: number, tempo: number): number {
  if (!(tempo > 0 && Number.isFinite(tempo))) throw new Error(`tempo ${tempo} must be > 0`);
  return (seconds * tempo) / 60;
}

function assertClipBase(c: Pick<VideoClip, "id" | "start_beats" | "length_beats" | "fps">): void {
  if (!c.id) throw new Error("video clip id must be non-empty");
  if (!(Number.isFinite(c.start_beats) && c.start_beats >= 0))
    throw new Error(`start_beats ${c.start_beats} must be finite and >= 0`);
  if (!(Number.isFinite(c.length_beats) && c.length_beats > 0))
    throw new Error(`length_beats ${c.length_beats} must be finite and > 0`);
  if (!(Number.isFinite(c.fps) && c.fps > 0 && c.fps <= 240))
    throw new Error(`fps ${c.fps} must be in (0, 240]`);
}

/** Range-check one video clip. Throws; the renderer never reinterprets. */
export function validateVideoClip(clip: VideoClip): void {
  assertClipBase(clip);
  if (!clip.src) throw new Error("video clip src must be non-empty");
  if (!(Number.isFinite(clip.offset_beats)))
    throw new Error(`offset_beats ${clip.offset_beats} must be finite`);
  if (!(Number.isFinite(clip.duration_seconds) && clip.duration_seconds >= 0))
    throw new Error(`duration_seconds ${clip.duration_seconds} must be >= 0`);
}

function activeClip(doc: VideoDoc, transportBeats: number): VideoClip | null {
  const sorted = doc.clips.slice().sort((a, b) => a.start_beats - b.start_beats);
  let hit: VideoClip | null = null;
  for (const c of sorted) {
    if (transportBeats >= c.start_beats && transportBeats < c.start_beats + c.length_beats)
      hit = c;
  }
  return hit;
}

/**
 * Media time (seconds) the preview should show for a transport position.
 * Returns null when no clip covers `transportBeats` or the media time is
 * negative (offset pushed picture before its head: hold first frame).
 * Result is NOT clamped to duration — the preview element holds last frame.
 */
export function videoTimeForTransport(
  doc: VideoDoc,
  transportBeats: number,
  tempo: number,
): number | null {
  const clip = activeClip(doc, transportBeats);
  if (!clip) return null;
  const mediaBeats = transportBeats - clip.start_beats - clip.offset_beats;
  if (mediaBeats < 0) return null;
  return beatsToSeconds(mediaBeats, tempo);
}

/** Inverse: transport beats that would show `videoSeconds` of `clip`. */
export function transportBeatsForVideoTime(
  clip: VideoClip,
  videoSeconds: number,
  tempo: number,
): number {
  return clip.start_beats + clip.offset_beats + secondsToBeats(videoSeconds, tempo);
}

/**
 * Slip a clip by `deltaBeats` (the offset drag). Returns the new offset;
 * clamped so the clip head never shows before media time 0 at its start,
 * i.e. `offset_beats >= -length` is always legal but playback stays sane.
 * Pure: the caller writes the result back into its store.
 */
export function applyOffsetDrag(clip: VideoClip, deltaBeats: number): number {
  const next = clip.offset_beats + deltaBeats;
  if (!Number.isFinite(next)) throw new Error(`offset drag ${deltaBeats} is not finite`);
  // Lower bound: picture may not lead by more than the clip length
  // (beyond that the whole lane would be blank).
  return Math.max(next, -clip.length_beats);
}

/** Pad helper for timecode fields. */
function pad2(n: number): string {
  return n < 10 ? `0${n}` : `${n}`;
}

/**
 * Format `seconds` as `HH:MM:SS:FF` at `fps`. Negative clamps to zero;
 * frames round to nearest (half up) and carry into seconds.
 */
export function formatTimecode(seconds: number, fps: number): string {
  if (!(fps > 0 && Number.isFinite(fps))) throw new Error(`fps ${fps} must be > 0`);
  const totalFrames = Math.max(0, Math.round(seconds * fps));
  const ff = totalFrames % Math.round(fps);
  let rest = Math.floor(totalFrames / Math.round(fps));
  const ss = rest % 60;
  rest = Math.floor(rest / 60);
  const mm = rest % 60;
  const hh = Math.floor(rest / 60);
  return `${pad2(hh)}:${pad2(mm)}:${pad2(ss)}:${pad2(ff)}`;
}

/** Parse `HH:MM:SS:FF` back to seconds at `fps`. Throws on bad shape. */
export function parseTimecode(tc: string, fps: number): number {
  if (!(fps > 0 && Number.isFinite(fps))) throw new Error(`fps ${fps} must be > 0`);
  const m = /^(\d+):([0-5]?\d):([0-5]?\d):(\d+)$/.exec(tc.trim());
  if (!m) throw new Error(`bad timecode \`${tc}\` (want HH:MM:SS:FF)`);
  const [, hh, mm, ss, ff] = m;
  const frames = Number(ff);
  if (frames >= Math.round(fps)) throw new Error(`frame ${ff} out of range for ${fps}fps`);
  return (
    Number(hh) * 3600 + Number(mm) * 60 + Number(ss) + frames / Math.round(fps)
  );
}

/** Timecode of the transport position against the active clip (blank when none). */
export function timecodeForTransport(
  doc: VideoDoc,
  transportBeats: number,
  tempo: number,
): string | null {
  const clip = activeClip(doc, transportBeats);
  if (!clip) return null;
  const t = videoTimeForTransport(doc, transportBeats, tempo);
  if (t === null) return formatTimecode(0, clip.fps);
  return formatTimecode(t, clip.fps);
}

/** Demo doc: one 8-beat title card from beat 0, one 16-beat scene from 16. */
export function sampleVideoDoc(): VideoDoc {
  return {
    clips: [
      {
        id: "vid_title",
        name: "Title card",
        src: "take:vid_title",
        start_beats: 0,
        length_beats: 8,
        offset_beats: 0,
        fps: 30,
        duration_seconds: 16,
      },
      {
        id: "vid_scene",
        name: "Scene 1",
        src: "take:vid_scene",
        start_beats: 16,
        length_beats: 16,
        offset_beats: 0,
        fps: 30,
        duration_seconds: 32,
      },
    ],
  };
}
