import type { Project } from "../generated/project";

/**
 * Track G: mixer console model (TypeScript mirror of `core/src/mixer.rs`).
 *
 * The mixer is a view over the one routing graph: strips read tracks +
 * `Audio` edges, VCA trims multiply, bus gains multiply downstream, the
 * reference track (`ref_*` id or `[REF] ` name) never sounds in the mix,
 * and snapshots capture/recall fader state exactly. Nothing here changes
 * the frozen v0 schema — groups and snapshots are UI-local sidecars.
 */

export const SILENCE_DB = -120;

export function dbToGain(db: number): number {
  if (!Number.isFinite(db) || db <= SILENCE_DB) return 0;
  return Math.pow(10, db / 20);
}

export function gainToDb(gain: number): number {
  if (!Number.isFinite(gain) || gain <= 0) return SILENCE_DB;
  return 20 * Math.log10(gain);
}

export interface VcaGroup {
  id: string;
  name: string;
  members: string[];
  /** Linear trim, 1.0 = unity. */
  gain: number;
}

export function makeGroup(id: string, name: string, members: string[], gainDb: number): VcaGroup {
  if (!id) throw new Error("group id must be non-empty");
  if (!Number.isFinite(gainDb) || gainDb < SILENCE_DB || gainDb > 24) {
    throw new Error(`gainDb ${gainDb} out of [-120, +24]`);
  }
  if (new Set(members).size !== members.length) throw new Error(`group \`${id}\` has duplicate members`);
  return { id, name, members, gain: dbToGain(gainDb) };
}

export function vcaTrim(groups: VcaGroup[], trackId: string): number {
  return groups
    .filter((g) => g.members.includes(trackId))
    .reduce((acc, g) => acc * g.gain, 1);
}

export interface Strip {
  id: string;
  name: string;
  volume: number;
  pan: number;
  muted: boolean;
  solo: boolean;
  isReference: boolean;
  out: string | null;
}

export function isReferenceTrack(id: string, name: string): boolean {
  return id.startsWith("ref_") || name.startsWith("[REF] ");
}

/** One strip per track in project order, with its `Audio` out-target. */
export function strips(project: Project): Strip[] {
  const outs = new Map<string, string>();
  for (const e of project.routing) {
    if (e.kind === "Audio" && !outs.has(e.from_node)) outs.set(e.from_node, e.to_node);
  }
  return project.tracks.map((t) => ({
    id: t.id,
    name: t.name,
    volume: t.volume,
    pan: t.pan,
    muted: t.muted,
    solo: t.solo,
    isReference: isReferenceTrack(t.id, t.name),
    out: outs.get(t.id) ?? null,
  }));
}

function busGain(project: Project, busId: string): number {
  const dev = project.devices.find((d) => d.id === busId);
  if (!dev) return 1;
  return dev.params
    .filter((p) => p.id === "gain" || p.id === "volume")
    .reduce((acc, p) => acc * p.value, 1);
}

/** Audible linear gain: volume × VCA trims × downstream bus gains. */
export function effectiveTrackGain(project: Project, groups: VcaGroup[], trackId: string): number {
  const track = project.tracks.find((t) => t.id === trackId);
  if (!track) throw new Error(`unknown mixer track \`${trackId}\``);
  let bus = 1;
  const visited = new Set([trackId]);
  const frontier = project.routing
    .filter((e) => e.kind === "Audio" && e.from_node === trackId)
    .map((e) => e.to_node);
  while (frontier.length > 0) {
    const node = frontier.pop() as string;
    if (visited.has(node)) continue;
    visited.add(node);
    bus *= busGain(project, node);
    for (const e of project.routing) {
      if (e.kind === "Audio" && e.from_node === node) frontier.push(e.to_node);
    }
  }
  return track.volume * vcaTrim(groups, trackId) * bus;
}

export interface MixerSnapshot {
  name: string;
  volumes: Record<string, number>;
  pans: Record<string, number>;
  mutes: Record<string, boolean>;
  solos: Record<string, boolean>;
  groupGains: Record<string, number>;
}

export function captureSnapshot(project: Project, groups: VcaGroup[], name: string): MixerSnapshot {
  const snap: MixerSnapshot = { name, volumes: {}, pans: {}, mutes: {}, solos: {}, groupGains: {} };
  for (const t of project.tracks) {
    snap.volumes[t.id] = t.volume;
    snap.pans[t.id] = t.pan;
    snap.mutes[t.id] = t.muted;
    snap.solos[t.id] = t.solo;
  }
  for (const g of groups) snap.groupGains[g.id] = g.gain;
  return snap;
}

/** Recall a snapshot in place; unknown (added-later) tracks are left alone. */
export function recallSnapshot(project: Project, groups: VcaGroup[], snap: MixerSnapshot): void {
  for (const t of project.tracks) {
    if (snap.volumes[t.id] !== undefined) t.volume = snap.volumes[t.id];
    if (snap.pans[t.id] !== undefined) t.pan = snap.pans[t.id];
    if (snap.mutes[t.id] !== undefined) t.muted = snap.mutes[t.id];
    if (snap.solos[t.id] !== undefined) t.solo = snap.solos[t.id];
  }
  for (const g of groups) {
    if (snap.groupGains[g.id] !== undefined) g.gain = snap.groupGains[g.id];
  }
}

/** Strip ids whose fader state differs between two snapshots. */
export function snapshotDiff(a: MixerSnapshot, b: MixerSnapshot): string[] {
  const ids = new Set([...Object.keys(a.volumes), ...Object.keys(a.pans)]);
  return [...ids].filter(
    (id) =>
      a.volumes[id] !== b.volumes[id] ||
      a.pans[id] !== b.pans[id] ||
      a.mutes[id] !== b.mutes[id] ||
      a.solos[id] !== b.solos[id],
  );
}

// -- loudness-matched A/B --------------------------------------------------------

export function rms(samples: ArrayLike<number>): number {
  if (samples.length === 0) return 0;
  let sum = 0;
  for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i];
  return Math.sqrt(sum / samples.length);
}

export function loudnessDb(samples: ArrayLike<number>): number {
  return gainToDb(rms(samples));
}

/** Linear gain to apply to `b` so it matches `a` (1.0 when either is silent). */
export function matchGainFor(a: ArrayLike<number>, b: ArrayLike<number>): number {
  const ra = rms(a);
  const rb = rms(b);
  if (ra === 0 || rb === 0) return 1;
  return ra / rb;
}

/** Reference ids in project order (never in the mix — cued explicitly). */
export function referenceTracks(project: Project): string[] {
  return project.tracks.filter((t) => isReferenceTrack(t.id, t.name)).map((t) => t.id);
}
