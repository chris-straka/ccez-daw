import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { InMemoryBackend } from "../src/backend";
import { ComposeStore, COMPOSE_TOOL_NAMES, midiPitch, parseNoteText, sequenceLength, targetDirs } from "../src/compose";
import { createServer } from "../src/index";
import { GameAudioStore } from "../src/gameaudio";

const STUB = resolve(import.meta.dir, "fixtures/stub-compose");
const REPO = resolve(import.meta.dir, "../..");
const REAL = targetDirs()
  .flatMap((d) => ["release", "debug"].map((p) => join(d, p, "ccez-compose")))
  .find(existsSync);

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

  test("pitch names follow core parse_pitch", () => {
    expect(["C4", "A4", "F#3", "Bb2", "C-1", "G9", 0, 127].map(midiPitch)).toEqual([60, 69, 54, 46, 0, 127, 0, 127]);
    for (const bad of ["H2", "D", "zz", "60", "G#9", 128, 60.5]) expect(() => midiPitch(bad)).toThrow();
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

  test("a bad pitch is rejected before the doc changes; the session keeps working", () => {
    withBin(STUB, () => {
      const s = new ComposeStore();
      s.newDoc({ name: "t", tempo: 90 });
      s.addSection({ id: "calm", role: "loop", bars: 2 });
      s.writeNotes({ section: "calm", layer: "bed", instrument: "keys", text: "D4/1" });
      const before = structuredClone(s.doc);
      const at = { section: "calm", layer: "bed", instrument: "keys" };
      expect(() => s.writeNotes({ ...at, text: "200/1" })).toThrow("outside MIDI 0..=127");
      expect(() => s.writeNotes({ ...at, notes: [{ pitch: "zz", start: 0, length: 1 }] })).toThrow("note name");
      expect(() => s.writeNotes({ ...at, notes: [{ pitch: "H2", start: 0, length: 1 }], mode: "append" })).toThrow(
        "note name",
      );
      expect(() => s.writeNotes({ ...at, text: "G#9/1" })).toThrow("outside MIDI 0..=127");
      expect(s.doc).toEqual(before);
      // Every validating tool still answers (one bad call can't brick it).
      expect(s.get()).toMatchObject({ ok: true });
      expect(s.addLayer({ id: "bed" })).toMatchObject({ added: "bed", ok: true });
      // Opening a document the CLI cannot load keeps the working one.
      const kept = structuredClone(s.doc);
      const dir = mkdtempSync(join(tmpdir(), "compose-bad-"));
      const path = join(dir, "bad.json");
      writeFileSync(path, JSON.stringify({ ...kept, parts: [{ ...kept!.parts[0], notes: [{ pitch: 200, start: 0, length: 1 }] }] }));
      try {
        expect(() => s.open({ path })).toThrow("0..=127");
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
      expect(s.doc).toEqual(kept);
    });
  }, SLOW);

  test("a mutation whose validation throws is rolled back", () => {
    const s = new ComposeStore();
    withBin(STUB, () => s.newDoc({ name: "t", tempo: 90 }));
    const before = structuredClone(s.doc);
    withBin("/nonexistent/ccez-compose", () => {
      expect(() => s.addLayer({ id: "bed" })).toThrow("cargo build");
      expect(() => s.addSection({ id: "calm", role: "loop", bars: 2 })).toThrow("cargo build");
      expect(() => s.setRoom({ decaySecs: 3 })).toThrow("cargo build");
    });
    expect(s.doc).toEqual(before);
  });

  test("preview and export pass the plan through to the CLI", () => {
    withBin(STUB, () => {
      const s = new ComposeStore();
      s.newDoc({ name: "t", tempo: 90 });
      const plan = [{ section: "calm", intensity: 3 }];
      expect(s.preview({ out: "/tmp/x.mp3", plan })).toMatchObject({ cmd: "preview", out: "/tmp/x.mp3", plan });
      expect(s.exportAudioforge({ dir: "/tmp/pkg" })).toMatchObject({ cmd: "export", plan: null });
    });
  }, SLOW);

  test("instruments: family filter, details, fetch, pan and room reach the CLI", () => {
    withBin(STUB, () => {
      const s = new ComposeStore();
      expect(s.patches({ family: "strings" })).toEqual([
        { id: "violins", kind: "sampled", family: "strings", description: "stub", range: [55, 86] },
      ]);
      expect((s.patches() as unknown[]).length).toBe(2);
      expect(s.instrument({ id: "bongos" })).toEqual({ cmd: "instrument", args: ["bongos"] });
      expect(s.fetch({ ids: ["violins", "flute"] })).toEqual({ cmd: "fetch", args: ["violins", "flute"] });
      s.newDoc({ name: "t", tempo: 90 });
      s.addSection({ id: "calm", role: "loop", bars: 2 });
      s.addInstrument({ id: "lead", patch: "flute", pan: -0.3 });
      s.addInstrument({ id: "keys", patch: "piano" });
      expect(s.doc!.instruments[0]).toEqual({ id: "lead", patch: "flute", gain_db: 0, reverb: 0.25, pan: -0.3 });
      expect("pan" in s.doc!.instruments[1]!).toBe(false);
      s.setRoom({ decaySecs: 3 });
      expect(s.doc!.room).toEqual({ decay_secs: 3, predelay_ms: 22, damping: 0.45 });
      s.setRoom({ damping: 0.8 });
      expect(s.doc!.room).toEqual({ decay_secs: 3, predelay_ms: 22, damping: 0.8 });
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
    // The sampled catalog lists without downloading anything.
    const strings = await call("compose_patches", { family: "strings" });
    const violins = strings.find((p: any) => p.id === "violins");
    expect(violins).toMatchObject({ kind: "sampled", family: "strings", licence: "CC0-1.0" });
    expect(strings.some((p: any) => p.id === "violins_pizz")).toBe(true);
    expect((await call("compose_patches", { family: "choir" }))[0].licence).toBe("MIT");
    const fixture = join(REPO, "contracts/fixtures/audioforge-tiny.composition.json");
    expect((await call("compose_open", { path: fixture })).ok).toBe(true);
    const dir = mkdtempSync(join(tmpdir(), "compose-e2e-"));
    dirs.push(dir);
    const prev = await call("compose_preview", { out: join(dir, "p.wav") });
    expect(prev.stats.peak_dbfs).toBeLessThanOrEqual(0);
    expect(prev.stats.loop_seams.every((s: any) => s.seamless)).toBe(true);
    expect(Object.keys(prev.stats.instrument_rms_dbfs).sort()).toEqual(["keys", "tick"]);
    // Stereo: the WAV header says two channels.
    expect(readFileSync(join(dir, "p.wav")).readUInt16LE(22)).toBe(2);
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
    // A pitch the CLI can't load is refused up front; the session still answers.
    const refused = (await client.callTool({
      name: "compose_write_notes",
      arguments: { section: "calm", layer: "bed", instrument: "keys", text: "200/1" },
    })) as any;
    expect(refused.isError).toBe(true);
    expect((await call("compose_get", {})).errors.join()).toContain("outside the section");
    await client.close();
  }, 120_000);
});
