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
