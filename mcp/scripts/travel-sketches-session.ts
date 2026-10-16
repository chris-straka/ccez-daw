#!/usr/bin/env bun
/**
 * Five contrasting travel-theme sketches, each composed through the MCP
 * compose_* tools (progressions for voicings, motifs and their
 * variations, the arranger for bass and pads, explicit notes for grooves
 * the arranger has no pattern for: odd meters, swing, cross-rhythms),
 * each on a `calm` layer (always) and a `lively` layer (intensity 3+).
 *
 *   bun mcp/scripts/travel-sketches-session.ts <out dir> [lufs] [only]
 *
 * Writes per sketch: <id>.mp3 (calm pass, then lively pass), the
 * composition, and what the ears heard (<id>.heard.json).
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { InMemoryBackend } from "../src/backend";
import { ComposeStore } from "../src/compose";
import { GameAudioStore } from "../src/gameaudio";
import { createServer } from "../src/index";

const out = resolve(process.argv[2] ?? "travel_sketches_out");
const lufs = Number(process.argv[3] ?? -18.8);
const only = process.argv[4];
mkdirSync(out, { recursive: true });

const [ct, st] = InMemoryTransport.createLinkedPair();
await createServer(new InMemoryBackend(), new GameAudioStore(), new ComposeStore()).connect(st);
const client = new Client({ name: "travel-sketches", version: "0" });
await client.connect(ct);
async function call(tool: string, args: Record<string, unknown> = {}): Promise<any> {
  const r = (await client.callTool({ name: tool, arguments: args })) as any;
  const text = r.content?.[0]?.text ?? "";
  if (r.isError) throw new Error(`${tool}: ${text}`);
  return JSON.parse(text);
}

// ---------------------------------------------------------------- notes

type Row = { pitch: string | number; start: number; length: number; velocity?: number };
const NAMES: Record<string, number> = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };
function midi(p: string | number): number {
  if (typeof p === "number") return p;
  const m = /^([A-G])([#b]?)(-?\d+)$/.exec(p)!;
  return (Number(m[3]) + 1) * 12 + NAMES[m[1]!]! + (m[2] === "#" ? 1 : m[2] === "b" ? -1 : 0);
}
function len(s: string): number {
  const [a, b] = s.split("/");
  return b === undefined ? Number(a) : Number(a) / Number(b);
}
/** `C4:2/3 F4+A4:1/3@0.6 r:1` — lengths in beats (fractions allowed), time advances. */
function seq(text: string, start = 0, vel?: number): Row[] {
  const rows: Row[] = [];
  let t = start;
  for (const tok of text.trim().split(/\s+/)) {
    const m = /^([^:@]+):([0-9./]+)(?:@([0-9.]+))?$/.exec(tok);
    if (!m) throw new Error(`bad token ${tok}`);
    const l = len(m[2]!);
    if (m[1] !== "r") {
      for (const p of m[1]!.split("+")) {
        const r: Row = { pitch: p, start: t, length: l };
        const v = m[3] !== undefined ? Number(m[3]) : vel;
        if (v !== undefined) r.velocity = v;
        rows.push(r);
      }
    }
    t += l;
  }
  return rows;
}
const shift = (rows: Row[], beats: number) => rows.map((r) => ({ ...r, start: r.start + beats }));
const oct = (rows: Row[], n: number) => rows.map((r) => ({ ...r, pitch: midi(r.pitch) + 12 * n }));
async function write(section: string, layer: string, instrument: string, notes: Row[]) {
  const r = await call("compose_write_notes", { section, layer, instrument, notes });
  if (r.errors?.length) throw new Error(`${instrument}: ${r.errors.join("; ")}`);
}
/** Voice-led chords from the progression tool: [{ start, bass, voices }] in MIDI. */
async function voiced(key: string, chords: string[], every: number, voicing: Record<string, unknown>) {
  const r = await call("compose_progression", { key, chords, every, voicing });
  if (r.parallel_fifths_octaves > 0) console.error(`  (${r.parallel_fifths_octaves} parallels in ${key})`);
  return (r.chords as any[]).map((c): Voiced => ({ start: c.start as number, symbol: c.symbol as string, bass: midi(c.bass), voices: (c.voices as string[]).map(midi) }));
}
type Voiced = { start: number; symbol: string; bass: number; voices: number[] };
/** The chord tone a fifth-ish above the bass (the real fifth of aug/dim chords too). */
function fifthOf(c: Voiced, hi: number): number {
  const b = c.bass % 12;
  const pcs = c.voices.map((p) => p % 12);
  const pc = [7, 8, 6].map((i) => (b + i) % 12).find((x) => pcs.includes(x)) ?? b;
  let p = c.bass + ((pc - b + 12) % 12);
  if (p > hi) p -= 12;
  return p;
}
/** Accompaniment notes that rub the tune (minor 2nd/9th for half a beat
 *  or more) move to another tone of their chord, or drop. */
function deRub(rows: Row[], tune: Row[], chords: Voiced[], every: number): Row[] {
  const rubs = (p: number, r: Row) =>
    tune.some((m) => {
      const ov = Math.min(m.start + m.length, r.start + r.length) - Math.max(m.start, r.start);
      const d = Math.abs(midi(m.pitch) - p);
      return ov >= 0.5 && (d === 1 || d === 13);
    });
  return rows.flatMap((r) => {
    const p = midi(r.pitch);
    if (!rubs(p, r)) return [r];
    const c = chords[Math.min(chords.length - 1, Math.floor(r.start / every))]!;
    const pcs = new Set([c.bass % 12, ...c.voices.map((v) => v % 12)]);
    for (let d = 1; d <= 7; d++) for (const q of [p - d, p + d]) if (pcs.has(q % 12) && !rubs(q, r)) return [{ ...r, pitch: q }];
    return [];
  });
}
const fold = (rows: Row[], lo: number, hi: number) =>
  rows.map((r) => {
    let p = midi(r.pitch);
    while (p > hi) p -= 12;
    while (p < lo) p += 12;
    return { ...r, pitch: p };
  });

async function finish(id: string, intensityLively = 4) {
  await call("compose_set_master", { lufs, ceilingDbtp: -1 });
  const plan = [
    { section: "travel", intensity: 1 },
    { section: "travel", intensity: intensityLively },
  ];
  const ears = await call("compose_listen", { plan, spectrogram: join(out, `${id}.spectrogram.jpg`) });
  const prev = await call("compose_preview", { out: join(out, `${id}.mp3`), plan });
  await call("compose_save", { path: join(out, `${id}.composition.json`) });
  writeFileSync(join(out, `${id}.heard.json`), JSON.stringify({ ears, master: prev.master, stats: prev.stats }, null, 2));
  console.log(`\n== ${id}`);
  console.log(ears.composition.summary.join("\n"));
  console.log("levels", Object.entries(prev.stats.instrument_rms_dbfs).map(([k, v]) => `${k} ${(v as number).toFixed(1)}`).join(", "));
  if (prev.stats.warnings.length) console.log("warnings", prev.stats.warnings);
}
async function instruments(list: [string, string, number, number, number?][]) {
  for (const [id, patch, gainDb, reverb, pan] of list) {
    await call("compose_add_instrument", { id, patch, gainDb, reverb, ...(pan !== undefined ? { pan } : {}) });
  }
}
async function layers(calmFx: Record<string, unknown> = {}, livelyFx: Record<string, unknown> = {}) {
  await call("compose_add_layer", { id: "calm", minIntensity: 1 });
  // The lively layer has to be heard when it joins: +5 dB over its parts' balance.
  await call("compose_add_layer", { id: "lively", minIntensity: 3, gainDb: 5 });
  await call("compose_set_layer_fx", { layer: "calm", ...calmFx });
  await call("compose_set_layer_fx", { layer: "lively", ...livelyFx });
}

// ------------------------------------------- 1. Lopsided Lighthouse (7/8)
// G Lydian (the raised C# gives it light), seven eighth-note beats a bar
// grouped 2+2+3, a marimba figure that limps on purpose, a solo violin
// singing over it. Lively: congas, shaker, plucked violin off-beats and
// a glockenspiel echo of the tune.
async function lighthouse() {
  const B = 7; // beats per bar, the beat is an eighth note
  await call("compose_new", { name: "lopsided_lighthouse", tempo: 228, beatsPerBar: B, key: "G lydian" });
  await call("compose_set_room", { decaySecs: 2.0, predelayMs: 18, damping: 0.4 });
  await call("compose_add_section", { id: "travel", role: "loop", bars: 16 });
  await layers({ eq: { lowDb: 1 } }, { eq: { lowCutHz: 200 }, reverb: 0.8 });
  await instruments([
    ["violin", "solo_violin", -1, 0.35, -0.1],
    ["marimba", "marimba", 0, 0.3, 0.25],
    ["bass", "basses_pizz", 0, 0.2, 0.3],
    ["pad", "violas", -9, 0.5, -0.3],
    ["glock", "glockenspiel", -2, 0.4, 0.35],
    ["pizz", "violins_pizz", 1, 0.25, -0.45],
    ["kalimba", "kalimba", 0, 0.35, 0.5],
    ["congas", "congas", 3, 0.15, 0.15],
    ["shaker", "shaker", 0, 0.15, -0.25],
  ]);
  const chords = ["G", "A/G", "G", "A/G", "Em", "D", "Cmaj7", "D", "G", "A/G", "Bm", "A", "Em", "F#m", "Cmaj7", "Dsus4"];
  const v = await voiced("G lydian", chords, B, { voices: 3, low: "G3", high: "G5", bassLow: "E1", bassHigh: "D3" });
  const tune =
    "D5:2 C#5:2 D5:3 E5:2 F#5:2 E5:1 C#5:2 B4:4 A4:1 B4:2 C#5:7 " +
    "G5:2 F#5:2 E5:3 F#5:2 A5:2 F#5:1 D5:2 E5:4 G5:3 F#5:7 " +
    "D5:2 C#5:2 D5:2 G5:1 A5:2 G5:2 F#5:1 E5:2 D5:2 F#5:2 B5:3 A5:4 E5:3 " +
    "G5:2 E5:2 B4:3 C#5:2 E5:2 A5:3 G5:2 E5:2 B4:3 A4:2 C#5:2 D5:3";
  await call("compose_motif", { name: "tune", notes: seq(tune, 0, 0.78) });
  await write("travel", "calm", "violin", seq(tune, 0, 0.78));
  // Marimba: low, top, mid, top | low, mid | top, mid, top (2+2+3), accents on group starts.
  const mar: Row[] = [];
  const bass: Row[] = [];
  v.forEach((c, i) => {
    const t = i * B;
    const [lo, mid, hi] = c.voices as [number, number, number];
    [lo, hi, mid, hi, lo, mid, hi].forEach((p, k) =>
      mar.push({ pitch: p, start: t + k, length: 1, velocity: [0, 2, 4].includes(k) ? 0.62 : 0.42 }),
    );
    bass.push({ pitch: c.bass, start: t, length: 3.5, velocity: 0.72 });
    bass.push({ pitch: fifthOf(c, 55), start: t + 4, length: 2.5, velocity: 0.6 });
  });
  const lightTune = seq(tune, 0, 0.78);
  await write("travel", "calm", "marimba", deRub(mar, lightTune, v, B));
  await write("travel", "calm", "bass", bass);
  await write("travel", "calm", "pad", deRub(v.flatMap((c) => c.voices.slice(0, 2).map((p) => ({ pitch: p, start: c.start, length: B, velocity: 0.3 }))), lightTune, v, B));
  // Lively: glockenspiel doubles the tune an octave up.
  await write("travel", "lively", "glock", oct(seq(tune, 0, 0.5), 1));
  await write("travel", "lively", "pizz", deRub(fold(v.flatMap((c) => [1, 3, 6].flatMap((k) => c.voices.slice(1).map((p) => ({ pitch: p + 12, start: c.start + k, length: 0.5, velocity: 0.55 })))), 55, 86), lightTune, v, B));
  // Kalimba picks out the marimba's top voice an octave up on the off-beats.
  await write("travel", "lively", "kalimba", deRub(fold(v.flatMap((c) => [1, 3, 5].map((k) => ({ pitch: c.voices[2]! + 12, start: c.start + k, length: 1, velocity: 0.5 }))), 60, 84), lightTune, v, B));
  const cong: Row[] = [];
  const shak: Row[] = [];
  for (let bar = 0; bar < 16; bar++) {
    const t = bar * B;
    // 65 tumba (low), 61 conga open, 63 quinto.
    for (const [k, key, vel] of [[0, 65, 0.75], [2, 61, 0.6], [4, 65, 0.7], [5, 63, 0.5], [6, 61, 0.55]] as const) cong.push({ pitch: key, start: t + k, length: 1, velocity: vel });
    for (let k = 0; k < B; k++) shak.push({ pitch: k % 2 ? 65 : 64, start: t + k, length: 1, velocity: [0, 2, 4].includes(k) ? 0.5 : 0.32 });
  }
  await write("travel", "lively", "congas", cong);
  await write("travel", "lively", "shaker", shak);
  await finish("1-lopsided-lighthouse");
}

// --------------------------------------------- 2. Bassoon Errand (swing)
// F Mixolydian (the flat seventh, Eb, makes it sly), swung eighths, a
// bassoon strolling the tune over tuba and off-beat piano. Lively: a
// clarinet joins an octave up, the xylophone answers each phrase, brushed
// snare taps, cajon and tambourine.
async function errand() {
  await call("compose_new", { name: "bassoon_errand", tempo: 116, key: "F mixolydian" });
  await call("compose_set_room", { decaySecs: 1.4, predelayMs: 12, damping: 0.55 });
  await call("compose_add_section", { id: "travel", role: "loop", bars: 16 });
  await layers({}, { eq: { lowCutHz: 180 }, reverb: 0.7 });
  await instruments([
    ["bassoon", "bassoon", 2, 0.2, -0.1],
    ["tuba", "tuba", -1, 0.15, 0.3],
    ["piano", "piano", -5, 0.25, 0.15],
    ["clarinet", "clarinet", 0, 0.25, -0.35],
    ["xylo", "xylophone", -1, 0.3, 0.4],
    ["snare", "snare", -2, 0.15, 0.1],
    ["cajon", "cajon", 3, 0.12, 0],
    ["tamb", "tambourine", -3, 0.15, -0.3],
  ]);
  const chords = ["F", "Eb/F", "F", "Eb/F", "Bb", "Cm7", "Dm7", "Eb", "F", "Eb/F", "Gm7", "Cm7", "Bb", "Cm7", "Ebmaj7", "F"];
  const v = await voiced("F mixolydian", chords, 4, { voices: 3, low: "A3", high: "A4", bassLow: "F1", bassHigh: "C3" });
  const S = "2/3";
  const s = "1/3";
  const bars = [
    `F3:${S} A3:${s} C4:${S} F4:${s} Eb4:1 C4:1`,
    `Bb3:${S} C4:${s} Eb4:1 D4:${S} C4:${s} Bb3:1`,
    `A3:${S} C4:${s} F4:3/2 G4:1/2 A4:1`,
    `G4:${S} F4:${s} Eb4:${S} C4:${s} F4:2`,
    `D4:${S} F4:${s} Bb4:1 A4:${S} G4:${s} F4:1`,
    `Eb4:${S} G4:${s} Bb4:1 G4:${S} Eb4:${s} C4:1`,
    `F4:${S} A4:${s} C5:1 A4:${S} F4:${s} D4:1`,
    `G4:2 Bb3:${S} C4:${s} Eb4:1`,
    `F3:${S} A3:${s} C4:${S} F4:${s} Eb4:1 C4:1`,
    `Bb3:${S} C4:${s} Eb4:1 D4:${S} C4:${s} Bb3:1`,
    `Bb3:${S} D4:${s} G4:1 F4:${S} D4:${s} Bb3:1`,
    `C4:${S} Eb4:${s} G4:2 F4:1`,
    `D4:${S} F4:${s} Bb4:1 A4:${S} G4:${s} F4:1`,
    `Eb4:${S} G4:${s} Bb4:1 G4:${S} Eb4:${s} C4:1`,
    `D4:${S} Eb4:${s} G4:1 Bb4:1 G4:1`,
    `F4:${S} C4:${s} A3:${S} C4:${s} F3:2`,
  ];
  const tune = bars.flatMap((b, i) => seq(b, i * 4, 0.82));
  await call("compose_motif", { name: "stroll", notes: tune.slice(0, 6) });
  await write("travel", "calm", "bassoon", tune);
  await write("travel", "lively", "clarinet", oct(tune, 1).map((r) => ({ ...r, velocity: 0.55 })));
  const tuba: Row[] = [];
  const piano: Row[] = [];
  v.forEach((c) => {
    tuba.push({ pitch: c.bass, start: c.start, length: 0.9, velocity: 0.75 });
    tuba.push({ pitch: fifthOf(c, 58), start: c.start + 2, length: 0.9, velocity: 0.65 });
    for (let b = 0; b < 4; b++) for (const p of c.voices) piano.push({ pitch: p, start: c.start + b + 2 / 3, length: 0.25, velocity: b % 2 ? 0.5 : 0.4 });
  });
  await write("travel", "calm", "tuba", tuba);
  await write("travel", "calm", "piano", deRub(piano, tune, v, 4));
  // Xylophone answers in the gaps (bars 4, 8, 12, 16), the stroll motif
  // turned upside down and run fast.
  await call("compose_vary", { motif: "stroll", ops: [{ op: "invert" }, { op: "augment", factor: 0.5 }, { op: "octave", octaves: 2 }], saveAs: "answer" });
  const answer = (await call("compose_get")).document.motifs.answer as Row[];
  await write("travel", "lively", "xylo", [3, 7, 11, 15].flatMap((bar) => shift(answer, bar * 4 + 2).map((r) => ({ ...r, velocity: 0.6 }))));
  const sn: Row[] = [];
  const cj: Row[] = [];
  const tb: Row[] = [];
  for (let bar = 0; bar < 16; bar++) {
    for (let b = 0; b < 4; b++) {
      const t = bar * 4 + b;
      sn.push({ pitch: 60, start: t, length: 2 / 3, velocity: b % 2 ? 0.34 : 0.22 }, { pitch: 60, start: t + 2 / 3, length: 1 / 3, velocity: 0.16 });
      if (b % 2 === 0) cj.push({ pitch: 60, start: t, length: 1, velocity: 0.7 });
      else tb.push({ pitch: 60, start: t, length: 1, velocity: 0.55 });
    }
    cj.push({ pitch: 62, start: bar * 4 + 3 + 2 / 3, length: 1 / 3, velocity: 0.45 });
  }
  await write("travel", "lively", "snare", sn);
  await write("travel", "lively", "cajon", cj);
  await write("travel", "lively", "tamb", tb);
  await finish("2-bassoon-errand");
}

// ------------------------------------------- 3. Tidepool Clock (3 vs 4)
// Ab major turning to Db Lydian: a vibraphone cell in dotted eighths
// (four notes every three beats) against marimba eighths in 4/4, so the
// pattern keeps landing somewhere new and lines up again every three
// bars; a solo violin floats long notes above. Lively: frame drum,
// shaker, a harp ripple and the glockenspiel on the tune.
async function tidepool() {
  await call("compose_new", { name: "tidepool_clock", tempo: 104, key: "Ab major" });
  await call("compose_set_room", { decaySecs: 3.0, predelayMs: 30, damping: 0.3 });
  await call("compose_add_section", { id: "travel", role: "loop", bars: 24 });
  await layers({ eq: { highDb: 1 } }, { eq: { lowCutHz: 220 }, reverb: 0.8 });
  await instruments([
    ["violin", "solo_violin", -1, 0.45, -0.15],
    ["vibes", "vibraphone", -2, 0.4, 0.35],
    ["marimba", "marimba", -4, 0.3, -0.3],
    ["cello", "cellos", -8, 0.4, 0.25],
    ["harp", "harp", 2, 0.4, 0.45],
    ["glock", "glockenspiel", 1, 0.45, 0.2],
    ["frame", "frame_drum", 4, 0.2, 0],
    ["shaker", "shaker", 2, 0.15, -0.35],
  ]);
  // A chord every three bars (12 beats), when the 3:4 cycle realigns.
  const chords = ["Ab", "Fm7", "Dbmaj7", "Ebsus4", "Ab/C", "Bbm7", "Db", "Eb"];
  const v = await voiced("Ab major", chords, 12, { voices: 3, low: "Ab3", high: "Ab5", bassLow: "C2", bassHigh: "Eb3" });
  const vib: Row[] = [];
  const mar: Row[] = [];
  const cel: Row[] = [];
  v.forEach((c) => {
    const [a, b, d] = c.voices as [number, number, number];
    const cell = [a + 12, d, b + 12, d];
    for (let k = 0; k < 16; k++) vib.push({ pitch: cell[k % 4]!, start: c.start + k * 0.75, length: 0.75, velocity: k % 4 === 0 ? 0.55 : 0.4 });
    const run = [a, b, d, b + 12, d, b, a, b];
    for (let k = 0; k < 24; k++) mar.push({ pitch: run[k % 8]!, start: c.start + k * 0.5, length: 0.5, velocity: k % 2 ? 0.32 : 0.45 });
    cel.push({ pitch: c.bass, start: c.start, length: 12, velocity: 0.42 });
  });
  const line =
    "Eb5:4 C5:2 Db5:2 Eb5:4 F5:6 Eb5:2 C5:4 Ab5:4 G5:4 F5:4 G5:6 F5:2 Eb5:4 " +
    "C6:4 Bb5:2 Ab5:2 G5:4 F5:4 Db5:4 Bb4:4 Ab4:4 Bb4:2 C5:2 Db5:4 Ab5:6 G5:6";
  await call("compose_motif", { name: "line", text: line.replace(/:/g, "/") });
  const lineRows = seq(line, 0, 0.72);
  await write("travel", "calm", "violin", lineRows);
  await write("travel", "calm", "vibes", deRub(vib, lineRows, v, 12));
  await write("travel", "calm", "marimba", deRub(mar, lineRows, v, 12));
  await write("travel", "calm", "cello", cel);
  await write("travel", "lively", "glock", oct(seq(line, 0, 0.45), 1));
  await call("compose_arrange", {
    section: "travel",
    chords,
    every: 12,
    ensemble: [{ instrument: "harp", role: "arp", layer: "lively", subdivision: 0.25, velocity: 0.38 }],
  });
  const fr: Row[] = [];
  const sh: Row[] = [];
  for (let bar = 0; bar < 24; bar++) {
    const t = bar * 4;
    // 61 low hit, 64 small hit, 63 small hand: the dotted 3:4 feel.
    fr.push({ pitch: 61, start: t, length: 1, velocity: 0.7 }, { pitch: 64, start: t + 1.5, length: 0.5, velocity: 0.45 }, { pitch: 63, start: t + 2.25, length: 0.5, velocity: 0.4 }, { pitch: 61, start: t + 3, length: 1, velocity: 0.5 });
    for (let k = 0; k < 16; k++) sh.push({ pitch: k % 2 ? 65 : 64, start: t + k * 0.25, length: 0.25, velocity: k % 4 === 0 ? 0.4 : 0.22 });
  }
  await write("travel", "lively", "frame", fr);
  await write("travel", "lively", "shaker", sh);
  await finish("3-tidepool-clock");
}

// ----------------------------------------------- 4. Moth Lanterns (6/8)
// E Dorian (minor, but its major sixth, C#, keeps it hopeful), a slow
// 6/8 lilt for dusk; an oboe tune over a wordless choir and harp. It
// borrows C major chords for a wistful turn. Lively: a horn counter-line,
// high violins, kalimba twinkles and a soft frame-drum heartbeat.
async function lanterns() {
  const B = 6; // eighth-note beats
  await call("compose_new", { name: "moth_lanterns", tempo: 192, beatsPerBar: B, key: "E dorian" });
  await call("compose_set_room", { decaySecs: 3.2, predelayMs: 30, damping: 0.5 });
  await call("compose_add_section", { id: "travel", role: "loop", bars: 24 });
  await layers({ eq: { highCutHz: 12000 } }, { eq: { lowCutHz: 200 }, reverb: 1.1 });
  await instruments([
    ["oboe", "oboe", 0, 0.35, -0.1],
    ["choir", "choir_oohs", -8, 0.55, 0],
    ["harp", "harp", -3, 0.4, 0.4],
    ["cello", "cellos", -7, 0.35, 0.25],
    ["horn", "horn", -4, 0.45, -0.35],
    ["violins", "violins", -5, 0.55, -0.45],
    ["kalimba", "kalimba", -2, 0.45, 0.3],
    ["frame", "frame_drum", 0, 0.25, 0],
  ]);
  const chords = [
    "Em9", "A/E", "Em9", "A/E", "Gmaj7", "F#m7", "Bm7", "D6",
    "Em9", "A/E", "Cmaj7", "D", "Em", "A/C#", "Cmaj7", "B7sus4",
    "Gmaj7", "A", "Bm7", "Cmaj7", "D", "A/C#", "Cmaj7", "D",
  ];
  const v = await voiced("E dorian", chords, B, { voices: 3, low: "G3", high: "G5", bassLow: "E2", bassHigh: "D3" });
  const tune =
    "B4:2 E5:1 F#5:2 G5:1 A5:3 F#5:2 E5:1 D5:2 E5:1 B4:3 C#5:2 D5:1 E5:3 " +
    "B5:2 A5:1 G5:2 F#5:1 E5:3 E5:3 D5:2 F#5:1 A5:2 B5:1 A5:6 " +
    "B5:2 G5:1 E5:2 F#5:1 E5:2 C#5:1 A4:3 B4:2 C5:1 E5:2 G5:1 F#5:3 E5:2 D5:1 " +
    "E5:2 B4:1 G4:2 B4:1 C#5:2 E5:1 A5:3 G5:2 E5:1 B5:3 A5:3 F#5:2 E5:1 " +
    "D6:2 B5:1 G5:3 C#6:2 A5:1 E5:3 D6:2 C#6:1 B5:2 A5:1 G5:3 E5:3 " +
    "F#5:2 A5:1 D6:3 C#6:2 B5:1 A5:2 E5:1 G5:2 F#5:1 E5:2 D5:1 E5:3 F#5:3";
  const oboe = seq(tune, 0, 0.75);
  await write("travel", "calm", "oboe", oboe);
  // Harp broken chords: low-mid-top | top-mid-top, in eighths.
  const harp: Row[] = [];
  v.forEach((c) => {
    const [a, b, d] = c.voices as [number, number, number];
    [a, b, d, d + 12 > 88 ? d : d + 12, d, b].forEach((p, k) => harp.push({ pitch: p, start: c.start + k, length: 2, velocity: k === 0 || k === 3 ? 0.5 : 0.36 }));
  });
  await write("travel", "calm", "harp", deRub(harp, oboe, v, B));
  await write("travel", "calm", "cello", v.map((c) => ({ pitch: c.bass, start: c.start, length: B, velocity: 0.45 })));
  // Pads hold plain triads/sevenths (the colour notes stay in harp and tune).
  const plain = chords.map((c) => c.replace("9", "").replace("6", "").replace("maj7", ""));
  await call("compose_arrange", {
    section: "travel",
    chords: plain,
    every: B,
    melody: { notes: oboe },
    fill: false,
    ensemble: [
      { instrument: "choir", role: "pad", layer: "calm", velocity: 0.38 },
      { instrument: "horn", role: "counter", layer: "lively", velocity: 0.55 },
      { instrument: "violins", role: "pad", layer: "lively", velocity: 0.34 },
    ],
  });
  // The counter-line steps through passing notes: keep those off the tune and bass.
  const doc = (await call("compose_get")).document;
  const cello = v.map((c) => ({ pitch: c.bass, start: c.start, length: B }));
  const hornPart = doc.parts.find((x: any) => x.instrument === "horn").notes as Row[];
  // The harp keeps the ninths and sixths, so the choir steps off any it would rub.
  const choirPart = deRub(doc.parts.find((x: any) => x.instrument === "choir").notes as Row[], [...oboe, ...harp], v, B);
  await write("travel", "calm", "choir", choirPart);
  await write("travel", "lively", "horn", deRub(hornPart, [...oboe, ...cello, ...choirPart, ...harp], v, B));
  const violinPart = doc.parts.find((x: any) => x.instrument === "violins").notes as Row[];
  await write("travel", "lively", "violins", deRub(violinPart, [...oboe, ...choirPart, ...harp], v, B));
  // Kalimba twinkles on the off-beats of each half bar.
  await write("travel", "lively", "kalimba", deRub(v.flatMap((c) => [[1, c.voices[2]!], [4, c.voices[1]! + 12], [5, c.voices[2]!]].map(([k, p]) => ({ pitch: p! > 84 ? p! - 12 : p!, start: c.start + k!, length: 1, velocity: 0.42 }))), oboe, v, B));
  const fr: Row[] = [];
  for (let bar = 0; bar < 24; bar++) fr.push({ pitch: 61, start: bar * B, length: 2, velocity: 0.6 }, { pitch: 60, start: bar * B + 3, length: 2, velocity: 0.42 });
  await write("travel", "lively", "frame", fr);
  await finish("4-moth-lanterns");
}

// ------------------------------------------ 5. Seven Keys March (5/4)
// The acoustic scale (Bb with a bright E natural and a soft Ab: Lydian and
// Mixolydian at once) in 5/4, grouped 3+2, an oom-pah that trips over its
// own extra beat. The tune opens on seven rising notes, one per lock, then
// plays them backwards and a step higher. Lively: piccolo two octaves up,
// trumpet answers, snare and xylophone.
async function march() {
  await call("compose_new", { name: "seven_keys_march", tempo: 138, beatsPerBar: 5, key: "F melodic_minor" });
  await call("compose_set_room", { decaySecs: 1.8, predelayMs: 16, damping: 0.45 });
  await call("compose_add_section", { id: "travel", role: "loop", bars: 16 });
  await layers({}, { eq: { lowCutHz: 200 }, reverb: 0.75 });
  await instruments([
    ["horn", "horn", -4, 0.3, -0.25],
    ["tuba", "tuba", 0, 0.15, 0.3],
    ["pizz", "violins_pizz", -2, 0.25, -0.4],
    ["clarinet", "clarinet", -3, 0.3, 0.2],
    ["piccolo", "piccolo", 1, 0.3, 0.3],
    ["trumpet", "trumpet", 1, 0.3, 0.15],
    ["xylo", "xylophone", 1, 0.3, 0.45],
    ["snare", "snare", 3, 0.15, 0],
  ]);
  // Seven rising notes, one per lock.
  const locks = await call("compose_motif", { name: "locks", text: "Bb4/0.5 C5 D5 E5 F5 G5 Ab5/2" });
  const back = await call("compose_vary", { motif: "locks", ops: [{ op: "retrograde" }] });
  const up = await call("compose_vary", { motif: "locks", ops: [{ op: "transpose", steps: 1 }] });
  const motifSeq = (text: string) => {
    // notation text from the tools -> seq tokens (sticky lengths resolved).
    let l = "1";
    return text.split(/\s+/).map((tok) => {
      const [p, x] = tok.split("/");
      if (x) l = x;
      return `${p}:${l}`;
    }).join(" ");
  };
  const tuneText = [
    motifSeq(locks.text),
    "G5:3/2 E5:1/2 C5:1 D5:1 E5:1",
    motifSeq(back.text),
    "C5:1 E5:1 G5:1 C5:2",
    "F5:3/2 Ab5:1/2 G5:1 F5:1 C5:1",
    "D5:3/2 F5:1/2 G5:1 Bb5:2",
    "C5:1 E5:1 Ab5:1 G5:1 E5:1",
    "E5:2 D5:1/2 C5:1/2 Bb4:2",
    motifSeq(up.text),
    "Bb5:3/2 G5:1/2 E5:1 F5:1 G5:1",
    "F5:3/2 D5:1/2 Bb4:1 D5:1 F5:1",
    "E5:1 Ab5:1 C6:3",
    "Ab5:3/2 G5:1/2 F5:1 C5:1 F5:1",
    "G5:3/2 F5:1/2 D5:1 Bb4:2",
    "C5:1/2 E5:1/2 Ab5:1/2 C6:1/2 Ab5:1 E5:2",
    "G5:1 E5:1 C5:1 Bb4:2",
  ];
  const tune = tuneText.flatMap((b, i) => seq(b, i * 5, 0.8));
  await write("travel", "calm", "horn", oct(tune, -1));
  // Piccolo plays the tune as written: two octaves over the horn.
  await write("travel", "lively", "piccolo", fold(tune.map((r) => ({ ...r, velocity: 0.55 })), 74, 91));
  const chords = ["Bb7", "C/Bb", "Bb7", "C/Bb", "Fm7", "Gm7", "Abaug", "C7", "Bb7", "C/Bb", "Gm7", "Abaug", "Fm7", "Gm7", "Abaug", "C7"];
  const v = await voiced("F melodic_minor", chords, 5, { voices: 3, low: "G4", high: "G5", bassLow: "Bb1", bassHigh: "C3" });
  const tuba: Row[] = [];
  const pz: Row[] = [];
  const cl: Row[] = [];
  v.forEach((c) => {
    tuba.push({ pitch: c.bass, start: c.start, length: 0.8, velocity: 0.75 }, { pitch: fifthOf(c, 55), start: c.start + 3, length: 0.8, velocity: 0.68 });
    for (const k of [1, 2, 4]) for (const p of c.voices) pz.push({ pitch: p, start: c.start + k, length: 0.4, velocity: k === 4 ? 0.55 : 0.45 });
    // Clarinet: a slow inner line on the chord's middle voice, lively only.
    cl.push({ pitch: c.voices[1]! - 12, start: c.start, length: 3, velocity: 0.45 }, { pitch: c.voices[0]! - 12, start: c.start + 3, length: 2, velocity: 0.4 });
  });
  const horn = oct(tune, -1);
  await write("travel", "calm", "tuba", deRub(tuba, horn, v, 5));
  await write("travel", "calm", "pizz", deRub(pz, horn, v, 5));
  await write("travel", "lively", "clarinet", deRub(cl, horn, v, 5));
  // Trumpet joins every statement of the seven-lock motif (bars 1, 3, 9).
  await write("travel", "lively", "trumpet", tune.filter((r) => [0, 2, 8].includes(Math.floor(r.start / 5))).map((r) => ({ ...r, velocity: 0.62 })));
  await write("travel", "lively", "xylo", [3, 7, 11, 15].flatMap((bar) => seq("Bb5:1/4 C6:1/4 D6:1/4 E6:1/4 F6:1/2 r:1/2", bar * 5 + 3, 0.55)));
  const sn: Row[] = [];
  for (let bar = 0; bar < 16; bar++) {
    const t = bar * 5;
    for (const [k, key, vel] of [[0, 61, 0.7], [1, 65, 0.35], [1.5, 65, 0.3], [2, 61, 0.5], [3, 61, 0.65], [3.5, 65, 0.3], [4, 65, 0.4], [4.5, 65, 0.35]] as const) sn.push({ pitch: key, start: t + k, length: 0.5, velocity: vel });
  }
  await write("travel", "lively", "snare", sn);
  await finish("5-seven-keys-march");
}

const sketches: Record<string, () => Promise<void>> = { lighthouse, errand, tidepool, lanterns, march };
for (const [name, f] of Object.entries(sketches)) {
  if (only && only !== name) continue;
  await f();
}
await client.close();
