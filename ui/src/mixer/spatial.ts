import type { Node, Project } from "../generated/project";
import { deviceClassOf } from "../devices/model";

/**
 * Track G: first-order ambisonic spatial monitor (TypeScript mirror of
 * `core/src/spatial.rs`).
 *
 * Fully open Ambisonics is the buildable spatial path for game-audio SFX;
 * Dolby Atmos is proprietary/licensed and out of scope. A track's placement
 * rides an ordinary frozen `Spatial` device node (`azimuth`/`elevation`/
 * `gain` params, `device_class` 11) edited through `ParamSet` ops — no
 * schema change. Conventions match Rust exactly: degrees on params,
 * azimuth 0 = front-center (+ = left), elevation 0 = horizon (+ = up),
 * unit (N3D-style) normalization (`W = s*g`), and a virtual-cardioid
 * HRTF-free stereo decode (`L = (W + X/2 + Y)/2`, `R = (W + X/2 - Y)/2`).
 * The decode is a placement monitor, not a binaural HRTF render.
 */

export type Wxyz = [number, number, number, number];
export type Stereo = [number, number];

export interface SpatialParams {
  azimuth: number;
  elevation: number;
  gain: number;
}

export const DEFAULT_SPATIAL_PARAMS: SpatialParams = { azimuth: 0, elevation: 0, gain: 1 };

export const AZIMUTH_RANGE = { min: -180, max: 180 } as const;
export const ELEVATION_RANGE = { min: -90, max: 90 } as const;
export const SPATIAL_GAIN_RANGE = { min: 0, max: 4 } as const;

export function degToRad(deg: number): number {
  return (deg * Math.PI) / 180;
}

/** Direction unit vector `[front, left, up]` for radian angles. */
export function direction(azimuthRad: number, elevationRad: number): [number, number, number] {
  return [
    Math.cos(elevationRad) * Math.cos(azimuthRad),
    Math.cos(elevationRad) * Math.sin(azimuthRad),
    Math.sin(elevationRad),
  ];
}

/** Encode one mono sample into FOA `WXYZ`. */
export function encodeMono(
  sample: number,
  azimuthDeg: number,
  elevationDeg: number,
  gain: number,
): Wxyz {
  const [front, left, up] = direction(degToRad(azimuthDeg), degToRad(elevationDeg));
  const s = sample * gain;
  return [s, s * front, s * left, s * up];
}

/**
 * Encode one stereo sample into FOA `WXYZ`: `L` at `azimuth - spread/2`,
 * `R` at `azimuth + spread/2`, averaged. `spreadDeg == 0` collapses exactly
 * to the mono encoding of the mid `(l+r)/2`.
 */
export function encodeStereo(
  l: number,
  r: number,
  azimuthDeg: number,
  elevationDeg: number,
  gain: number,
  spreadDeg: number,
): Wxyz {
  const a = encodeMono(l, azimuthDeg - spreadDeg / 2, elevationDeg, gain);
  const b = encodeMono(r, azimuthDeg + spreadDeg / 2, elevationDeg, gain);
  return [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1]), 0.5 * (a[2] + b[2]), 0.5 * (a[3] + b[3])];
}

/** Decode one FOA `WXYZ` frame to a stereo monitor pair (HRTF-free). */
export function decodeStereo([w, x, y]: Wxyz): Stereo {
  return [0.5 * (w + 0.5 * x + y), 0.5 * (w + 0.5 * x - y)];
}

/** Mono-chain insert gain: the field's `(L+R)` projection, degrees clamped. */
export function insertGain(azimuthDeg: number, elevationDeg: number, gain: number): number {
  const az = Math.min(180, Math.max(-180, azimuthDeg));
  const el = Math.min(90, Math.max(-90, elevationDeg));
  const [front] = direction(degToRad(az), degToRad(el));
  return gain * (1 + front / 2);
}

/** Stereo monitor pair for a mono block through one placement. */
export function monitorStereo(
  samples: ArrayLike<number>,
  params: SpatialParams,
): { left: number[]; right: number[] } {
  const left: number[] = new Array(samples.length);
  const right: number[] = new Array(samples.length);
  for (let i = 0; i < samples.length; i++) {
    const [l, r] = decodeStereo(encodeMono(samples[i], params.azimuth, params.elevation, params.gain));
    left[i] = l;
    right[i] = r;
  }
  return { left, right };
}

/** Read a device node's spatial params (missing node or params = defaults). */
export function spatialParamsOf(node: Pick<Node, "params"> | null | undefined): SpatialParams {
  if (!node) return { ...DEFAULT_SPATIAL_PARAMS };
  const at = (id: string, fallback: number): number =>
    node.params.find((p) => p.id === id)?.value ?? fallback;
  return {
    azimuth: at("azimuth", DEFAULT_SPATIAL_PARAMS.azimuth),
    elevation: at("elevation", DEFAULT_SPATIAL_PARAMS.elevation),
    gain: at("gain", DEFAULT_SPATIAL_PARAMS.gain),
  };
}

/** First `Spatial`-class device id on the track's chain (none = undefined). */
export function spatialDeviceForTrack(project: Project, trackId: string): Node | undefined {
  const track = project.tracks.find((t) => t.id === trackId);
  if (!track) throw new Error(`unknown mixer track \`${trackId}\``);
  for (const devId of track.device_ids) {
    const node = project.devices.find((d) => d.id === devId);
    if (node && deviceClassOf(node) === "spatial") return node;
  }
  return undefined;
}

/** A track's placement (defaults when it carries no `Spatial` device). */
export function trackSpatialParams(project: Project, trackId: string): SpatialParams {
  return spatialParamsOf(spatialDeviceForTrack(project, trackId));
}

/** Stereo monitor pair for a mono block through the track's placement. */
export function trackMonitorStereo(
  project: Project,
  trackId: string,
  samples: ArrayLike<number>,
): { left: number[]; right: number[] } {
  return monitorStereo(samples, trackSpatialParams(project, trackId));
}
