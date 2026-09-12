// Patch-graph layout: deterministic layered positions over the one routing
// model. Framework-free (no DOM, no Solid): the SVG component and the
// scripted test share these functions.
//
// The algorithm mirrors `AudioGraph::topo_order` in
// `core/src/audio/graph.rs`: Kahn's algorithm over `Audio` + `Midi` edges
// only. `Modulation` + `Sidechain` edges are control-rate (previous-block
// reads), so they never impose order and never create cycles — a feedback
// LFO lays out fine, a feedback audio cable reports a cycle.

import type { Edge, Project } from "../generated/project";
import { collectNodeIds, isSignalKind } from "./model";

export const LAYER_DX = 180;
export const LAYER_DY = 56;
export const NODE_W = 140;
export const NODE_H = 34;

export type TopoResult = { ok: true; order: string[] } | { ok: false; cycle: string[] };

/**
 * Signal-flow order, TS mirror of `AudioGraph::topo_order`. Returns the
 * cycle members (sorted) instead of throwing, so the view can paint them.
 */
export function signalTopoOrder(project: Project): TopoResult {
  const nodes = collectNodeIds(project);
  const succ = new Map<string, string[]>();
  for (const id of nodes) succ.set(id, []);
  for (const e of project.routing) {
    if (isSignalKind(e.kind)) succ.get(e.from_node)?.push(e.to_node);
  }
  const indegree = new Map<string, number>();
  for (const id of nodes) indegree.set(id, 0);
  for (const tos of succ.values()) {
    for (const to of tos) indegree.set(to, (indegree.get(to) ?? 0) + 1);
  }
  const ready = nodes.filter((id) => indegree.get(id) === 0);
  const order: string[] = [];
  while (ready.length > 0) {
    const id = ready.shift()!;
    order.push(id);
    for (const next of succ.get(id) ?? []) {
      indegree.set(next, indegree.get(next)! - 1);
      if (indegree.get(next) === 0) ready.push(next);
    }
  }
  if (order.length !== nodes.length) {
    const cycle = nodes.filter((id) => indegree.get(id)! > 0).sort();
    return { ok: false, cycle };
  }
  return { ok: true, order };
}

export interface PositionedNode {
  id: string;
  role: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** Signal-flow layer (0 = sources). Nodes in cycles share layer 0. */
  layer: number;
  inCycle: boolean;
}

export interface PatchLayout {
  nodes: PositionedNode[];
  edges: Edge[];
  cycle: string[];
}

function roleOf(project: Project, id: string): string {
  if (project.tracks.some((t) => t.id === id)) return "Track";
  const device = project.devices.find((d) => d.id === id);
  if (device) return String(device.kind);
  if (project.clips.some((c) => c.id === id)) return "Clip";
  return "Bus";
}

/**
 * Layered layout: a node's layer is the longest signal path feeding it
 * (0 for sources), so cables always flow left-to-right. Rows within a
 * layer follow sorted id order — fully deterministic, no simulation.
 */
export function layoutPatch(project: Project): PatchLayout {
  const topo = signalTopoOrder(project);
  const cycle = topo.ok ? [] : topo.cycle;
  const inCycle = new Set(cycle);
  const order = topo.ok ? topo.order : collectNodeIds(project);

  const preds = new Map<string, string[]>();
  for (const id of collectNodeIds(project)) preds.set(id, []);
  if (topo.ok) {
    for (const e of project.routing) {
      if (isSignalKind(e.kind)) preds.get(e.to_node)?.push(e.from_node);
    }
  }
  const layer = new Map<string, number>();
  for (const id of order) {
    if (inCycle.has(id)) {
      layer.set(id, 0);
      continue;
    }
    const depth = (preds.get(id) ?? []).map((p) => layer.get(p) ?? 0);
    layer.set(id, depth.length === 0 ? 0 : Math.max(...depth) + 1);
  }

  const byLayer = new Map<number, string[]>();
  for (const id of order) {
    const l = layer.get(id) ?? 0;
    if (!byLayer.has(l)) byLayer.set(l, []);
    byLayer.get(l)!.push(id);
  }
  for (const ids of byLayer.values()) ids.sort();

  const nodes: PositionedNode[] = [];
  for (const [l, ids] of [...byLayer.entries()].sort((a, b) => a[0] - b[0])) {
    ids.forEach((id, row) => {
      nodes.push({
        id,
        role: roleOf(project, id),
        x: l * LAYER_DX,
        y: row * LAYER_DY,
        w: NODE_W,
        h: NODE_H,
        layer: l,
        inCycle: inCycle.has(id),
      });
    });
  }
  nodes.sort((a, b) => a.layer - b.layer || a.y - b.y || (a.id < b.id ? -1 : 1));
  return { nodes, edges: [...project.routing], cycle };
}

/** Cable endpoints in layout space (right edge -> left edge of the boxes). */
export function cablePoints(layout: PatchLayout, edge: Edge): { x1: number; y1: number; x2: number; y2: number } | null {
  const from = layout.nodes.find((n) => n.id === edge.from_node);
  const to = layout.nodes.find((n) => n.id === edge.to_node);
  if (!from || !to) return null;
  return {
    x1: from.x + from.w,
    y1: from.y + from.h / 2,
    x2: to.x,
    y2: to.y + to.h / 2,
  };
}
