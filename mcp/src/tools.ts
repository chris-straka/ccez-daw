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

/**
 * GA-5 game-audio tool names (contracts/export-package.md, "Game-audio MCP
 * tools" rows). New names only — the six v0 names above are untouched and
 * stay first, so prefix checks on the frozen list keep passing.
 */
export const GAMEAUDIO_TOOL_NAMES = [
  "gameaudio_list_cues",
  "gameaudio_audition",
  "gameaudio_trigger_sfx",
  "gameaudio_export",
] as const;

export type GameAudioToolName = (typeof GAMEAUDIO_TOOL_NAMES)[number];

export type ToolName = (typeof TOOL_NAMES)[number];

/**
 * Track L handlers: each frozen tool runs against an `InMemoryBackend`
 * (or any matching seam) and every mutation lands in the op log with
 * actor `"mcp"`. Handlers take plain args and return plain JSON-able
 * values; `index.ts` wraps them into MCP `content` blocks.
 */

import type { InMemoryBackend, NewClip } from "./backend.js";
import { GameAudioStore, type GameStateValue } from "./gameaudio.js";

/** GA-5 input shapes (field names mirror the export-package.md tool table). */
export const AuditionInput = z.object({
  state: z.string(),
  values: z.array(z.object({ param: z.string(), value: z.number() })).default([]),
});
export const TriggerSfxInput = z.object({
  eventId: z.string(),
  values: z.array(z.object({ param: z.string(), value: z.number() })).optional(),
});
export const ExportInput = z.object({
  name: z.string(),
  cueIds: z.array(z.string()),
  bankIds: z.array(z.string()),
});

export type ToolHandler = (args: any) => Promise<unknown> | unknown;

function requireString(value: unknown, what: string): string {
  if (typeof value !== "string" || !value) {
    throw new Error(`${what} must be a non-empty string`);
  }
  return value;
}

/**
 * Build the handler table for one backend (one per server instance).
 * Pass a `GameAudioStore` to share/seed cue+bank data (tests do); otherwise
 * a fresh store with the demo cue+bank is created.
 */
export function createToolHandlers(
  backend: InMemoryBackend,
  gameAudio: GameAudioStore = new GameAudioStore(),
): Record<ToolName | GameAudioToolName, ToolHandler> {
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
    gameaudio_list_cues: () => gameAudio.listCues(),
    gameaudio_audition: (args: { state: string; values?: GameStateValue[] }) =>
      gameAudio.audition({
        state: requireString(args?.state, "state"),
        values: Array.isArray(args?.values) ? args.values : [],
      }),
    gameaudio_trigger_sfx: (args: { eventId: string; values?: GameStateValue[] }) =>
      gameAudio.trigger(requireString(args?.eventId, "eventId"), args?.values),
    gameaudio_export: (args: { name: string; cueIds: string[]; bankIds: string[] }) => {
      const pkg = gameAudio.buildPackage(
        requireString(args?.name, "name"),
        requireArray(args?.cueIds, "cueIds"),
        requireArray(args?.bankIds, "bankIds"),
      );
      const project = backend.getProject();
      const report = gameAudio.validatePackage(pkg, {
        clipIds: new Set(project.clips.map((c) => c.id)),
        nodeIds: new Set([
          ...project.tracks.map((t) => t.id),
          ...(Array.isArray(project.devices)
            ? project.devices.flatMap((d) =>
                typeof d === "string" ? [d] : typeof (d as { id?: unknown }).id === "string" ? [(d as { id: string }).id] : [],
              )
            : []),
          "master",
        ]),
      });
      // A green export lands one ordinary op under `gameaudio:export`;
      // red reports return as data (no export ships with validator errors).
      if (report.ok) backend.logGameAudioExport(pkg.name, report.package.stems.length);
      return report;
    },
  };
}

function requireArray(value: unknown, what: string): string[] {
  if (!Array.isArray(value) || !value.every((v) => typeof v === "string")) {
    throw new Error(`${what} must be an array of strings`);
  }
  return value;
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
    name: "gameaudio_list_cues",
    description: "List adaptive cues (layers, states, transition counts).",
    action: "gameaudio.list_cues",
  },
  {
    name: "gameaudio_audition",
    description: "Post a game-state snapshot; returns audible layers per cue.",
    action: "gameaudio.audition_cue",
  },
  {
    name: "gameaudio_trigger_sfx",
    description: "Trigger one SFX event by id (audition only, never writes).",
    action: "gameaudio.trigger_event",
  },
  {
    name: "gameaudio_export",
    description: "Build + validate an engine export package; returns the report.",
    action: "gameaudio.export_bank",
  },
];
