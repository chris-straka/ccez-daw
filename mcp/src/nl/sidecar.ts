/**
 * Track L (agent 2): sidecar pattern for NL (models unpinned).
 *
 * `NlProvider.compile` is the only seam a neural model touches. The local
 * provider (today's default) runs the deterministic parser; a future HTTP
 * sidecar may serve any model — the model id is a plain string, nothing
 * pins a version. Swapping models changes plans, never op shapes or UI.
 */
import { parseNlCommand, type ParseContext } from "./parse.js";
import type { NlPlan } from "./types.js";

export interface NlProvider {
  /** Sidecar name; becomes the op actor `ai:<id>`. */
  readonly id: string;
  compile(text: string, ctx?: ParseContext): Promise<NlPlan>;
}

export class LocalNlProvider implements NlProvider {
  readonly id = "local-nl";

  async compile(text: string, ctx: ParseContext = {}): Promise<NlPlan> {
    return parseNlCommand(text, ctx);
  }
}

export interface SidecarOptions {
  endpoint: string;
  /** Unpinned model id, e.g. "qwen2.5:7b" — a plain string, never enforced. */
  model: string;
  sidecarId?: string;
  timeoutMs?: number;
}

export class SidecarNlProvider implements NlProvider {
  readonly id: string;
  readonly model: string;
  readonly endpoint: string;
  readonly timeoutMs: number;

  constructor(opts: SidecarOptions) {
    this.endpoint = opts.endpoint;
    this.model = opts.model;
    this.id = opts.sidecarId ?? "sidecar";
    this.timeoutMs = opts.timeoutMs ?? 10_000;
  }

  async compile(text: string, ctx: ParseContext = {}): Promise<NlPlan> {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), this.timeoutMs);
    try {
      const res = await fetch(`${this.endpoint}/compile`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ model: this.model, text, ctx }),
        signal: ctrl.signal,
      });
      if (!res.ok) throw new Error(`sidecar ${res.status}`);
      const plan = (await res.json()) as NlPlan;
      if (!Array.isArray(plan.ops))
        throw new Error("sidecar returned no ops[]");
      return plan;
    } finally {
      clearTimeout(timer);
    }
  }
}

/** Compile via the given provider (default: deterministic local). */
export async function compileNlCommand(
  text: string,
  provider: NlProvider = new LocalNlProvider(),
  ctx: ParseContext = {},
): Promise<NlPlan> {
  return provider.compile(text, ctx);
}
