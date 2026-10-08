import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { InMemoryBackend } from "../src/backend";
import { ComposeStore, COMPOSE_TOOL_NAMES, parseNoteText, sequenceLength } from "../src/compose";
import { createServer } from "../src/index";
import { GameAudioStore } from "../src/gameaudio";

const STUB = resolve(import.meta.dir, "fixtures/stub-compose");
const REPO = resolve(import.meta.dir, "../..");
const REAL = ["release", "debug"].map((p) => join(REPO, "target", p, "ccez-compose")).find(existsSync);

function withBin<T>(bin: string, f: () => T): T {
  const old = process.env["CCEZ_COMPOSE_BIN"];
  process.env["CCEZ_COMPOSE_BIN"] = bin;
  try {
    return f();
  } finally {
    if (old === undefined) delete process.env["CCEZ_COMPOSE_BIN"];
    else process.env["CCEZ_COMPOSE_BIN"] = old;
  }
}

// Each mutation spawns the validator; allow for a busy machine.
const SLOW = 30_000;

describe("note text notation", () => {
  test("lengths are sticky, chords use +, rests advance time", () => {
    const n = parseNoteText("A4/1.5 G4/0.5 F4 r/1 D3+F3+A3/2@0.4", 4);
    expect(n).toEqual([
      { pitches: ["A4"], start: 4, length: 1.5 },
      { pitches: ["G4"], start: 5.5, length: 0.5 },
      { pitches: ["F4"], start: 6, length: 0.5 },
      { pitches: ["D3", "F3", "A3"], start: 7.5, length: 2, velocity: 0.4 },
    ]);
    expect(sequenceLength("A4/1.5 G4/0.5 F4 r/1 D3+F3+A3/2")).toBe(5.5);
    expect(parseNoteText("60/2")).toEqual([{ pitches: [60], start: 0, length: 2 }]);
    expect(() => parseNoteText("A4/x")).toThrow("can't read");
    expect(() => parseNoteText("A4@2")).toThrow("velocity");
  });
});

describe("compose store (stub CLI)", () => {
  test("tools need a composition first, with a plain reason", () => {
    withBin(STUB, () => {
      const s = new ComposeStore();
      expect(() => s.addLayer({ id: "bed" })).toThrow("call compose_new");
    });
  });

  test("chords expand, replace vs append, upserts keep one row", () => {
    withBin(STUB, () => {
      const s = new ComposeStore();
      s.newDoc({ name: "t", tempo: 90 });
      s.addSection({ id: "calm", role: "loop", bars: 2 });
      expect(s.addSection({ id: "calm", role: "loop", bars: 4 })).toMatchObject({ updated: "calm", beats: 16 });
      s.addLayer({ id: "bed" });
      s.addInstrument({ id: "keys", patch: "felt_piano" });
      const chord = { pitches: ["D3", "F3", "A3"], start: 0, length: 4 };
      expect(s.writeNotes({ section: "calm", layer: "bed", instrument: "keys", notes: [chord] })).toMatchObject({
        notes: 3,
        ok: true,
      });
      s.writeNotes({
        section: "calm",
        layer: "bed",
        instrument: "keys",
        notes: [{ pitch: "A4", start: 4, length: 1 }],
        mode: "append",
      });
      expect(s.doc!.parts[0]!.notes).toHaveLength(4);
      // text + repeat: a one-bar ostinato written twice
      s.writeNotes({ section: "calm", layer: "bed", instrument: "keys", text: "D3/0.5 A3 F3 A3 r/2", repeat: 2, start: 4 });
      const starts = s.doc!.parts[0]!.notes.map((n) => n.start);
      expect(starts).toEqual([4, 4.5, 5, 5.5, 8, 8.5, 9, 9.5]);
      expect(s.doc!.sections).toHaveLength(1);
      expect(() =>
        s.writeNotes({ section: "calm", layer: "bed", instrument: "keys", notes: [{ start: 0, length: 1 }] }),
      ).toThrow("give pitch or pitches");
    });
  }, SLOW);

  test("preview and export pass the plan through to the CLI", () => {
    withBin(STUB, () => {
      const s = new ComposeStore();
      s.newDoc({ name: "t", tempo: 90 });
      const plan = [{ section: "calm", intensity: 3 }];
      expect(s.preview({ out: "/tmp/x.mp3", plan })).toMatchObject({ cmd: "preview", out: "/tmp/x.mp3", plan });
      expect(s.exportAudioforge({ dir: "/tmp/pkg" })).toMatchObject({ cmd: "export", plan: null });
    });
  }, SLOW);

  test("a missing CLI says how to build it", () => {
    withBin("/nonexistent/ccez-compose", () => {
      const s = new ComposeStore();
      s.newDoc({ name: "t", tempo: 90 });
      expect(() => s.validate()).toThrow("cargo build --release");
    });
  });
});

describe("compose tools over MCP", () => {
  test("every compose tool is listed after the registry rows", async () => {
    const [ct, st] = InMemoryTransport.createLinkedPair();
    await createServer(new InMemoryBackend()).connect(st);
    const client = new Client({ name: "compose-test", version: "0" });
    await client.connect(ct);
    const names = (await client.listTools()).tools.map((t) => t.name);
    expect(names.slice(-COMPOSE_TOOL_NAMES.length)).toEqual([...COMPOSE_TOOL_NAMES]);
    await client.close();
  });

  // Real audio: needs core's ccez-compose built (CI builds it in the
  // compose-e2e job and sets CCEZ_COMPOSE_E2E=1 so this can't skip there).
  const e2e = REAL !== undefined || process.env["CCEZ_COMPOSE_E2E"] === "1";
  const dirs: string[] = [];
  afterAll(() => dirs.forEach((d) => rmSync(d, { recursive: true, force: true })));

  test.skipIf(!e2e)("an agent composes, previews and exports through MCP", async () => {
    const [ct, st] = InMemoryTransport.createLinkedPair();
    await createServer(new InMemoryBackend(), new GameAudioStore(), new ComposeStore()).connect(st);
    const client = new Client({ name: "compose-e2e", version: "0" });
    await client.connect(ct);
    const call = async (name: string, args: Record<string, unknown>) => {
      const r = (await client.callTool({ name, arguments: args })) as any;
      if (r.isError) throw new Error(r.content[0].text);
      return JSON.parse(r.content[0].text);
    };
    const fixture = join(REPO, "contracts/fixtures/audioforge-tiny.composition.json");
    expect((await call("compose_open", { path: fixture })).ok).toBe(true);
    const dir = mkdtempSync(join(tmpdir(), "compose-e2e-"));
    dirs.push(dir);
    const prev = await call("compose_preview", { out: join(dir, "p.wav") });
    expect(prev.stats.peak_dbfs).toBeLessThanOrEqual(0);
    expect(prev.stats.warnings).toEqual([]);
    expect(existsSync(join(dir, "p.wav"))).toBe(true);
    const rep = await call("compose_export_audioforge", { dir: join(dir, "pkg") });
    expect(rep.stems).toBe(4);
    const score = JSON.parse(readFileSync(join(dir, "pkg/score.json"), "utf8"));
    expect(score.sections.intro).toMatchObject({ once: true, next: "calm" });
    // A bad edit comes back as validation errors, not a crash.
    const bad = await call("compose_write_notes", {
      section: "calm",
      layer: "bed",
      instrument: "keys",
      notes: [{ pitch: "D4", start: 99, length: 1 }],
    });
    expect(bad.ok).toBe(false);
    expect(bad.errors.join()).toContain("outside the section");
    await client.close();
  }, 120_000);
});
