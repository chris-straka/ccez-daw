/**
 * WAV import helpers behind the Timeline's import control: validate a
 * PCM16-mono WAV the same way the engine decoder will (so a file the UI
 * accepts always renders), derive its length in beats, and shape the
 * `ClipAdded` op that places it.
 */
import { ClipSchema, OpSchema, type Clip, type Op, type OpKind } from "../generated/project";

export class ImportError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

export interface WavInfo {
  sampleRate: number;
  numFrames: number;
}

function u16le(b: Uint8Array, o: number): number {
  return b[o]! | (b[o + 1]! << 8);
}

function u32le(b: Uint8Array, o: number): number {
  return b[o]! | (b[o + 1]! << 8) | (b[o + 2]! << 16) | (b[o + 3]! << 24);
}

/** Validate a PCM16-mono WAV header; mirror of `decode_wav` in core. */
export function parseWavInfo(bytes: Uint8Array): WavInfo {
  const bad = (m: string) => new ImportError("bad-wav", `not an importable WAV: ${m}`);
  if (
    bytes.length < 44 ||
    bytes[0] !== 0x52 || bytes[1] !== 0x49 || bytes[2] !== 0x46 || bytes[3] !== 0x46 ||
    bytes[8] !== 0x57 || bytes[9] !== 0x41 || bytes[10] !== 0x56 || bytes[11] !== 0x45
  ) {
    throw bad("want RIFF/WAVE");
  }
  let pos = 12;
  let sampleRate = 0;
  let numFrames = 0;
  let sawData = false;
  while (pos + 8 <= bytes.length) {
    const len = u32le(bytes, pos + 4);
    const body = bytes.subarray(pos + 8, pos + 8 + len);
    if (body.length !== len) throw bad("truncated chunk");
    const id = String.fromCharCode(bytes[pos]!, bytes[pos + 1]!, bytes[pos + 2]!, bytes[pos + 3]!);
    if (id === "fmt ") {
      if (len < 16 || u16le(body, 0) !== 1 || u16le(body, 2) !== 1 || u16le(body, 14) !== 16) {
        throw bad("want PCM16 mono (engine decoder)");
      }
      sampleRate = u32le(body, 4);
      if (sampleRate === 0) throw bad("zero sample rate");
    } else if (id === "data") {
      if (len % 2 !== 0) throw bad("odd data length");
      numFrames = len / 2;
      sawData = true;
    }
    pos += 8 + len;
  }
  if (sampleRate === 0) throw bad("missing fmt chunk");
  if (!sawData || numFrames === 0) throw bad("no audio frames");
  return { sampleRate, numFrames };
}

/** Bare-filename asset key from an upload name (mirrors `check_asset_key`). */
export function sanitizeAssetKey(name: string): string {
  const base = name.split(/[\\/]/).pop() ?? "";
  const clean = base.replace(/[^A-Za-z0-9._-]+/g, "_").replace(/\.\.+/g, "_");
  if (clean.length === 0 || clean === "." || clean === "..") {
    throw new ImportError("bad-key", `cannot make an asset key from ${JSON.stringify(name)}`);
  }
  return clean;
}

/** Clip length in beats for `numFrames` at `sampleRate` under `tempo`. */
export function beatsForFrames(numFrames: number, sampleRate: number, tempo: number): number {
  if (!(tempo > 0) || !Number.isFinite(tempo)) {
    throw new ImportError("bad-tempo", `tempo must be positive finite, got ${tempo}`);
  }
  const beats = (numFrames / sampleRate) * (tempo / 60);
  if (!(beats > 0) || !Number.isFinite(beats)) {
    throw new ImportError("bad-length", "WAV holds no audio time");
  }
  return Math.round(beats * 100) / 100;
}

export function importClipId(trackId: string, startBeats: number, key: string): string {
  return `clip_${trackId}_${startBeats}_${key}`.replaceAll(".", "_").replaceAll(/[^A-Za-z0-9_-]/g, "_");
}

/** The placed Audio clip naming the stored asset bytes. */
export function buildImportClip(
  trackId: string,
  startBeats: number,
  name: string,
  key: string,
  lengthBeats: number,
): Clip {
  return ClipSchema.parse({
    id: importClipId(trackId, startBeats, key),
    track_id: trackId,
    name,
    start_beats: startBeats,
    length_beats: lengthBeats,
    kind: "Audio",
    source: `asset:${key}`,
  });
}

/** Frozen `ClipAdded` op placing an imported clip (undoable like a take). */
export function importClipOp(actor: string, clip: Clip): Op {
  return OpSchema.parse({
    seq: 0,
    actor,
    kind: "ClipAdded" as OpKind,
    target: clip.id,
    value_json: JSON.stringify(clip),
  });
}
