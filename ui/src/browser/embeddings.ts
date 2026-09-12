import { embedText } from "./library";

/**
 * Track J: embedding sidecar interface.
 *
 * The ranking model is deliberately unpinned. Today everything runs through
 * {@link LocalEmbeddingProvider} — the deterministic hashed embedding from
 * `./library.ts` (mirrored in `core/src/library.rs`), which works offline
 * and keeps tests hermetic. When a real model sidecar lands (local HTTP
 * server serving e.g. a sentence-transformer), implementors only need to
 * satisfy {@link EmbeddingProvider} and pass it to the palette — no ranking
 * or UI code changes.
 */

export interface EmbeddingProvider {
  /** Human-readable backend name shown in the palette footer. */
  readonly name: string;
  /** One vector per input text, in input order. */
  embed(texts: string[]): Promise<number[][]>;
}

/** Offline deterministic fallback. Synchronous math wrapped async. */
export class LocalEmbeddingProvider implements EmbeddingProvider {
  readonly name = "local-hash-v1";

  async embed(texts: string[]): Promise<number[][]> {
    return texts.map((t) => embedText(t));
  }
}

/**
 * Future neural sidecar over HTTP. Posts `{ model, texts }` and expects
 * `{ vectors: number[][] }`. The model id is a plain string on purpose —
 * nothing in the UI pins a specific model or version.
 *
 * Any failure (sidecar down, bad shape, wrong width) throws; callers that
 * want resilience should catch and fall back to {@link LocalEmbeddingProvider}.
 */
export class SidecarEmbeddingProvider implements EmbeddingProvider {
  readonly name: string;
  private readonly url: string;
  private readonly model: string;

  constructor(options: { url: string; model: string }) {
    this.url = options.url;
    this.model = options.model;
    this.name = `sidecar:${options.model}`;
  }

  async embed(texts: string[]): Promise<number[][]> {
    const res = await fetch(this.url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ model: this.model, texts }),
    });
    if (!res.ok) throw new Error(`embedding sidecar HTTP ${res.status}`);
    const body = (await res.json()) as { vectors?: unknown };
    if (!Array.isArray(body.vectors)) {
      throw new Error("embedding sidecar returned no vectors[]");
    }
    return (body.vectors as unknown[]).map((v, i) => {
      if (!Array.isArray(v) || !v.every((x) => typeof x === "number")) {
        throw new Error(`embedding sidecar vector ${i} is not number[]`);
      }
      return v as number[];
    });
  }
}

/** The default provider: offline, deterministic, no server needed. */
export function getDefaultProvider(): EmbeddingProvider {
  return new LocalEmbeddingProvider();
}
