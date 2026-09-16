import type { Node, Op } from "../generated/project";
import { paramSetOp } from "../scripting/ops";

/**
 * Device panels shared model: pure helpers over the frozen v0 schema.
 *
 * Every audible edit here becomes one `ParamSet` op per param against the
 * `deviceId:param` address (undoable via the existing `op_undo`, no new
 * IPC, no schema change). The drum pad *map* itself is UI-local sidecar
 * state — same precedent as the mixer VCA groups — because the frozen
 * `Node` carries numeric params only and cannot hold `(track, note)`
 * routing; per-pad *trim* (gain/transpose) still lands as `ParamSet` ops
 * on the pad's bound sampler device.
 */

/** Numeric class codes. Mirrors `CLASS_*` in `core/src/devices/class.rs`. */
export const DEVICE_CLASS_CODES = {
  gain: 1,
  lowpass: 2,
  highpass: 3,
  delay: 4,
  distortion: 5,
  sampler: 7,
  arpeggiator: 8,
  chord: 9,
  humanize: 10,
} as const;

export type DeviceClassName = keyof typeof DEVICE_CLASS_CODES;

export function deviceClassOf(node: Pick<Node, "params">): DeviceClassName | "foreign" {
  const tag = node.params.find((p) => p.id === "device_class")?.value;
  for (const [name, code] of Object.entries(DEVICE_CLASS_CODES)) {
    if (tag === code) return name as DeviceClassName;
  }
  return "foreign";
}

/** Read one param with a fallback default (missing = default). */
export function paramValue(
  node: Pick<Node, "params">,
  id: string,
  fallback: number,
): number {
  return node.params.find((p) => p.id === id)?.value ?? fallback;
}

/** Clamp a value to the node's own `[min, max]` for `id` (fallback passthrough). */
export function clampToNode(
  node: Pick<Node, "params">,
  id: string,
  value: number,
): number {
  const p = node.params.find((q) => q.id === id);
  if (!p) return value;
  return Math.min(p.max, Math.max(p.min, value));
}

/**
 * Expand a param patch to undoable ops: one `ParamSet` op per entry,
 * values clamped to the node's own ranges. Unknown param ids throw —
 * typos must surface, not vanish.
 */
export function deviceParamOps(
  actor: string,
  node: Pick<Node, "id" | "params">,
  patch: Record<string, number>,
): Op[] {
  const ids = new Set(node.params.map((p) => p.id));
  return Object.entries(patch).map(([param, value]) => {
    if (!ids.has(param)) throw new Error(`unknown device param ${node.id}:${param}`);
    return paramSetOp(actor, node.id, param, clampToNode(node, param, value));
  });
}

/** Sampler param ids. Mirrors `core/src/devices/kernel.rs`. */
export const SAMPLER_PARAMS = ["transpose", "gain", "attack", "release", "cutoff"] as const;

/** MIDI FX param ids. Mirrors `core/src/devices/midifx.rs`. */
export const ARP_PARAMS = ["arp_mode", "arp_rate", "arp_gate", "arp_octaves", "arp_seed"] as const;
export const CHORD_PARAMS = ["chord_type", "chord_inversion", "chord_voicing"] as const;
export const HUM_PARAMS = ["hum_timing", "hum_velocity", "hum_seed"] as const;

export interface SelectOption {
  code: number;
  label: string;
}

/** Arp pattern modes. Codes mirror `ArpMode` in `core/src/devices/midifx.rs`. */
export const ARP_MODE_OPTIONS: SelectOption[] = [
  { code: 0, label: "up" },
  { code: 1, label: "down" },
  { code: 2, label: "up-down" },
  { code: 3, label: "random" },
];

/** Chord qualities. Codes mirror `ChordType` in `core/src/devices/midifx.rs`. */
export const CHORD_TYPE_OPTIONS: SelectOption[] = [
  { code: 0, label: "maj" },
  { code: 1, label: "min" },
  { code: 2, label: "dim7" },
  { code: 3, label: "sus4" },
];

/** Chord voicings. Codes mirror `Voicing` in `core/src/devices/midifx.rs`. */
export const CHORD_VOICING_OPTIONS: SelectOption[] = [
  { code: 0, label: "close" },
  { code: 1, label: "drop-2" },
  { code: 2, label: "open" },
];

/** GM drum notes for the 4x4 grid, pad `i` → note. Kick/snare/hats first. */
export const DEFAULT_PAD_NOTES: readonly number[] = [
  36, 37, 38, 39,
  40, 41, 42, 43,
  44, 45, 46, 47,
  48, 49, 50, 51,
];

/** Display names for the default GM notes above. */
export const DEFAULT_PAD_NAMES: readonly string[] = [
  "Kick", "SideStick", "Snare", "Clap",
  "TomLo", "TomHi", "HatClosed", "TomMid",
  "HatPedal", "TomFloor", "HatOpen", "TomHi2",
  "Crash", "Crash2", "Ride", "RideBell",
];

export const DRUM_PAD_COUNT = 16;

export interface DrumPadState {
  pad: number;
  trackId: string;
  note: number;
  gain: number;
  transpose: number;
  /** Sampler device receiving this pad's trim as `ParamSet` ops ("" = unbound). */
  deviceId: string;
}

/** Default 16-pad map against one track (GM notes, unity trim, unbound). */
export function defaultDrumRack(trackId: string): DrumPadState[] {
  return DEFAULT_PAD_NOTES.map((note, pad) => ({
    pad,
    trackId,
    note,
    gain: 1,
    transpose: 0,
    deviceId: "",
  }));
}

function checkPad(pad: number): void {
  if (!Number.isInteger(pad) || pad < 0 || pad >= DRUM_PAD_COUNT) {
    throw new Error(`pad ${pad} out of range 0..16`);
  }
}

/** Retarget one pad's note (0..=127). Returns a new rack (immutable update). */
export function setPadNote(rack: DrumPadState[], pad: number, note: number): DrumPadState[] {
  checkPad(pad);
  if (!Number.isInteger(note) || note < 0 || note > 127) {
    throw new Error(`note ${note} out of range 0..=127`);
  }
  return rack.map((p) => (p.pad === pad ? { ...p, note } : p));
}

/** Retarget one pad's track. Returns a new rack (immutable update). */
export function setPadTrack(rack: DrumPadState[], pad: number, trackId: string): DrumPadState[] {
  checkPad(pad);
  if (!trackId) throw new Error("drum pad trackId must be non-empty");
  return rack.map((p) => (p.pad === pad ? { ...p, trackId } : p));
}

/** Bind one pad to a sampler device for trim edits. */
export function bindPadDevice(rack: DrumPadState[], pad: number, deviceId: string): DrumPadState[] {
  checkPad(pad);
  return rack.map((p) => (p.pad === pad ? { ...p, deviceId } : p));
}

/**
 * Per-pad trim as undoable ops: `gain` + `transpose` `ParamSet` ops against
 * the pad's bound device, clamped to the sampler ranges (gain 0..=4,
 * transpose ±48). Unbound pads yield no ops — nothing audible to address.
 */
export function padTrimOps(actor: string, pad: DrumPadState): Op[] {
  if (!pad.deviceId) return [];
  const gain = Math.min(4, Math.max(0, pad.gain));
  const transpose = Math.min(48, Math.max(-48, pad.transpose));
  return [
    paramSetOp(actor, pad.deviceId, "gain", gain),
    paramSetOp(actor, pad.deviceId, "transpose", transpose),
  ];
}
