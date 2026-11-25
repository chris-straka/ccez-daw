export { default as CueEditor } from "./CueEditor";
export { default as Cues } from "./CueEditor";
export {
  CUE_SCHEMA_VERSION,
  TRANSITION_KINDS,
  addLayer,
  audibleLayers,
  cloneCue,
  collectStates,
  describeTransition,
  layerMatrix,
  makeCue,
  makeLayer,
  makeTransition,
  normalizeRule,
  parseCue,
  removeLayer,
  removeTransition,
  serializeCue,
  setLayerClips,
  setLayerStates,
  setLayerVolume,
  stingerSlots,
  toggleLayerState,
  transitionFor,
  upsertTransition,
  validateCue,
} from "./cues";
export type { CueValidationOptions } from "./cues";
// Sibling GA-3/GA-4 modules share this barrel. `transitionFor` here is the
// editor's nullable lookup (null = Cut fallback); the simulator's same-named
// helper is re-exported under an alias so both stay reachable.
export * from "./sample";
export * from "./export";
export * from "./batch";
export {
  addTimelineEntry,
  advanceTransport,
  beatsToSamples,
  beatsToSeconds,
  clamp,
  effectiveKind,
  fireEvent,
  isDegraded,
  layersForState,
  makeFireRuntime,
  makeTransport,
  normalizeParam,
  previewGains,
  rampGainAt,
  resolveRtpc,
  secondsToBeats,
  snapshotAt,
  transportSeconds,
  valueFor,
} from "./model";
export { transitionFor as simulatorTransitionFor } from "./model";
// S-2 loop-seam audition: gapless preview + click meter + waiver helpers.
export { default as LoopAudition } from "./loopAudition";
export * from "./loop";
// S-1 fixture: TP-like demo package authoring builders + validator mirror.
export * from "./fixture";
export type {
  FireRecord,
  FireResult,
  FireRuntime,
  RejectKind,
  ResolvedRtpc,
  SimTransport,
  TimelineEntry,
} from "./model";
