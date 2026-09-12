import { describe, expect, test } from "bun:test";
import { LibraryItemSchema } from "../src/generated/project";
import { embedText, searchLibrary } from "../src/browser/library";
import { seedLibrary } from "../src/browser/seed";

describe("Track J browser + semantic search", () => {
  test("concept query returns the seeded warm pad first", () => {
    const hits = searchLibrary(
      "cozy warm analog pad for an ambient intro",
      seedLibrary(),
      { topK: 3 },
    );
    expect(hits.length).toBeGreaterThan(0);
    expect(hits[0].item.id).toBe("preset_warm_pad");
  });

  test("kind filter narrows to samples", () => {
    const hits = searchLibrary("kick drum", seedLibrary(), {
      kind: "Sample",
      topK: 8,
    });
    expect(hits.length).toBeGreaterThan(0);
    expect(hits[0].item.id).toBe("sample_boomy_kick");
    for (const h of hits) expect(h.item.kind).toBe("Sample");
  });

  test("every seed validates against the generated schema", () => {
    const lib = seedLibrary();
    expect(lib).toHaveLength(8);
    for (const item of lib) {
      expect(() => LibraryItemSchema.parse(item)).not.toThrow();
    }
    const kinds = new Set(lib.map((i) => i.kind));
    for (const k of ["Sample", "Preset", "Plugin", "Project"]) {
      expect(kinds.has(k as never)).toBe(true);
    }
  });

  test("embedding is deterministic and L2-normalized", () => {
    const a = embedText("Warm Analog Pad");
    const b = embedText("Warm Analog Pad");
    expect(a).toHaveLength(64);
    expect(a).toEqual(b);
    const norm = Math.sqrt(a.reduce((s, x) => s + x * x, 0));
    expect(Math.abs(norm - 1)).toBeLessThan(1e-6);
    expect(embedText("")).toEqual(new Array(64).fill(0));
  });

  test("empty query returns id order with zero scores", () => {
    const hits = searchLibrary("", seedLibrary(), { topK: 8 });
    expect(hits).toHaveLength(8);
    for (const h of hits) expect(h.score).toBe(0);
    const ids = hits.map((h) => h.item.id);
    expect([...ids].sort()).toEqual(ids);
  });
});
