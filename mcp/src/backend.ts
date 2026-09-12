/**
 * Track L: MCP backend seam.
 *
 * The MCP server runs as its own Bun process, so it cannot `invoke` Tauri
 * commands directly. This module is the seam where the six frozen tools
 * (`contracts/mcp-tools.md`) meet the op log (`contracts/op-log-format.md`):
 * every mutating call appends an ordinary op with actor `"mcp"`, so MCP
 * output is undoable by design via `undo()` (mirrors `op_undo`), exactly
 * like script output (`script:<name>`) from Track K.
 *
 * Field names reuse the generated project shapes' snake_case
 * (`track_id`, `start_beats`, …) so the MCP surface cannot drift from the
 * UI surfaces. Devices, routing, and automation need no extra tools: the
 * universal node model exposes them through `getProject` (devices/routing/
 * automation lists) and `setParam` (`node:param` addresses anything).
 */

export const MCP_ACTOR = "mcp";

/**
 * GA-5: actor for game-audio export records. Audition/trigger tools are
 * evaluation-only and write nothing; a green `gameaudio_export` lands one
 * ordinary op under this actor, so exports stay undoable/addressable like
 * every other MCP mutation.
 */
export const GAMEAUDIO_EXPORT_ACTOR = "gameaudio:export";

export type ClipKind = "Audio" | "Midi";
export type EngineState = "Stopped" | "Playing";

export interface TrackView {
  id: string;
  name: string;
  volume: number;
  pan: number;
  muted: boolean;
  solo: boolean;
  clip_ids: string[];
  device_ids: string[];
}

export interface ClipView {
  id: string;
  track_id: string;
  name: string;
  start_beats: number;
  length_beats: number;
  kind: ClipKind;
  source: string;
}

export interface ProjectView {
  schema_version: 0;
  id: string;
  name: string;
  tempo: number;
  time_sig_num: number;
  time_sig_den: number;
  tracks: TrackView[];
  clips: ClipView[];
  devices: unknown[];
  routing: unknown[];
  automation: unknown[];
}

/** One event-sourced op-log entry (`contracts/op-log-format.md`). */
export interface OpView {
  seq: number;
  actor: string;
  kind:
    | "TrackAdded"
    | "ClipAdded"
    | "ClipMoved"
    | "ParamSet"
    | "TempoSet"
    | "UndoMarker"
    | "GameAudioExported";
  target: string;
  value_json: string;
}

export interface NewClip {
  trackId: string;
  name: string;
  startBeats: number;
  lengthBeats: number;
  kind: ClipKind;
}

/** In-memory project store + op log. Swap for a Tauri/core-backed
 *  implementation without touching the tool shapes. */
export class InMemoryBackend {
  private project: ProjectView;
  private engine: EngineState = "Stopped";
  private ops: OpView[] = [];
  private nextSeq = 1;

  constructor(name = "Untitled") {
    this.project = {
      schema_version: 0,
      id: "proj_1",
      name,
      tempo: 120,
      time_sig_num: 4,
      time_sig_den: 4,
      tracks: [
        {
          id: "trk_1",
          name: "Drums",
          volume: 0.8,
          pan: 0,
          muted: false,
          solo: false,
          clip_ids: [],
          device_ids: [],
        },
      ],
      clips: [],
      devices: [],
      routing: [],
      automation: [],
    };
  }

  private appendOp(
    kind: OpView["kind"],
    target: string,
    payload: unknown,
  ): OpView {
    return this.appendOpAs(MCP_ACTOR, kind, target, payload);
  }

  private appendOpAs(
    actor: string,
    kind: OpView["kind"],
    target: string,
    payload: unknown,
  ): OpView {
    const op: OpView = {
      seq: this.nextSeq++,
      actor,
      kind,
      target,
      value_json: JSON.stringify(payload),
    };
    this.ops.push(op);
    return op;
  }

  /** GA-5: record a validated export package (called only on green reports). */
  logGameAudioExport(packageName: string, stemCount: number): { seq: number } {
    const op = this.appendOpAs(GAMEAUDIO_EXPORT_ACTOR, "GameAudioExported", packageName, {
      packageName,
      stemCount,
    });
    return { seq: op.seq };
  }

  getProject(): ProjectView {
    return structuredClone(this.project);
  }

  listTracks(): TrackView[] {
    return structuredClone(this.project.tracks);
  }

  getOpLog(): OpView[] {
    return structuredClone(this.ops);
  }

  getEngineState(): EngineState {
    return this.engine;
  }

  addClip(input: NewClip): { clipId: string; seq: number } {
    const track = this.project.tracks.find((t) => t.id === input.trackId);
    if (!track) throw new Error(`unknown track: ${input.trackId}`);
    if (!input.name || typeof input.name !== "string") {
      throw new Error("clip name must be a non-empty string");
    }
    for (const [label, v] of [
      ["startBeats", input.startBeats],
      ["lengthBeats", input.lengthBeats],
    ] as const) {
      if (typeof v !== "number" || !Number.isFinite(v)) {
        throw new Error(`${label} must be a finite number`);
      }
    }
    if (input.lengthBeats <= 0) {
      throw new Error("lengthBeats must be > 0");
    }
    if (input.kind !== "Audio" && input.kind !== "Midi") {
      throw new Error(`kind must be "Audio" or "Midi"`);
    }
    const clip: ClipView = {
      id: `clip_${this.nextSeq}`,
      track_id: input.trackId,
      name: input.name,
      start_beats: input.startBeats,
      length_beats: input.lengthBeats,
      kind: input.kind,
      source: "",
    };
    this.project.clips.push(clip);
    track.clip_ids.push(clip.id);
    const op = this.appendOp("ClipAdded", clip.id, clip);
    return { clipId: clip.id, seq: op.seq };
  }

  setParam(node: string, param: string, value: number): { seq: number } {
    if (!node || !param) {
      throw new Error("param address needs node and param names");
    }
    if (typeof value !== "number" || !Number.isFinite(value)) {
      throw new Error("param value must be a finite number");
    }
    const op = this.appendOp("ParamSet", `${node}:${param}`, value);
    return { seq: op.seq };
  }

  play(): EngineState {
    this.engine = "Playing";
    return this.engine;
  }

  stop(): EngineState {
    this.engine = "Stopped";
    return this.engine;
  }

  /**
   * Undo the last undoable op (mirrors `op_undo`); records an `UndoMarker`
   * so redo branches stay addressable, and returns the undone seq.
   */
  undo(): { undoneSeq: number } {
    const last = [...this.ops]
      .reverse()
      .find((o) => o.kind !== "UndoMarker");
    if (!last) throw new Error("nothing to undo");
    if (last.kind === "ClipAdded") {
      this.project.clips = this.project.clips.filter(
        (c) => c.id !== last.target,
      );
      for (const t of this.project.tracks) {
        t.clip_ids = t.clip_ids.filter((id) => id !== last.target);
      }
    }
    this.appendOp("UndoMarker", String(last.seq), { undoneSeq: last.seq });
    return { undoneSeq: last.seq };
  }
}
