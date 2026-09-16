/** Agent 2 (video track) public surface: sidecar model + sync + views. */
export type { VideoClip, VideoDoc } from "./model";
export type { ClipThumbMap, StripFrameLike, ThumbCell } from "./thumbnails";
export {
  cellsForClip,
  isMediaSrc,
  placeholderCell,
  placeholderCells,
  stripToCells,
} from "./thumbnails";
export {
  DEFAULT_FPS,
  FILMSTRIP_THUMBS,
  applyOffsetDrag,
  beatsToSeconds,
  formatTimecode,
  parseTimecode,
  sampleVideoDoc,
  secondsToBeats,
  timecodeForTransport,
  transportBeatsForVideoTime,
  validateVideoClip,
  videoTimeForTransport,
} from "./model";
export type { PlayState, SyncAction, SyncInput } from "./sync";
export { SEEK_THRESHOLD_S, STALL_THRESHOLD_S, applySyncDecision, syncDecision } from "./sync";
export { default as FilmstripLane } from "./Filmstrip";
export { default as VideoPreview } from "./Preview";
