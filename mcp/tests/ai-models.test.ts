import { describe, expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  ensureModelCached,
  HttpTranscriptionSidecar,
  LocalTranscriptionProvider,
  MODEL_PINS,
  pinFor,
  resolveProviderFor,
} from "../src/ai";

function withEmptyModelDir(fn: () => void): void {
  const dir = mkdtempSync(join(tmpdir(), "ccez-models-absent-"));
  const prev = process.env.CCEZ_MODEL_DIR;
  process.env.CCEZ_MODEL_DIR = dir;
  try {
    fn();
  } finally {
    if (prev === undefined) delete process.env.CCEZ_MODEL_DIR;
    else process.env.CCEZ_MODEL_DIR = prev;
  }
}

describe("Track P-2: model pins resolve behind the one-interface seam", () => {
  test("every sidecar has a verdict", () => {
    const sidecars = [
      "transcribe-drums",
      "transcribe-melody",
      "transcribe-chords",
      "separation",
      "cleanup",
      "groove-transfer",
    ];
    expect(MODEL_PINS).toHaveLength(sidecars.length);
    for (const id of sidecars) expect(pinFor(id)).toBeDefined();
    expect(pinFor("groove-transfer")?.verdict.kind).toBe("baselines-only");
  });

  test("models-absent run uses the deterministic local provider", () => {
    withEmptyModelDir(() => {
      for (const kind of ["drums", "melody", "chords"] as const) {
        expect(resolveProviderFor(kind)).toBeInstanceOf(LocalTranscriptionProvider);
      }
    });
  });

  test("cached weights resolve to the HTTP sidecar with the pinned id", () => {
    const dir = mkdtempSync(join(tmpdir(), "ccez-models-present-"));
    const prev = process.env.CCEZ_MODEL_DIR;
    process.env.CCEZ_MODEL_DIR = dir;
    try {
      writeFileSync(join(dir, "basic-pitch.onnx"), "fake weights");
      const provider = resolveProviderFor("melody");
      expect(provider).toBeInstanceOf(HttpTranscriptionSidecar);
      expect((provider as HttpTranscriptionSidecar).model).toBe("basic-pitch");
      // Drums weights still absent: baseline.
      expect(resolveProviderFor("drums")).toBeInstanceOf(LocalTranscriptionProvider);
    } finally {
      if (prev === undefined) delete process.env.CCEZ_MODEL_DIR;
      else process.env.CCEZ_MODEL_DIR = prev;
    }
  });

  test("ensureModelCached downloads on first use, then no-ops; baselines-only throws", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ccez-models-dl-"));
    const prev = process.env.CCEZ_MODEL_DIR;
    process.env.CCEZ_MODEL_DIR = dir;
    try {
      let calls = 0;
      const fakeFetch = (async () => {
        calls += 1;
        return new Response("weights-bytes");
      }) as unknown as typeof fetch;
      const dest = await ensureModelCached(
        "transcribe-melody",
        "https://example.invalid/basic-pitch.onnx",
        fakeFetch,
      );
      expect(dest).toBe(join(dir, "basic-pitch.onnx"));
      expect(calls).toBe(1);
      await ensureModelCached("transcribe-melody", "https://example.invalid/x", fakeFetch);
      expect(calls).toBe(1);
      await expect(ensureModelCached("groove-transfer", "https://example.invalid/x")).rejects.toThrow(
        "baselines-only",
      );
    } finally {
      if (prev === undefined) delete process.env.CCEZ_MODEL_DIR;
      else process.env.CCEZ_MODEL_DIR = prev;
    }
  });
});
