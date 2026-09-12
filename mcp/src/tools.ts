import { z } from "zod";

/**
 * v0 MCP tool list stub. Each tool mirrors one action-registry entry
 * (`contracts/action-registry.md`); Track L implements the handlers against
 * the same op log the UI uses. Shapes reuse the generated project types'
 * field names so the three surfaces (palette / vim / MCP) cannot diverge.
 */

export const ProjectRef = z.object({ projectId: z.string() });
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

export type ToolName = (typeof TOOL_NAMES)[number];

/**
 * Track L handlers: each frozen tool runs against an `InMemoryBackend`
 * (or any matching seam) and every mutation lands in the op log with
 * actor `"mcp"`. Handlers take plain args and return plain JSON-able
 * values; `index.ts` wraps them into MCP `content` blocks.
 */

import type { InMemoryBackend, NewClip } from "./backend.js";

export type ToolHandler = (args: any) => Promise<unknown> | unknown;

function requireString(value: unknown, what: string): string {
  if (typeof value !== "string" || !value) {
    throw new Error(`${what} must be a non-empty string`);
  }
  return value;
}

/** Build the handler table for one backend (one per server instance). */
export function createToolHandlers(
  backend: InMemoryBackend,
): Record<ToolName, ToolHandler> {
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
];
