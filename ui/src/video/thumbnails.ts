import { FILMSTRIP_THUMBS } from "./model";

/**
 * Filmstrip thumbnail cells: the UI-side end of the core thumbnail seam.
 *
 * Core (`core/src/video.rs`) extracts one JPEG per strip slot with `ffmpeg`
 * (decode; JPEG encode by ffmpeg or the native q72 4:2:0 encoder) and
 * serializes a `ThumbStrip` beside `video.json` (`thumbs/<track>.json`). The video
 * sidecar never crosses the Tauri IPC boundary, so the host converts each
 * real frame's disk `path` into a renderable URL (a `blob:` URL from reading
 * the JPEG bytes, or a Tauri asset URL) and hands the cells to
 * `FilmstripLane` via its `thumbs` prop. This module is the pure mapping
 * between those two ends:
 *
 * - `stripToCells` maps a serialized core strip (or any
 *   path/placeholder list) to renderable cells through a host-supplied
 *   path->URL resolver.
 * - `cellsForClip` looks a clip's cells up in the lane map and pads /
 *   truncates to exactly `FILMSTRIP_THUMBS` slots, so layout never shifts.
 * - `isMediaSrc` reports whether a clip `src` can have media at all:
 *   `take:<key>` sources (and empty strings) render placeholders only.
 */

export interface ThumbCell {
  index: number;
  atSeconds: number;
  /** Renderable URL (`blob:` / asset) for a real frame; null = placeholder. */
  url: string | null;
  placeholder: boolean;
}

/** Lane map: clip id -> per-slot cells. Owned and passed in by the host. */
export type ClipThumbMap = Record<string, ThumbCell[]>;

/** True when `src` can name real media. `take:<key>` is asset-less by contract. */
export function isMediaSrc(src: string): boolean {
  if (!src) return false;
  return !src.startsWith("take:");
}

/** One all-placeholder cell for slot `index`. */
export function placeholderCell(index: number, atSeconds = 0): ThumbCell {
  return { index, atSeconds, url: null, placeholder: true };
}

/** `count` all-placeholder cells (defaults to the lane width). */
export function placeholderCells(count: number = FILMSTRIP_THUMBS): ThumbCell[] {
  const n = Math.max(0, Math.floor(count));
  return Array.from({ length: n }, (_, index) => placeholderCell(index));
}

function asNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/**
 * Cells for one clip: the map entry when present, otherwise placeholders.
 * Always returns exactly `count` cells — shorter entries are padded with
 * placeholders, longer ones truncated — so the lane width never shifts.
 * Clips without media (`take:` src) always get placeholders, even if the
 * map carries an entry for them.
 */
export function cellsForClip(
  clip: { id: string; src: string },
  thumbs?: ClipThumbMap,
  count: number = FILMSTRIP_THUMBS,
): ThumbCell[] {
  const n = Math.max(0, Math.floor(count));
  if (!isMediaSrc(clip.src)) return placeholderCells(n);
  const entry = thumbs?.[clip.id];
  if (!entry || entry.length === 0) return placeholderCells(n);
  const cells = entry.slice(0, n).map((c, i) => ({
    index: asNumber(c?.index, i),
    atSeconds: asNumber(c?.atSeconds, 0),
    url: typeof c?.url === "string" && c.url.length > 0 ? c.url : null,
    placeholder: c?.placeholder !== false || typeof c?.url !== "string" || c.url.length === 0,
  }));
  // A cell that claims to be real but carries no URL is a placeholder.
  for (let i = cells.length; i < n; i++) cells.push(placeholderCell(i));
  return cells;
}

/**
 * Serialized core strip frame (snake_case over IPC-free JSON) or the
 * camelCase UI twin; both accepted so `thumbs/<track>.json` parses as-is.
 */
export interface StripFrameLike {
  index?: number;
  at_seconds?: number;
  atSeconds?: number;
  path?: string | null;
  url?: string | null;
  placeholder?: boolean;
}

/**
 * Map serialized strip frames to renderable cells. Real frames (non-empty
 * `path`/`url`, not flagged `placeholder`) resolve through `toUrl`
 * (host: file bytes -> `blob:` URL, or path -> asset URL); a resolver that
 * returns null/empty keeps that slot a placeholder. Never throws for
 * malformed entries — bad slots degrade to placeholders.
 */
export function stripToCells(
  frames: StripFrameLike[] | null | undefined,
  toUrl: (path: string, index: number) => string | null | undefined,
): ThumbCell[] {
  if (!Array.isArray(frames) || frames.length === 0) return [];
  return frames.map((f, i) => {
    const index = asNumber(f?.index, i);
    const atSeconds = asNumber(f?.at_seconds ?? f?.atSeconds, 0);
    const source = typeof f?.path === "string" && f.path.length > 0 ? f.path : null;
    const direct = typeof f?.url === "string" && f.url.length > 0 ? f.url : null;
    if (f?.placeholder === true || (!source && !direct)) {
      return placeholderCell(index, atSeconds);
    }
    let url: string | null = direct;
    if (!url && source) {
      try {
        const resolved = toUrl(source, index);
        url = typeof resolved === "string" && resolved.length > 0 ? resolved : null;
      } catch {
        url = null;
      }
    }
    if (!url) return placeholderCell(index, atSeconds);
    return { index, atSeconds, url, placeholder: false };
  });
}
