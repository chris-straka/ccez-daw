/**
 * Track L (agent 2): deterministic local NL parser (the fallback, not the
 * ceiling). Rule-based, offline, fully deterministic — the layer that stays
 * true regardless of which neural model a sidecar serves.
 */
import type { ClipDraft, ClipKind, NlPlan, OpDraft } from "./types.js";

export class NlParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "NlParseError";
  }
}

export interface ParseContext {
  /** Used when the utterance names no track. Defaults to `trk_music`. */
  defaultTrackId?: string;
  /** Known track ids, used only for a "did you mean …?" warning. */
  knownTrackIds?: string[];
}

const DEFAULT_TRACK = "trk_music";

function slug(s: string): string {
  const t = s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return t.length > 0 ? t : "clip";
}

function num(raw: string | undefined, fallback: number): number {
  if (raw === undefined) return fallback;
  const v = Number(raw);
  if (!Number.isFinite(v)) throw new NlParseError(`not a number: ${raw}`);
  return v;
}

/** `add [audio|midi] clip [called|named] <name> [on|to|track <id>] [at beat N] [length N]` */
function tryParseAddClip(
  text: string,
  ctx: ParseContext,
): { plan: NlPlan } | null {
  const lower = text.toLowerCase();
  if (!lower.includes("add") || !lower.includes("clip")) return null;

  const kind: ClipKind = lower.includes("audio") ? "Audio" : "Midi";

  const nameMatch =
    text.match(/(?:called|named)\s+["']?([^"']+?)["']?\s+(?=(?:on|to|at|starting|length|len|long|beats?|bars?|$))/i) ??
    text.match(/(?:called|named)\s+["']?([^"']+?)["']?\s*$/i);
  const name = (nameMatch?.[1] ?? "Untitled").trim() || "Untitled";

  const trackMatch = text.match(
    /(?:on|to|in)\s+(?:track\s+)?["']?([A-Za-z0-9_:-]+)["']?/i,
  );
  let trackId = trackMatch?.[1] ?? ctx.defaultTrackId ?? DEFAULT_TRACK;
  // "on track" without an id falls through to the default.
  if (/^track$/i.test(trackId)) trackId = ctx.defaultTrackId ?? DEFAULT_TRACK;

  const startMatch = text.match(
    /(?:at|starting at|start(?:ing)?(?: on| at)?)\s+(?:beat\s+|bar\s+)?(\d+(?:\.\d+)?)/i,
  );
  // Bare "at N" already covered; also accept "beat N" without "at".
  const beatOnly = startMatch ? null : text.match(/\bbeat\s+(\d+(?:\.\d+)?)/i);
  const barOnly = text.match(/\bbar\s+(\d+(?:\.\d+)?)/i);
  let startBeats = num(startMatch?.[1] ?? beatOnly?.[1], 0);
  if (!startMatch && !beatOnly && barOnly) startBeats = num(barOnly[1], 0) * 4;

  const lenMatch = text.match(
    /(?:length|len|long)\s+(?:of\s+)?(\d+(?:\.\d+)?)\s*(bars?|beats?)?/i,
  );
  let lengthBeats = num(lenMatch?.[1], 4);
  if (lenMatch?.[2]?.toLowerCase().startsWith("bar")) lengthBeats *= 4;
  if (!(lengthBeats > 0))
    throw new NlParseError("clip length must be positive");

  const clip: ClipDraft = {
    id: `clip_${slug(name)}_${startBeats}`,
    track_id: trackId,
    name,
    start_beats: startBeats,
    length_beats: lengthBeats,
    kind,
    source: kind === "Audio" ? "take:nl" : "take:nl",
  };
  const op: OpDraft = {
    kind: "ClipAdded",
    target: clip.id,
    valueJson: JSON.stringify(clip),
  };
  const warnings: string[] = [];
  if (ctx.knownTrackIds && !ctx.knownTrackIds.includes(trackId)) {
    warnings.push(
      `unknown track "${trackId}"; the op log will reject it (UnknownTarget)`,
    );
  }
  return {
    plan: {
      summary: `Add ${kind} clip '${name}' on ${trackId} at beat ${startBeats} (length ${lengthBeats})`,
      ops: [op],
      warnings,
    },
  };
}

function tryParseTempo(text: string): { plan: NlPlan } | null {
  const m = text.match(
    /\b(?:set\s+)?tempo\s+(?:to\s+)?(\d+(?:\.\d+)?)\s*(?:bpm)?/i,
  );
  if (!m) return null;
  const tempo = num(m[1], NaN);
  if (!(tempo > 0)) throw new NlParseError("tempo must be positive");
  return {
    plan: {
      summary: `Set tempo to ${tempo} BPM`,
      ops: [{ kind: "TempoSet", target: "", valueJson: String(tempo) }],
      warnings: [],
    },
  };
}

function tryParseParamSet(text: string): { plan: NlPlan } | null {
  const m = text.match(
    /\bset\s+([A-Za-z0-9_:-]+):([A-Za-z0-9_-]+)\s+to\s+(\d+(?:\.\d+)?)/i,
  );
  if (!m) return null;
  const [, node, param, raw] = m;
  return {
    plan: {
      summary: `Set ${node}:${param} to ${raw}`,
      ops: [
        { kind: "ParamSet", target: `${node}:${param}`, valueJson: raw },
      ],
      warnings: [],
    },
  };
}

function tryParseMoveClip(text: string): { plan: NlPlan } | null {
  const m = text.match(
    /\bmove\s+clip\s+([A-Za-z0-9_:-]+)\s+to\s+(?:beat\s+)?(\d+(?:\.\d+)?)/i,
  );
  if (!m) return null;
  const [, id, raw] = m;
  return {
    plan: {
      summary: `Move clip ${id} to beat ${raw}`,
      ops: [
        {
          kind: "ClipMoved",
          target: id,
          valueJson: JSON.stringify({ startBeats: Number(raw) }),
        },
      ],
      warnings: [],
    },
  };
}

/**
 * Compile one NL utterance to ordinary undoable ops. Throws
 * `NlParseError` when no rule matches — callers surface that as a
 * "say it differently" reply, never as an op.
 */
export function parseNlCommand(text: string, ctx: ParseContext = {}): NlPlan {
  const input = text.trim();
  if (input.length === 0) throw new NlParseError("empty command");
  return (
    tryParseTempo(input)?.plan ??
    tryParseParamSet(input)?.plan ??
    tryParseMoveClip(input)?.plan ??
    tryParseAddClip(input, ctx)?.plan ??
    (() => {
      throw new NlParseError(
        `I don't understand "${input}". Try e.g. "add a midi clip called Solo on track trk_music at beat 8 length 4".`,
      );
    })()
  );
}
