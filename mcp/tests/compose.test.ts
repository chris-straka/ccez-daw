import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { InMemoryBackend } from "../src/backend";
import { ComposeStore, COMPOSE_TOOL_NAMES, formatNoteText, parseNoteText, sequenceLength, targetDirs } from "../src/compose";
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

  test("notes read back as the same notation", () => {
    const text = "D5/0.5 F#5 A5/1 r/0.5 D4+F#4/1.5";
    const rows = parseNoteText(text).flatMap((n) => n.pitches!.map((p) => ({ pitch: p, start: n.start, length: n.length })));
    expect(formatNoteText(rows)).toBe(text);
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
    await client.close();
  }, 120_000);

  test.skipIf(!e2e)("an agent writes with motifs, chords and an ensemble, then listens and opens it in the DAW", async () => {
    const [ct, st] = InMemoryTransport.createLinkedPair();
    await createServer(new InMemoryBackend(), new GameAudioStore(), new ComposeStore()).connect(st);
    const client = new Client({ name: "compose-write-e2e", version: "0" });
    await client.connect(ct);
    const call = async (name: string, args: Record<string, unknown>) => {
      const r = (await client.callTool({ name, arguments: args })) as any;
      if (r.isError) throw new Error(r.content[0].text);
      return JSON.parse(r.content[0].text);
    };
    const dir = mkdtempSync(join(tmpdir(), "compose-write-"));
    dirs.push(dir);
    await call("compose_new", { name: "walk", tempo: 100, key: "D major" });
    await call("compose_add_section", { id: "a", role: "loop", bars: 4 });
    for (const [id, patch] of [["lead", "celesta"], ["pad", "warm_pad"], ["low", "soft_bass"], ["pluck", "pizzicato"]]) {
      await call("compose_add_instrument", { id, patch });
    }
    // State a motif and vary it in the key.
    expect((await call("compose_motif", { name: "hook", text: "D5/1 E5 F#5 A5" })).text).toBe("D5/1 E5 F#5 A5");
    const inv = await call("compose_vary", { motif: "hook", ops: [{ op: "invert" }], saveAs: "hook_inv" });
    expect(inv.text).toBe("D5/1 C#5 B4 G4");
    const aug = await call("compose_vary", { motif: "hook", ops: [{ op: "augment", factor: 2 }] });
    expect(aug.text).toBe("D5/2 E5 F#5 A5");
    // A named progression, voice-led.
    const presets = await call("compose_progression", {});
    expect(presets.presets.some((p: any) => p.name === "royal_road")).toBe(true);
    const prog = await call("compose_progression", { chords: ["I", "V", "vi", "IV"] });
    expect(prog.chords.map((c: any) => c.symbol)).toEqual(["D", "A", "Bm", "G"]);
    expect(prog.parallel_fifths_octaves).toBe(0);
    // Chords found for the tune.
    const re = await call("compose_reharmonise", { motif: "hook", every: 2 });
    expect(re.chords.length).toBe(2);
    // Arrange onto calm and lively layers (created on the way).
    const arr = await call("compose_arrange", {
      section: "a",
      melody: { motif: "hook" },
      chords: ["I", "IV", "V", "I"],
      ensemble: [
        { instrument: "lead", role: "melody", layer: "calm" },
        { instrument: "pad", role: "pad", layer: "calm" },
        { instrument: "low", role: "bass", layer: "calm" },
        { instrument: "pluck", role: "arp", layer: "lively" },
      ],
    });
    expect(arr.ok).toBe(true);
    expect(arr.added).toContain("layer lively (intensity 3+)");
    expect(arr.parts).toHaveLength(4);
    await call("compose_set_layer_fx", { layer: "lively", eq: { lowCutHz: 300 }, reverb: 0.5 });
    expect((await call("compose_set_master", { lufs: -18 })).ok).toBe(true);
    // Ears: tempo and key come back, and the lively layer is heard.
    const ears = await call("compose_listen", { spectrogram: join(dir, "s.jpg") });
    expect(ears.preview.features.key.key).toBe("D major");
    expect(ears.composition.layers.find((l: any) => l.layer === "lively").adds_lu).toBeGreaterThan(0);
    expect(ears.composition.summary.join("\n")).toContain("intensity 1 -> 5");
    expect(existsSync(join(dir, "s.jpg"))).toBe(true);
    // The preview is mastered to the target.
    const prev = await call("compose_preview", { out: join(dir, "p.wav") });
    expect(Math.abs(prev.master.lufs_after - -18)).toBeLessThan(1);
    const cmp = await call("compose_listen", { audio: join(dir, "p.wav"), reference: join(dir, "p.wav") });
    expect(cmp.reference.comparison.overall).toBeGreaterThan(0.99);
    // Open in the DAW: a project bundle with audio and MIDI tracks.
    const daw = await call("compose_open_in_daw", { dir: join(dir, "proj") });
    expect(daw.audio_tracks).toBe(2);
    expect(daw.midi_tracks).toBe(4);
    expect(existsSync(join(dir, "proj/project.json"))).toBe(true);
    await client.close();
  }, 180_000);

  test.skipIf(!e2e)("a transcription becomes the current composition", async () => {
    const dir = mkdtempSync(join(tmpdir(), "compose-tr-"));
    dirs.push(dir);
    // basic-pitch's note-event CSV: D-major bars at 120 BPM.
    const rows = ["start_time_s,end_time_s,pitch_midi,velocity,pitch_bend"];
    for (let bar = 0; bar < 4; bar++) {
      const t0 = bar * 2;
      rows.push(`${t0},${t0 + 1.9},38,90`);
      [74, 78, 81, 78].forEach((p, k) => rows.push(`${t0 + k * 0.5},${t0 + k * 0.5 + 0.45},${p},90`));
    }
    const csv = join(dir, "take.csv");
    await Bun.write(csv, rows.join("\n"));
    const s = new ComposeStore();
    const r = s.transcribe({ audio: csv, tempo: 120, bars: 4, out: join(dir, "take.json") }) as any;
    expect(r.transcription.key).toBe("D major");
    expect(r.transcription.chords).toEqual(["D", "D", "D", "D"]);
    expect(r.composition.ok).toBe(true);
    expect(s.doc!.motifs!["transcribed_melody"]!.length).toBe(16);
  }, 60_000);
});
