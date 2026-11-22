import { z } from "zod";

/**
 * v0 MCP tool list stub. Each tool mirrors one action-registry entry
 * (`contracts/action-registry.md`); Track L implements the handlers against
 * the same op log the UI uses. Shapes reuse the generated project types'
 * field names so the three surfaces (palette / vim / MCP) cannot diverge.
 */

export const ProjectRef = z.object({ projectId: z.string() });

/** Post-v0 coverage input shapes (field names mirror the registry + core). */
export const AutomationPointInput = z.object({
  lane: z.string(),
  beat: z.number(),
  value: z.number(),
  node: z.string().optional(),
  param: z.string().optional(),
});
export const SessionLaunchInput = z.object({
  pressBeat: z.number(),
  gridBeats: z.number(),
});
export const CompCommitInput = z.object({
  trackId: z.string(),
  name: z.string(),
  startBeats: z.number(),
  lengthBeats: z.number(),
  kind: z.enum(["Audio", "Midi"]),
  takes: z.array(z.string()),
});
export const GrooveApplyInput = z.object({
  clipId: z.string(),
  groove: z.string(),
  amount: z.number(),
});
export const MergeOpSchema = z.object({
  kind: z.enum([
    "TrackAdded",
    "ClipAdded",
    "ClipMoved",
    "ParamSet",
    "TempoSet",
    "UndoMarker",
    "AutomationPointSet",
    "GameAudioExported",
  ]),
  target: z.string(),
  value_json: z.string(),
});
export const BranchMergeInput = z.object({
  source: z.string(),
  target: z.string(),
  sourceOps: z.array(MergeOpSchema).default([]),
});
export const PunchCommitInput = z.object({
  trackId: z.string(),
  name: z.string(),
  kind: z.enum(["Audio", "Midi"]),
  startBeats: z.number(),
  endBeats: z.number(),
});
export const LinkJoinInput = z.object({
  session: z.string(),
  peer: z.string(),
  tempo: z.number().optional(),
});
export const TrackRef = z.object({ trackId: z.string() });
export const ClipInput = z.object({
  trackId: z.string(),
  name: z.string(),
  startBeats: z.number(),
  lengthBeats: z.number(),
  kind: z.enum(["Audio", "Midi"]),
});
export const ParamInput = z.object({
  node: z.string(),
  param: z.string(),
  value: z.number(),
});

export const JamRecordInput = z.object({
  clips: z.array(ClipInput),
});

export interface McpToolDef {
  name: string;
  description: string;
  action: string;
}

/** Frozen tool names (contracts/mcp-tools.md). Handlers below are additive. */
export const TOOL_NAMES = [
  "project_get",
  "project_list_tracks",
  "project_add_clip",
  "param_set",
  "transport_play",
  "transport_stop",
] as const;

/**
 * Post-v0 coverage tool names (contracts/mcp-tools.md, "Additive post-v0
 * coverage tools" rows).
 */
export const COVERAGE_TOOL_NAMES = [
  "automation_set_point",
  "session_launch",
  "session_jam_record",
  "comp_commit",
  "groove_apply",
  "branch_merge",
  "record_punch",
  "link_join",
] as const;

export type CoverageToolName = (typeof COVERAGE_TOOL_NAMES)[number];

export type ToolName = (typeof TOOL_NAMES)[number];

/**
 * Track L handlers: each frozen tool runs against an `InMemoryBackend`
 * (or any matching seam) and every mutation lands in the op log with
 * actor `"mcp"`. Handlers take plain args and return plain JSON-able
 * values; `index.ts` wraps them into MCP `content` blocks.
 */

import type { InMemoryBackend, MergeOpInput, NewClip } from "./backend.js";

export type ToolHandler = (args: any) => Promise<unknown> | unknown;

function requireString(value: unknown, what: string): string {
  if (typeof value !== "string" || !value) {
    throw new Error(`${what} must be a non-empty string`);
  }
  return value;
}

/**
 * Build the handler table for one backend (one per server instance).
 */
export function createToolHandlers(
  backend: InMemoryBackend,
): Record<ToolName | CoverageToolName, ToolHandler> {
  return {
    project_get: () => backend.getProject(),
    project_list_tracks: () =>
      backend
        .listTracks()
        .map(({ id, name, volume, pan, muted, solo }) => ({
          id,
          name,
          volume,
          pan,
          muted,
          solo,
        })),
    project_add_clip: (args: {
      trackId: string;
      name: string;
      startBeats: number;
      lengthBeats: number;
      kind: "Audio" | "Midi";
    }) => {
      const input: NewClip = {
        trackId: requireString(args?.trackId, "trackId"),
        name: requireString(args?.name, "name"),
        startBeats: args?.startBeats,
        lengthBeats: args?.lengthBeats,
        kind: args?.kind,
      };
      return backend.addClip(input);
    },
    param_set: (args: { node: string; param: string; value: number }) =>
      backend.setParam(
        requireString(args?.node, "node"),
        requireString(args?.param, "param"),
        args?.value,
      ),
    transport_play: () => ({ state: backend.play() }),
    transport_stop: () => ({ state: backend.stop() }),
    automation_set_point: (args: {
      lane: string;
      beat: number;
      value: number;
      node?: string;
      param?: string;
    }) =>
      backend.setAutomationPoint(
        requireString(args?.lane, "lane"),
        args?.beat,
        args?.value,
        args?.node,
        args?.param,
      ),
    session_launch: (args: { pressBeat: number; gridBeats: number }) =>
      backend.planLaunch(args?.pressBeat, args?.gridBeats),
    session_jam_record: (args: { clips: NewClip[] }) => {
      if (!Array.isArray(args?.clips)) throw new Error("clips must be an array");
      return backend.jamRecord(
        args.clips.map((c) => ({
          trackId: requireString(c?.trackId, "trackId"),
          name: requireString(c?.name, "name"),
          startBeats: c?.startBeats,
          lengthBeats: c?.lengthBeats,
          kind: c?.kind,
        })),
      );
    },
    comp_commit: (args: {
      trackId: string;
      name: string;
      startBeats: number;
      lengthBeats: number;
      kind: "Audio" | "Midi";
      takes: string[];
    }) =>
      backend.commitComp({
        trackId: requireString(args?.trackId, "trackId"),
        name: requireString(args?.name, "name"),
        startBeats: args?.startBeats,
        lengthBeats: args?.lengthBeats,
        kind: args?.kind,
        takes: args?.takes,
      }),
    groove_apply: (args: { clipId: string; groove: string; amount: number }) =>
      backend.applyGroove(
        requireString(args?.clipId, "clipId"),
        requireString(args?.groove, "groove"),
        args?.amount,
      ),
    branch_merge: (args: { source: string; target: string; sourceOps?: MergeOpInput[] }) =>
      backend.mergeBranch(
        requireString(args?.source, "source"),
        requireString(args?.target, "target"),
        Array.isArray(args?.sourceOps) ? args.sourceOps : [],
      ),
    record_punch: (args: {
      trackId: string;
      name: string;
      kind: "Audio" | "Midi";
      startBeats: number;
      endBeats: number;
    }) =>
      backend.commitPunch({
        trackId: requireString(args?.trackId, "trackId"),
        name: requireString(args?.name, "name"),
        kind: args?.kind,
        startBeats: args?.startBeats,
        endBeats: args?.endBeats,
      }),
    link_join: (args: { session: string; peer: string; tempo?: number }) =>
      backend.joinLink(
        requireString(args?.session, "session"),
        requireString(args?.peer, "peer"),
        args?.tempo,
      ),
  };
}

export const TOOLS: McpToolDef[] = [
  {
    name: "project_get",
    description: "Return the currently open project.",
    action: "project.get",
  },
  {
    name: "project_list_tracks",
    description: "List tracks (id, name, volume, pan, mute/solo).",
    action: "track.list",
  },
  {
    name: "project_add_clip",
    description: "Add a clip to a track; returns the clip id.",
    action: "clip.add",
  },
  {
    name: "param_set",
    description: "Set one addressed parameter (node:param).",
    action: "param.set",
  },
  {
    name: "transport_play",
    description: "Start the transport.",
    action: "transport.play",
  },
  {
    name: "transport_stop",
    description: "Stop the transport.",
    action: "transport.stop",
  },
  {
    name: "automation_set_point",
    description: "Upsert one automation point (creates the lane); returns the op seq.",
    action: "automation.point_set",
  },
  {
    name: "session_launch",
    description: "Quantize a press beat to the next grid boundary (pure plan, no writes).",
    action: "session.launch",
  },
  {
    name: "session_jam_record",
    description: "Record jam clips to the timeline; returns clip ids and op seqs.",
    action: "session.jam_record",
  },
  {
    name: "comp_commit",
    description: "Commit a composite take from named takes; returns the clip id.",
    action: "comp.commit",
  },
  {
    name: "groove_apply",
    description: "Stamp a grooved copy of a clip; returns the new clip id.",
    action: "groove.apply",
  },
  {
    name: "branch_merge",
    description: "Merge source ops against the log; returns merged seqs + conflicts.",
    action: "branch.merge",
  },
  {
    name: "record_punch",
    description: "Commit one punched take; returns the clip id.",
    action: "record.punch",
  },
  {
    name: "link_join",
    description: "Join a Link session clock; returns tempo and peers (no writes).",
    action: "link.join",
  },
];
