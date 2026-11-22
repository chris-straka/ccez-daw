import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
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

  test("ensureModelCached verifies SHA-256 checksum, caches on match, and rejects unverified or tampered weights", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ccez-models-dl-"));
    const prev = process.env.CCEZ_MODEL_DIR;
    process.env.CCEZ_MODEL_DIR = dir;
    try {
      const payload = "weights-bytes";
      const rightSha = createHash("sha256").update(payload).digest("hex");
      const wrongSha = "0".repeat(64);
      let calls = 0;
      const fakeFetch = (async () => {
        calls += 1;
        return new Response(payload);
      }) as unknown as typeof fetch;

      // Missing/invalid SHA-256 must fail before downloading.
      await expect(
        ensureModelCached("transcribe-melody", "https://example.invalid/basic-pitch.onnx", "", fakeFetch),
      ).rejects.toThrow();
      expect(calls).toBe(0);

      // Mismatching SHA-256 must fail without leaving a file in cache.
      await expect(
        ensureModelCached(
          "transcribe-melody",
          "https://example.invalid/basic-pitch.onnx",
          wrongSha,
          fakeFetch,
        ),
      ).rejects.toThrow(/checksum|sha-?256/i);
      expect(existsSync(join(dir, "basic-pitch.onnx"))).toBe(false);

      const dest = await ensureModelCached(
        "transcribe-melody",
        "https://example.invalid/basic-pitch.onnx",
        rightSha,
        fakeFetch,
      );
      expect(dest).toBe(join(dir, "basic-pitch.onnx"));
      const callsAfterFirst = calls;
      await ensureModelCached(
        "transcribe-melody",
        "https://example.invalid/x",
        rightSha,
        fakeFetch,
      );
      expect(calls).toBe(callsAfterFirst);

      // Tampered cached file must also fail checksum check.
      writeFileSync(dest, "tampered");
      await expect(
        ensureModelCached(
          "transcribe-melody",
          "https://example.invalid/x",
          rightSha,
          fakeFetch,
        ),
      ).rejects.toThrow(/checksum|sha-?256/i);

      await expect(
        ensureModelCached("groove-transfer", "https://example.invalid/x", rightSha),
      ).rejects.toThrow("baselines-only");
    } finally {
      if (prev === undefined) delete process.env.CCEZ_MODEL_DIR;
      else process.env.CCEZ_MODEL_DIR = prev;
    }
  });
});
