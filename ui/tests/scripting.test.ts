import { describe, expect, test } from "bun:test";
import type { Op } from "../src/generated/project";
import { OpSchema } from "../src/generated/project";
import { compileScript, runScript } from "../src/scripting/api";
import { ScriptSandboxError } from "../src/scripting/sandbox";

const DEMO_SCRIPT = `
  addTrack("Strings");
  addClip({
    id: "clip_script_1",
    track_id: "trk_music",
    name: "Scripted",
    start_beats: 12,
    length_beats: 4,
    kind: "Midi",
    source: "take:script",
  });
  moveClip("clip_script_1", 16);
  setParam("trk_music", "volume", 0.5);
  setTempo(128);
`;

describe("Track K scripting API", () => {
  test("a script compiles to ordinary undoable ops with a script: actor", async () => {
    const ops = await compileScript("demo", DEMO_SCRIPT);
    expect(ops).toHaveLength(5);
    expect(ops.map((o) => o.kind)).toEqual([
      "TrackAdded",
      "ClipAdded",
      "ClipMoved",
      "ParamSet",
      "TempoSet",
    ]);
    for (const op of ops) {
      expect(op.actor).toBe("script:demo");
      expect(() => OpSchema.parse(op)).not.toThrow();
    }
    // Payload shapes the Rust op log replays.
    expect(ops[0].value_json).toBe(JSON.stringify("Strings"));
    expect(JSON.parse(ops[1].value_json)).toMatchObject({ id: "clip_script_1" });
    expect(JSON.parse(ops[2].value_json)).toEqual({ startBeats: 16 });
    expect(ops[3].target).toBe("trk_music:volume");
    expect(ops[4].target).toBe("transport");
  });

  test("runScript applies every op through op_apply and returns seqs", async () => {
    const seen: Op[] = [];
    let seq = 40;
    const seqs = (
      await runScript("demo", DEMO_SCRIPT, async (op) => {
        seen.push(op);
        return ++seq;
      })
    ).seqs;
    expect(seen).toHaveLength(5);
    expect(seqs).toEqual([41, 42, 43, 44, 45]);
    // Undo path stays intact: every applied op carries its seq target.
    expect(seen.every((o) => o.actor === "script:demo")).toBe(true);
  });

  test("sandbox blocks host escapes", async () => {
    const bad = [
      `import x from "y"; addTrack("nope");`,
      `const p = process.env; addTrack("nope");`,
      `eval("addTrack('nope')");`,
      `fetch("https://example.com");`,
      `invoke("project_get", {});`,
    ];
    for (const source of bad) {
      await expect(compileScript("evil", source)).rejects.toBeInstanceOf(ScriptSandboxError);
    }
  });

  test("script input validation rejects bad musical values", async () => {
    await expect(compileScript("demo", `setTempo(0);`)).rejects.toThrow();
    await expect(compileScript("demo", `setParam("n", "p", NaN);`)).rejects.toThrow();
    await expect(
      compileScript("demo", `moveClip("c", Infinity);`),
    ).rejects.toThrow();
  });
});
