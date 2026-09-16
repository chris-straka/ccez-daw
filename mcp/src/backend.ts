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
    | "AutomationPointSet"
    | "GameAudioExported";
  target: string;
  value_json: string;
}

/** One automation lane mirrored from the frozen project shape. */
export interface AutomationLaneView {
  id: string;
  target: { node: string; param: string };
  points: Array<{ beat: number; value: number }>;
}

/** One op offered for a branch merge (fork-point diff input). */
export interface MergeOpInput {
  kind: OpView["kind"];
  target: string;
  value_json: string;
}

/** Link session clock state (evaluation-only; never touches the op log). */
export interface LinkSessionView {
  session: string;
  tempo: number;
  peers: string[];
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
  private automationLanes: AutomationLaneView[] = [];
  private linkSessions = new Map<string, { tempo: number; peers: string[] }>();

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

  /**
   * Post-v0 coverage mirrors. Every mutation appends an ordinary op with
   * actor `"mcp"` (undoable via `undo()`); the two evaluation-only mirrors
   * (`planLaunch`, `joinLink`) write nothing, like the game-audio
   * audition/trigger tools.
   */

  /** Mirror of `automation.point_set`: upsert one point, creating the lane. */
  setAutomationPoint(
    lane: string,
    beat: number,
    value: number,
    node?: string,
    param?: string,
  ): { seq: number; laneId: string } {
    if (!lane) throw new Error("lane must be a non-empty string");
    if (typeof beat !== "number" || !Number.isFinite(beat) || beat < 0) {
      throw new Error("beat must be a finite number >= 0");
    }
    if (typeof value !== "number" || !Number.isFinite(value)) {
      throw new Error("value must be a finite number");
    }
    let existing = this.automationLanes.find((l) => l.id === lane);
    if (!existing) {
      if (!node || !param) {
        throw new Error(`unknown lane: ${lane} (pass node and param to create it)`);
      }
      existing = { id: lane, target: { node, param }, points: [] };
      this.automationLanes.push(existing);
    }
    const payload: Record<string, number | string> = { beat, value };
    if (existing.target.node && existing.target.param) {
      payload.node = existing.target.node;
      payload.param = existing.target.param;
    }
    existing.points.push({ beat, value });
    existing.points.sort((a, b) => a.beat - b.beat);
    const op = this.appendOp("AutomationPointSet", lane, payload);
    return { seq: op.seq, laneId: lane };
  }

  listAutomationLanes(): AutomationLaneView[] {
    return structuredClone(this.automationLanes);
  }

  /**
   * Mirror of `session.launch`: quantize a press beat to the next grid
   * boundary at or after it (the `LaunchQuant` grid rule). Pure plan, no op.
   */
  planLaunch(pressBeat: number, gridBeats: number): { startBeat: number } {
    if (typeof pressBeat !== "number" || !Number.isFinite(pressBeat) || pressBeat < 0) {
      throw new Error("pressBeat must be a finite number >= 0");
    }
    if (typeof gridBeats !== "number" || !Number.isFinite(gridBeats) || gridBeats <= 0) {
      throw new Error("gridBeats must be a finite number > 0");
    }
    return { startBeat: Math.ceil(pressBeat / gridBeats) * gridBeats };
  }

  /** Mirror of `session.jam_record`: each jam clip lands as `ClipAdded`. */
  jamRecord(clips: NewClip[]): { clipIds: string[]; seqs: number[] } {
    if (!Array.isArray(clips) || clips.length === 0) {
      throw new Error("clips must be a non-empty array");
    }
    const clipIds: string[] = [];
    const seqs: number[] = [];
    for (const c of clips) {
      const { clipId, seq } = this.addClip(c);
      clipIds.push(clipId);
      seqs.push(seq);
    }
    return { clipIds, seqs };
  }

  /**
   * Mirror of `comp.commit`: stitch named takes into one composite clip
   * (`source = "comp:<take-1>+<take-2>..."`, the core `build_comp`
   * convention) and commit it as one `ClipAdded` op.
   */
  commitComp(input: NewClip & { takes: string[] }): { clipId: string; seq: number } {
    if (!Array.isArray(input?.takes) || input.takes.length === 0) {
      throw new Error("takes must be a non-empty array of take ids");
    }
    for (const t of input.takes) {
      if (typeof t !== "string" || !t) throw new Error("take ids must be non-empty strings");
    }
    const track = this.project.tracks.find((t) => t.id === input.trackId);
    if (!track) throw new Error(`unknown track: ${input.trackId}`);
    const { clipId, seq } = this.addClip(input);
    const clip = this.project.clips.find((c) => c.id === clipId);
    if (clip) clip.source = `comp:${input.takes.join("+")}`;
    const op = this.ops.find((o) => o.seq === seq);
    if (op && clip) op.value_json = JSON.stringify(clip);
    return { clipId, seq };
  }

  /**
   * Mirror of `groove.apply`: stamp a grooved copy of a clip
   * (`"<id>~groove-<name>"`, `source = "groove:<name>+<src>"`, the UI
   * `grooveCommitOp` convention) as one `ClipAdded` op. `amount` blends
   * 0 (unchanged) .. 1 (full template feel).
   */
  applyGroove(clipId: string, groove: string, amount: number): { clipId: string; seq: number } {
    const target = this.project.clips.find((c) => c.id === clipId);
    if (!target) throw new Error(`unknown clip: ${clipId}`);
    const clean = groove
      .trim()
      .toLowerCase()
      .replace(/\s+/g, "-")
      .replace(/[^a-z0-9_-]/g, "")
      .slice(0, 64);
    if (!clean) throw new Error("groove must be a non-empty name");
    if (typeof amount !== "number" || !Number.isFinite(amount) || amount < 0 || amount > 1) {
      throw new Error("amount must be a finite number in 0..=1");
    }
    const track = this.project.tracks.find((t) => t.id === target.track_id);
    if (!track) throw new Error(`unknown track: ${target.track_id}`);
    const clip: ClipView = {
      ...structuredClone(target),
      id: `${target.id}~groove-${clean}`,
      name: `${target.name} (groove ${groove.trim()})`,
      source: `groove:${clean}+${target.source}`,
    };
    this.project.clips.push(clip);
    track.clip_ids.push(clip.id);
    const op = this.appendOp("ClipAdded", clip.id, clip);
    return { clipId: clip.id, seq: op.seq };
  }

  /**
   * Mirror of `branch.merge`: three-way op-granularity merge of `sourceOps`
   * against this backend's own log (the target side). Same-`target` edits
   * with a different effect surface as conflicts (never applied); clean ops
   * append with actor `"mcp"`, like `merge_apply`.
   */
  mergeBranch(
    source: string,
    target: string,
    sourceOps: MergeOpInput[],
  ): { merged: number[]; conflicts: string[] } {
    if (!source) throw new Error("source must be a non-empty branch name");
    if (!target) throw new Error("target must be a non-empty branch name");
    if (!Array.isArray(sourceOps)) throw new Error("sourceOps must be an array");
    const kinds: ReadonlyArray<OpView["kind"]> = [
      "TrackAdded",
      "ClipAdded",
      "ClipMoved",
      "ParamSet",
      "TempoSet",
      "UndoMarker",
      "AutomationPointSet",
    ];
    const merged: number[] = [];
    const conflicts: string[] = [];
    for (const op of sourceOps) {
      if (!kinds.includes(op?.kind)) throw new Error(`bad op kind: ${String(op?.kind)}`);
      if (typeof op?.target !== "string" || !op.target) {
        throw new Error("merge ops need non-empty targets");
      }
      if (typeof op?.value_json !== "string") throw new Error("merge ops need a value_json string");
      const other = this.ops.find((o) => o.target === op.target && o.kind !== "UndoMarker");
      if (other && (other.kind !== op.kind || other.value_json !== op.value_json)) {
        if (!conflicts.includes(op.target)) conflicts.push(op.target);
        continue;
      }
      if (other) continue;
      merged.push(this.appendOp(op.kind, op.target, JSON.parse(op.value_json)).seq);
    }
    return { merged, conflicts };
  }

  /**
   * Mirror of `record.punch`: commit one punched take
   * (`source = "take:<id>"`, the `punch_take_clip` convention) as one
   * `ClipAdded` op.
   */
  commitPunch(input: {
    trackId: string;
    name: string;
    kind: ClipKind;
    startBeats: number;
    endBeats: number;
  }): { clipId: string; seq: number } {
    const { startBeats, endBeats } = input;
    if (typeof startBeats !== "number" || !Number.isFinite(startBeats) || startBeats < 0) {
      throw new Error("startBeats must be a finite number >= 0");
    }
    if (typeof endBeats !== "number" || !Number.isFinite(endBeats) || !(endBeats > startBeats)) {
      throw new Error("endBeats must be finite and > startBeats");
    }
    const { clipId, seq } = this.addClip({
      trackId: input.trackId,
      name: input.name,
      startBeats,
      lengthBeats: endBeats - startBeats,
      kind: input.kind,
    });
    const clip = this.project.clips.find((c) => c.id === clipId);
    if (clip) clip.source = `take:${clipId}`;
    const op = this.ops.find((o) => o.seq === seq);
    if (op && clip) op.value_json = JSON.stringify(clip);
    return { clipId, seq };
  }

  /**
   * Mirror of `link.join`: join the named session clock. Pure session
   * state (tempo consensus + peer list) — writes no ops and touches no
   * frozen schema, like the core `LinkBus`.
   */
  joinLink(session: string, peer: string, tempo?: number): LinkSessionView {
    if (!session) throw new Error("session must be a non-empty name");
    if (!peer) throw new Error("peer must be a non-empty name");
    let entry = this.linkSessions.get(session);
    if (!entry) {
      entry = { tempo: 120, peers: [] };
      this.linkSessions.set(session, entry);
    }
    if (tempo !== undefined) {
      if (typeof tempo !== "number" || !Number.isFinite(tempo) || tempo < 1 || tempo > 960) {
        throw new Error("tempo must be a finite BPM in 1..=960");
      }
      entry.tempo = tempo;
    }
    if (!entry.peers.includes(peer)) entry.peers.push(peer);
    return { session, tempo: entry.tempo, peers: [...entry.peers] };
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
