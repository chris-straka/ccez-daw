/**
 * Track P-2: AI sidecar model pins — TS mirror of `core/src/ai/models.rs`.
 *
 * Same verdict table (six sidecars, six verdicts), same runtime rule: the
 * DAW runs fully with models absent. `resolveProviderFor` returns the
 * deterministic local provider unless the weight file is already cached
 * under `$CCEZ_MODEL_DIR`; only then does it return the existing
 * `HttpTranscriptionSidecar` seam with the pinned model id. The first-use
 * download itself is `ensureModelCached`, which fetches the canonical
 * weights into the same cache path the Rust side probes. Plans may change;
 * op shapes and UI don't.
 */
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import {
  HttpTranscriptionSidecar,
  LocalTranscriptionProvider,
  type TranscriptionProvider,
} from "./sidecar.js";
import { SIDECAR_IDS, type TranscriptionKind } from "./types.js";

export type ModelVerdict =
  | {
      kind: "pinned";
      /** Model id sent over the HTTP seam (plain string, never enforced). */
      modelId: string;
      /** Approximate weight size in MB, for the download prompt. */
      sizeMb: string;
      /** Redistribution license (verified before bundling anything). */
      license: string;
      /** Canonical download source for the weights. */
      url: string;
      /** Weight filename inside the model cache dir. */
      file: string;
    }
  | { kind: "baselines-only"; reason: string };

export interface ModelPin {
  sidecar: string;
  verdict: ModelVerdict;
}

/** The v3 verdict table. Mirrors `PINS` in `core/src/ai/models.rs`. */
export const MODEL_PINS: readonly ModelPin[] = [
  {
    sidecar: SIDECAR_IDS.drums,
    verdict: {
      kind: "pinned",
      modelId: "oaf-drums",
      sizeMb: "~40",
      license: "Apache-2.0 (Magenta; re-verify checkpoint header at wire-up)",
      url: "https://github.com/magenta/onsets-and-frames",
      file: "oaf-drums.ckpt",
    },
  },
  {
    sidecar: SIDECAR_IDS.melody,
    verdict: {
      kind: "pinned",
      modelId: "basic-pitch",
      sizeMb: "~15",
      license: "Apache-2.0 (Spotify Audio Intelligence Lab)",
      url: "https://github.com/spotify/basic-pitch",
      file: "basic-pitch.onnx",
    },
  },
  {
    sidecar: SIDECAR_IDS.chords,
    verdict: {
      kind: "pinned",
      modelId: "basic-pitch",
      sizeMb: "~15",
      license: "Apache-2.0 (Spotify Audio Intelligence Lab)",
      url: "https://github.com/spotify/basic-pitch",
      file: "basic-pitch.onnx",
    },
  },
  {
    sidecar: "separation",
    verdict: {
      kind: "pinned",
      modelId: "htdemucs",
      sizeMb: "~80-350 by variant/precision (4-stem)",
      license: "MIT (Meta AI Research, code + weights)",
      url: "https://github.com/facebookresearch/demucs",
      file: "htdemucs.th",
    },
  },
  {
    sidecar: "cleanup",
    verdict: {
      kind: "pinned",
      modelId: "deepfilternet3",
      sizeMb: "~4-30",
      license:
        "MIT/Apache-2.0 dual (Rikorose/DeepFilterNet; re-verify LICENSE at wire-up)",
      url: "https://github.com/Rikorose/DeepFilterNet",
      file: "deepfilternet3.onnx",
    },
  },
  {
    sidecar: "groove-transfer",
    verdict: {
      kind: "baselines-only",
      reason:
        "groove transfer is an analytic MIDI timing/velocity transform " +
        "(extract + apply template); no neural model beats it at this task " +
        "for its size. HTTP seam stays open for a future style model.",
    },
  },
] as const;

export function pinFor(sidecar: string): ModelPin | undefined {
  return (MODEL_PINS as readonly ModelPin[]).find((p) => p.sidecar === sidecar);
}

/** Weight cache dir: `$CCEZ_MODEL_DIR`, else `~/.local/share/ccez-daw/models`. */
export function modelCacheDir(): string {
  const env = (process.env.CCEZ_MODEL_DIR ?? "").trim();
  if (env.length > 0) return env;
  const home = homedir();
  if (home.length > 0) return join(home, ".local", "share", "ccez-daw", "models");
  return join(tmpdir(), "ccez-daw-models");
}

export function cachedWeightPath(modelId: string, file: string): string {
  void modelId;
  return join(modelCacheDir(), file);
}

export function isModelCached(pin: ModelPin): boolean {
  if (pin.verdict.kind !== "pinned") return false;
  return existsSync(cachedWeightPath(pin.verdict.modelId, pin.verdict.file));
}

/** Default local sidecar endpoint (loopback; serves cached weights). */
export const DEFAULT_SIDECAR_ENDPOINT = "http://127.0.0.1:8765";

export interface ResolveOptions {
  endpoint?: string;
}

/**
 * Resolve a transcription kind to its provider. Models-absent is the
 * default: baselines-only verdicts, missing weight files, and unknown
 * kinds all yield the deterministic local provider.
 */
export function resolveProviderFor(
  kind: TranscriptionKind,
  opts: ResolveOptions = {},
): TranscriptionProvider {
  const pin = pinFor(SIDECAR_IDS[kind]);
  if (pin?.verdict.kind === "pinned" && isModelCached(pin)) {
    return new HttpTranscriptionSidecar({
      endpoint: opts.endpoint ?? DEFAULT_SIDECAR_ENDPOINT,
      model: pin.verdict.modelId,
    });
  }
  return new LocalTranscriptionProvider();
}

/**
 * Download-on-first-use: fetch a pinned model's weights into the cache
 * dir. No-op (returns the existing path) when already cached; throws for
 * baselines-only sidecars. The caller passes the direct weight URL; the
 * pin's `url` names the canonical upstream to take it from.
 */
export async function ensureModelCached(
  sidecar: string,
  weightUrl: string,
  fetchImpl: typeof fetch = fetch,
): Promise<string> {
  const pin = pinFor(sidecar);
  if (!pin) throw new Error(`unknown AI sidecar: ${sidecar}`);
  if (pin.verdict.kind !== "pinned") {
    throw new Error(`sidecar ${sidecar} is baselines-only: nothing to download`);
  }
  const dest = cachedWeightPath(pin.verdict.modelId, pin.verdict.file);
  if (existsSync(dest)) return dest;
  const res = await fetchImpl(weightUrl);
  if (!res.ok) throw new Error(`model download failed: ${res.status}`);
  const bytes = new Uint8Array(await res.arrayBuffer());
  await mkdir(modelCacheDir(), { recursive: true });
  await writeFile(dest, bytes);
  return dest;
}
