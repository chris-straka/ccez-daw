/**
 * Arrangement / piano-roll cursor motions (Track K).
 *
 * Both views share one 2D cursor: `track` selects the row (arrangement track
 * or piano-roll pitch lane) and `step` selects the grid column. All functions
 * are pure and clamp into the grid, so headless tests and the live UI share
 * the exact same motion semantics.
 */

export interface Cursor {
  track: number;
  step: number;
}

export interface GridSize {
  tracks: number;
  steps: number;
}

export type Direction = "left" | "down" | "up" | "right";

export function clampCursor(c: Cursor, grid: GridSize): Cursor {
  return {
    track: Math.min(Math.max(c.track, 0), Math.max(grid.tracks - 1, 0)),
    step: Math.min(Math.max(c.step, 0), Math.max(grid.steps - 1, 0)),
  };
}

export function moveCursor(c: Cursor, grid: GridSize, dir: Direction, count = 1): Cursor {
  switch (dir) {
    case "left":
      return clampCursor({ track: c.track, step: c.step - count }, grid);
    case "right":
      return clampCursor({ track: c.track, step: c.step + count }, grid);
    case "down":
      return clampCursor({ track: c.track + count, step: c.step }, grid);
    case "up":
      return clampCursor({ track: c.track - count, step: c.step }, grid);
  }
}

/** `w`: jump to the next clip/note start after `pos` (stays put if none). */
export function nextClipStart(starts: readonly number[], pos: number): number {
  const sorted = [...starts].sort((a, b) => a - b);
  for (const s of sorted) if (s > pos) return s;
  return pos;
}

/** `b`: jump to the previous clip/note start before `pos` (stays put if none). */
export function prevClipStart(starts: readonly number[], pos: number): number {
  const sorted = [...starts].sort((a, b) => a - b);
  let best = pos;
  for (const s of sorted) {
    if (s < pos) best = s;
    else break;
  }
  return best;
}

export function lineStart(c: Cursor): Cursor {
  return { track: c.track, step: 0 };
}

export function lineEnd(c: Cursor, grid: GridSize): Cursor {
  return { track: c.track, step: Math.max(grid.steps - 1, 0) };
}

export function firstPosition(): Cursor {
  return { track: 0, step: 0 };
}

export function lastPosition(grid: GridSize): Cursor {
  return { track: Math.max(grid.tracks - 1, 0), step: Math.max(grid.steps - 1, 0) };
}
