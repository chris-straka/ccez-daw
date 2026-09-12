import type { LibraryItem, LibraryKind } from "../generated/project";

/**
 * Track J: deterministic similarity search over the browser library.
 *
 * This is the exact TypeScript mirror of `core/src/library.rs`. The two
 * implementations must agree bit-for-bit on scores: `tokenize`, `fnv1a32`,
 * and `embedText` are line-by-line ports (FNV-1a via `Math.imul`, ASCII
 * alphanumeric tokenization). The Rust tests and `ui/tests/browser.test.ts`
 * pin the same concept query to the same seeded winner, so any drift
 * between the two shows up as a red test on either side.
 *
 * Real neural embeddings arrive later behind the sidecar interface in
 * `./embeddings.ts`; this module stays as the offline fallback and oracle.
 */

export const EMBED_DIM = 64;

export type { LibraryItem, LibraryKind };
export type KindFilter = LibraryKind | "All";

export interface RankedItem {
  item: LibraryItem;
  score: number;
}

/** Lowercase alphanumeric tokenization. Mirrors Rust `tokenize`. */
export function tokenize(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((t) => t.length > 0);
}

/** FNV-1a 32-bit hash. Mirrors Rust `fnv1a32` (wrapping u32 arithmetic). */
export function fnv1a32(bytes: Uint8Array): number {
  let hash = 2_166_136_261;
  for (const b of bytes) {
    hash ^= b;
    hash = Math.imul(hash, 16_777_619);
  }
  return hash >>> 0;
}

const encoder = new TextEncoder();

/** Deterministic hashed bag-of-words embedding, L2-normalized. */
export function embedText(text: string): number[] {
  const v = new Array<number>(EMBED_DIM).fill(0);
  for (const token of tokenize(text)) {
    const idx = fnv1a32(encoder.encode(token)) % EMBED_DIM;
    v[idx] += 1;
  }
  const norm = Math.sqrt(v.reduce((s, x) => s + x * x, 0));
  if (norm > 0) return v.map((x) => x / norm);
  return v;
}

/** Searchable document text for one item. Mirrors Rust `item_text`. */
export function itemText(item: LibraryItem): string {
  return [item.name, ...item.tags, item.text].join(" ");
}

/** Cosine similarity (dot product of normalized vectors). */
export function cosine(a: number[], b: number[]): number {
  if (a.length !== b.length) return 0;
  let dot = 0;
  for (let i = 0; i < a.length; i++) dot += a[i] * b[i];
  return Number.isNaN(dot) ? 0 : dot;
}

export interface SearchOptions {
  topK?: number;
  kind?: KindFilter;
}

/**
 * Rank `items` by concept similarity to `query`. Ties break by ascending
 * id for determinism; empty queries score 0 everywhere and return id order.
 */
export function searchLibrary(
  query: string,
  items: LibraryItem[],
  options: SearchOptions = {},
): RankedItem[] {
  const { topK = 8, kind = "All" } = options;
  const q = embedText(query);
  const ranked: RankedItem[] = items
    .filter((it) => kind === "All" || it.kind === kind)
    .map((item) => ({ item, score: cosine(q, embedText(itemText(item))) }));
  ranked.sort((a, b) => b.score - a.score || (a.item.id < b.item.id ? -1 : 1));
  return ranked.slice(0, topK);
}
