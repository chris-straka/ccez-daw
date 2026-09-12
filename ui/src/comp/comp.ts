import type { Clip, ClipKind } from "../generated/project";

/**
 * Track E (agent 2): comping + retrospective capture.
 *
 * Exact TypeScript mirror of `core/src/comp.rs`. The two implementations
 * must agree on validation: contiguous-section checks (with the same
 * 1e-9 beat epsilon), take coverage, single-kind compositing, and the
 * `comp:<take>+...` / `retro:<n>-events` opaque `source` conventions.
 * `ui/tests/comp.test.ts` pins the same comp-take behavior as the Rust
 * `comp_take_*` tests, so drift on either side shows up as red.
 *
 * Like the Rust side this module adds no IPC or project-schema surface:
 * composites are ordinary clips applied through the frozen `ClipAdded` op.
 */

const EPS = 1e-9;

export interface CompSection {
  take_id: string;
  start_beats: number;
  end_beats: number;
}

export type CompFailure =
  | "empty-sections"
  | "unknown-take"
  | "wrong-track"
  | "bad-range"
  | "gap-or-overlap"
  | "uncovered"
  | "mixed-kinds";

export class CompError extends Error {
  readonly failure: CompFailure;
  constructor(failure: CompFailure, message: string) {
    super(message);
    this.failure = failure;
  }
}

/** Takes on `trackId` overlapping `[startBeats, endBeats)`, sorted by start. */
export function takesForRegion(
  clips: Clip[],
  trackId: string,
  startBeats: number,
  endBeats: number,
): Clip[] {
  return clips
    .filter(
      (c) =>
        c.track_id === trackId &&
        c.start_beats < endBeats &&
        c.start_beats + c.length_beats > startBeats,
    )
    .sort(
      (a, b) => a.start_beats - b.start_beats || (a.id < b.id ? -1 : 1),
    );
}

/** Stitch contiguous `sections` into one composite clip. Throws CompError. */
export function buildComp(
  id: string,
  trackId: string,
  name: string,
  sections: CompSection[],
  takes: Clip[],
): Clip {
  if (sections.length === 0) {
    throw new CompError("empty-sections", "comp needs at least one section");
  }
  for (const s of sections) {
    if (!(s.start_beats < s.end_beats)) {
      throw new CompError(
        "bad-range",
        `bad comp range: [${s.start_beats}, ${s.end_beats})`,
      );
    }
  }
  for (let i = 1; i < sections.length; i++) {
    if (Math.abs(sections[i].start_beats - sections[i - 1].end_beats) > EPS) {
      throw new CompError(
        "gap-or-overlap",
        `comp sections gap or overlap at beat ${sections[i - 1].end_beats}`,
      );
    }
  }
  let kind: ClipKind | null = null;
  for (const s of sections) {
    const take = takes.find((c) => c.id === s.take_id);
    if (!take) {
      throw new CompError("unknown-take", `unknown comp take \`${s.take_id}\``);
    }
    if (take.track_id !== trackId) {
      throw new CompError(
        "wrong-track",
        `take \`${take.id}\` is not on track \`${trackId}\``,
      );
    }
    const takeEnd = take.start_beats + take.length_beats;
    if (take.start_beats - s.start_beats > EPS || s.end_beats - takeEnd > EPS) {
      throw new CompError(
        "uncovered",
        `take does not cover comp section [${s.start_beats}, ${s.end_beats})`,
      );
    }
    if (kind === null) kind = take.kind;
    else if (kind !== take.kind) {
      throw new CompError(
        "mixed-kinds",
        "comp takes must all be the same clip kind",
      );
    }
  }
  const startBeats = sections[0].start_beats;
  const endBeats = sections[sections.length - 1].end_beats;
  return {
    id,
    track_id: trackId,
    name,
    start_beats: startBeats,
    length_beats: endBeats - startBeats,
    kind: kind as ClipKind,
    source: `comp:${sections.map((s) => s.take_id).join("+")}`,
  };
}

export interface RetroEvent {
  beat: number;
  data: string;
}

/**
 * Always-on capture ring. `push` every played event whether or not the
 * transport is recording; `toClip` rescues the window afterwards.
 * Beats are stamped by the caller — this buffer owns storage, not time.
 */
export class RetroBuffer {
  private readonly capacity: number;
  private events: RetroEvent[] = [];

  constructor(capacity: number) {
    this.capacity = capacity;
  }

  get length(): number {
    return this.events.length;
  }

  push(beat: number, data: string): void {
    if (this.capacity === 0) return;
    if (this.events.length >= this.capacity) this.events.shift();
    this.events.push({ beat, data });
  }

  /** Events with `beat >= sinceBeats`, in push order. */
  retrieve(sinceBeats: number): RetroEvent[] {
    return this.events.filter((e) => e.beat >= sinceBeats);
  }

  /**
   * Materialize the window as a clip spanning first to last captured
   * event, or `null` when the window is empty — never an empty clip.
   */
  toClip(
    id: string,
    trackId: string,
    name: string,
    kind: ClipKind,
    sinceBeats: number,
  ): Clip | null {
    const window = this.retrieve(sinceBeats);
    if (window.length === 0) return null;
    const first = window[0].beat;
    const last = window[window.length - 1].beat;
    return {
      id,
      track_id: trackId,
      name,
      start_beats: first,
      length_beats: Math.max(last - first, EPS),
      kind,
      source: `retro:${window.length}-events`,
    };
  }
}
