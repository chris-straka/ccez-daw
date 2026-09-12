// Visual patch graph: pure graph-edit model over the ONE routing table.
//
// The frozen v0 `Edge { id, from_node, from_port, to_node, to_port, kind }`
// list in `contracts/project-schema.md` is the single routing model: the
// mixer is a view over the `Audio` edges, and this patch graph is a view
// over all four `EdgeKind`s. Nothing here changes the schema — every edit
// builds or removes plain `Edge` values validated by the generated
// `EdgeSchema`, so `bun run typegen -- --check` stays green.
//
// Mirrors `core/src/audio/graph.rs`: `Audio` + `Midi` edges carry signal
// (they define order), `Modulation` + `Sidechain` edges are control-rate
// (previous-block reads, never order, never cycle). The layout half of
// that mirror lives in `./layout`.

import { EdgeSchema } from "../generated/project";
import type { Edge, EdgeKind, Project } from "../generated/project";

/** All addressable node ids: tracks, devices, clips, plus implicit edge endpoints. */
export function collectNodeIds(project: Project): string[] {
  const ids = new Set<string>();
  for (const t of project.tracks) ids.add(t.id);
  for (const d of project.devices) ids.add(d.id);
  for (const c of project.clips) ids.add(c.id);
  for (const e of project.routing) {
    ids.add(e.from_node);
    ids.add(e.to_node);
  }
  return [...ids].sort();
}

/** Best-effort node role for display. Unknown endpoints are mix buses / ports. */
export function nodeRoleOf(project: Project, id: string): string {
  if (project.tracks.some((t) => t.id === id)) return "Track";
  const device = project.devices.find((d) => d.id === id);
  if (device) return device.kind;
  if (project.clips.some((c) => c.id === id)) return "Clip";
  return "Bus";
}

/** Signal edges define order; control edges never do (mirrors `AudioGraph`). */
export function isSignalKind(kind: EdgeKind): boolean {
  return kind === "Audio" || kind === "Midi";
}

/** Throw a human message when `edge` cannot join `project.routing`. */
export function validateEdge(project: Project, edge: Edge): void {
  EdgeSchema.parse(edge);
  if (!edge.id) throw new Error("edge id must be non-empty");
  if (!edge.from_node || !edge.to_node) throw new Error("edge endpoints must be non-empty");
  if (!edge.from_port || !edge.to_port) throw new Error("edge ports must be non-empty");
  if (project.routing.some((e) => e.id === edge.id)) {
    throw new Error(`duplicate edge id \`${edge.id}\``);
  }
  if (
    project.routing.some(
      (e) =>
        e.from_node === edge.from_node &&
        e.from_port === edge.from_port &&
        e.to_node === edge.to_node &&
        e.to_port === edge.to_port &&
        e.kind === edge.kind,
    )
  ) {
    throw new Error(
      `duplicate cable ${edge.from_node}:${edge.from_port} -> ${edge.to_node}:${edge.to_port} (${edge.kind})`,
    );
  }
  if (isSignalKind(edge.kind) && edge.from_node === edge.to_node) {
    throw new Error(`signal self-loop on \`${edge.from_node}\` would be an audio cycle`);
  }
}

/** Immutable append: the patch cable becomes a frozen `Edge` row. */
export function addEdge(project: Project, edge: Edge): Project {
  const parsed = EdgeSchema.parse(edge);
  validateEdge(project, parsed);
  return { ...project, routing: [...project.routing, parsed] };
}

export interface RemoveResult {
  project: Project;
  removed: boolean;
}

/** Immutable delete by edge id. Unknown ids are a no-op (`removed: false`). */
export function removeEdge(project: Project, edgeId: string): RemoveResult {
  const routing = project.routing.filter((e) => e.id !== edgeId);
  return { project: { ...project, routing }, removed: routing.length !== project.routing.length };
}

export interface ReconnectPatch {
  from_node?: string;
  from_port?: string;
  to_node?: string;
  to_port?: string;
}

/**
 * Immutable re-patch: drag a cable end onto a new node/port. Keeps the
 * edge id and kind (re-kind via remove + add instead, so duplicate-cable
 * checks stay exact). Throws on unknown ids and on signal self-loops.
 */
export function reconnectEdge(project: Project, edgeId: string, patch: ReconnectPatch): Project {
  const current = project.routing.find((e) => e.id === edgeId);
  if (!current) throw new Error(`unknown edge \`${edgeId}\``);
  const next: Edge = EdgeSchema.parse({
    ...current,
    from_node: patch.from_node ?? current.from_node,
    from_port: patch.from_port ?? current.from_port,
    to_node: patch.to_node ?? current.to_node,
    to_port: patch.to_port ?? current.to_port,
  });
  if (!next.from_node || !next.to_node) throw new Error("edge endpoints must be non-empty");
  if (!next.from_port || !next.to_port) throw new Error("edge ports must be non-empty");
  if (isSignalKind(next.kind) && next.from_node === next.to_node) {
    throw new Error(`signal self-loop on \`${next.from_node}\` would be an audio cycle`);
  }
  const clash = project.routing.some(
    (e) =>
      e.id !== edgeId &&
      e.from_node === next.from_node &&
      e.from_port === next.from_port &&
      e.to_node === next.to_node &&
      e.to_port === next.to_port &&
      e.kind === next.kind,
  );
  if (clash) throw new Error(`reconnect would duplicate an existing ${next.kind} cable`);
  return { ...project, routing: project.routing.map((e) => (e.id === edgeId ? next : e)) };
}

/** All cables of one kind, in routing order. */
export function edgesOfKind(project: Project, kind: EdgeKind): Edge[] {
  return project.routing.filter((e) => e.kind === kind);
}

/** Signal cables leaving `nodeId` (the mixer's downstream view). */
export function cablesFrom(project: Project, nodeId: string): Edge[] {
  return project.routing.filter((e) => e.from_node === nodeId && isSignalKind(e.kind));
}

/** Signal cables entering `nodeId` (one mix point's inputs). */
export function cablesTo(project: Project, nodeId: string): Edge[] {
  return project.routing.filter((e) => e.to_node === nodeId && isSignalKind(e.kind));
}

/** Serialize the routing table for clipboard/file round-trips. */
export function patchToJson(project: Project): string {
  return JSON.stringify(project.routing);
}

/**
 * Replace the routing table from `patchToJson` output (or any external
 * `Edge[]` JSON). Validates every row against the frozen schema plus the
 * same duplicate/cycle rules as interactive edits, so a dropped file can
 * never smuggle in drift. Returns a new project; the input is untouched.
 */
export function patchFromJson(project: Project, json: string): Project {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    throw new Error("patch decode: invalid JSON");
  }
  if (!Array.isArray(raw)) throw new Error("patch decode: expected an Edge array");
  const edges = raw.map((row) => EdgeSchema.parse(row));
  const ids = new Set<string>();
  for (const e of edges) {
    if (ids.has(e.id)) throw new Error(`duplicate edge id \`${e.id}\``);
    ids.add(e.id);
    if (isSignalKind(e.kind) && e.from_node === e.to_node) {
      throw new Error(`signal self-loop on \`${e.from_node}\` would be an audio cycle`);
    }
  }
  return { ...project, routing: edges };
}
