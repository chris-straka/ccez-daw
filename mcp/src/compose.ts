import { z } from "zod";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

/**
 * Game-music composer tools (`compose_*`). The MCP server keeps one
 * composition document (the JSON `core/src/compose` reads) and hands it
 * to the Rust `ccez-compose` CLI for validation, rendering and export,
 * so agents get the same synth, the same seamless loop stems, and the
 * same audioforge package the core tests pin. Nothing here fakes audio:
 * if the CLI is missing, the tool says how to build it.
 */

const REPO = resolve(import.meta.dir, "../..");

/** Where cargo puts binaries: $CARGO_TARGET_DIR when set, else `target/`. */
export function targetDirs(): string[] {
  const env = process.env["CARGO_TARGET_DIR"];
  return env ? [resolve(REPO, env), join(REPO, "target")] : [join(REPO, "target")];
}

/** Resolve the CLI: $CCEZ_COMPOSE_BIN, a built binary, else `cargo run`. */
export function composeCommand(): string[] {
  const env = process.env["CCEZ_COMPOSE_BIN"];
  if (env) return [env];
  for (const dir of targetDirs()) {
    for (const profile of ["release", "debug"]) {
      const bin = join(dir, profile, "ccez-compose");
      if (existsSync(bin)) return [bin];
    }
  }
  return [
    "cargo",
    "run",
    "--release",
    "-q",
    "--manifest-path",
    join(REPO, "core/Cargo.toml"),
    "--bin",
    "ccez-compose",
    "--",
  ];
}

export function runCompose(args: string[]): unknown {
  const [cmd, ...pre] = composeCommand();
  const r = spawnSync(cmd!, [...pre, ...args], { encoding: "utf8", maxBuffer: 64 << 20 });
  if (r.error) {
    throw new Error(
      `ccez-compose unavailable (${r.error.message}); build it: cargo build --release --manifest-path core/Cargo.toml --bin ccez-compose`,
    );
  }
  if (r.status !== 0) {
    throw new Error((r.stderr || `ccez-compose exited ${r.status}`).trim());
  }
  return JSON.parse(r.stdout);
}

export const PitchSchema = z.union([z.string(), z.number().int().min(0).max(127)]);
export const NoteSchema = z.object({
  pitch: PitchSchema.optional().describe('MIDI number or name like "D4", "F#3", "Bb2" (C4 = 60)'),
  pitches: z
    .array(PitchSchema)
    .optional()
    .describe("several pitches at once (a chord); same start/length/velocity"),
  start: z.number().min(0).describe("beats from the section start"),
  length: z.number().positive().describe("beats"),
  velocity: z.number().min(0).max(1).optional(),
});
export type NoteInput = z.infer<typeof NoteSchema>;

export const ComposeNewInput = z.object({
  name: z.string().describe("lowercase id [a-z0-9_]+ (prefixes exported event names)"),
  tempo: z.number().min(40).max(300),
  beatsPerBar: z.number().int().min(1).max(16).optional(),
  key: z.string().optional(),
});
export const InstrumentInput = z.object({
  id: z.string(),
  patch: z
    .string()
    .describe("an id from compose_patches: a sampled instrument (violins, flute, piano, harp, choir_aahs, timpani, bongos, ...) or a synth patch"),
  gainDb: z.number().optional(),
  reverb: z.number().min(0).max(1).optional().describe("send into the shared hall, 0-1 (default 0.25)"),
  pan: z
    .number()
    .min(-1)
    .max(1)
    .optional()
    .describe("stereo position -1 left .. 1 right (default: the instrument's orchestral seat)"),
});
export const PatchesInput = z.object({
  family: z
    .enum(["synth", "strings", "woodwinds", "brass", "keys", "plucked", "mallets", "choir", "percussion", "hand_drums"])
    .optional()
    .describe("only this family"),
});
export const InstrumentIdInput = z.object({ id: z.string().describe("an id from compose_patches") });
export const FetchInput = z.object({
  ids: z.array(z.string()).min(1).describe('sampled instrument ids, or ["all"]'),
});
export const RoomInput = z.object({
  decaySecs: z.number().min(0.3).max(8).optional().describe("reverb tail RT60 in seconds (default 2.4)"),
  predelayMs: z.number().min(0).max(120).optional().describe("gap before the reverb (default 22)"),
  damping: z.number().min(0).max(1).optional().describe("0 bright stone .. 1 dark wood (default 0.45)"),
});
export const LayerInput = z.object({
  id: z.string(),
  minIntensity: z
    .number()
    .int()
    .min(1)
    .max(5)
    .optional()
    .describe("lowest game intensity (1-5) at which the layer sounds; 1 = always"),
  gainDb: z.number().optional(),
});
export const SectionInput = z.object({
  id: z.string(),
  role: z.enum(["intro", "loop", "outro"]),
  bars: z.number().int().min(1).max(256),
  next: z.string().optional().describe("intro only: the section it hands off to"),
});
/**
 * Compact sequential notation, one token per event, time advancing by each
 * token's length: `PITCH[/LEN][@VEL]`, chords `D3+F3+A3/4`, rests `r/2`.
 * LEN defaults to the previous token's (1 beat to start), VEL to 0.8.
 * Example (a bar of 4/4): `A4/1.5 G4/0.5 F4/1 E4/0.5 F4`.
 */
export function parseNoteText(text: string, start = 0): NoteInput[] {
  const out: NoteInput[] = [];
  let t = start;
  let len = 1;
  for (const tok of text.trim().split(/\s+/).filter(Boolean)) {
    const m = /^([^/@]+)(?:\/([0-9.]+))?(?:@([0-9.]+))?$/.exec(tok);
    if (!m) throw new Error(`notes text: can't read ${JSON.stringify(tok)} (want e.g. D4/1.5@0.6, D3+F3/2, r/1)`);
    if (m[2] !== undefined) {
      len = Number(m[2]);
      if (!(len > 0)) throw new Error(`notes text: ${JSON.stringify(tok)} needs a positive length`);
    }
    const vel = m[3] !== undefined ? Number(m[3]) : undefined;
    if (vel !== undefined && !(vel >= 0 && vel <= 1)) {
      throw new Error(`notes text: ${JSON.stringify(tok)} velocity must be 0..1`);
    }
    const head = m[1]!;
    if (head !== "r" && head !== "R") {
      const pitches = head.split("+").map((p) => (/^\d+$/.test(p) ? Number(p) : p));
      const n: NoteInput = { pitches, start: t, length: len };
      if (vel !== undefined) n.velocity = vel;
      out.push(n);
    }
    t += len;
  }
  return out;
}

export const WriteNotesInput = z.object({
  section: z.string(),
  layer: z.string(),
  instrument: z.string(),
  notes: z.array(NoteSchema).optional().describe("explicit notes; or use text"),
  text: z
    .string()
    .optional()
    .describe(
      "sequential notation from `start`: PITCH[/LEN][@VEL] tokens, chords D3+F3+A3/4, rests r/2 (LEN in beats, sticky)",
    ),
  start: z.number().min(0).optional().describe("beat where text begins (default 0)"),
  repeat: z.number().int().min(1).max(64).optional().describe("repeat text back to back this many times"),
  mode: z.enum(["replace", "append"]).optional().describe("default replace (this section/layer/instrument)"),
});
export const PlanStepSchema = z.object({
  section: z.string(),
  bars: z.number().int().min(1).optional(),
  intensity: z.number().int().min(1).max(5).optional(),
});
export const PreviewInput = z.object({
  out: z.string().describe("file path ending .wav or .mp3"),
  plan: z.array(PlanStepSchema).optional().describe("default: intro, each loop at intensity 1 then 5, outro"),
});
export const ExportAudioforgeInput = z.object({
  dir: z.string(),
  plan: z.array(PlanStepSchema).optional().describe("the preview scene written into the package"),
});
export const PathInput = z.object({ path: z.string() });

/** A line of notes: a stored motif, notation text, or explicit notes. */
export const NotesSource = z.object({
  motif: z.string().optional().describe("name of a motif stated with compose_motif"),
  text: z.string().optional().describe("notation like 'D5/1 F#5 A5/2' (beats from 0)"),
  notes: z.array(NoteSchema).optional(),
});
const WriteTarget = z.object({
  section: z.string(),
  layer: z.string(),
  instrument: z.string(),
  start: z.number().min(0).optional().describe("beat in the section where it goes (default 0)"),
  mode: z.enum(["replace", "append"]).optional().describe("default append for variations, replace otherwise"),
});
export const ListenInput = z.object({
  audio: z
    .string()
    .optional()
    .describe("an audio file (wav/mp3/ogg/flac) to listen to instead of the current composition"),
  reference: z.string().optional().describe("a track to compare against (e.g. the ACE-Step take or a game's current theme)"),
  spectrogram: z.string().optional().describe("write a spectrogram picture here (.jpg) to look at"),
  plan: z.array(PlanStepSchema).optional().describe("what the preview plays (default: intro, loops at 1 and 5, outro)"),
});
export const CompareInput = z.object({ a: z.string(), b: z.string() });
export const MotifInput = NotesSource.extend({
  name: z.string().describe("lowercase id; the motif is kept in the composition"),
});
export const MotifOpSchema = z.discriminatedUnion("op", [
  z.object({ op: z.literal("transpose"), steps: z.number().int().describe("scale steps (stays in key)") }),
  z.object({ op: z.literal("chromatic"), semitones: z.number().int() }),
  z.object({ op: z.literal("invert"), axis: PitchSchema.optional().describe("mirror pitch (default the first note)") }),
  z.object({ op: z.literal("retrograde") }),
  z.object({ op: z.literal("augment"), factor: z.number().positive().describe("2 = twice as slow, 0.5 = twice as fast") }),
  z.object({ op: z.literal("sequence"), times: z.number().int().min(0).max(32), steps: z.number().int() }),
  z.object({ op: z.literal("fragment"), from: z.number().min(0), to: z.number().positive() }),
  z.object({ op: z.literal("displace"), beats: z.number() }),
  z.object({ op: z.literal("octave"), octaves: z.number().int() }),
  z.object({ op: z.literal("dynamics"), scale: z.number().min(0) }),
]);
export const VaryInput = z.object({
  motif: z.string().describe("the motif to vary"),
  ops: z.array(MotifOpSchema).describe("applied in order, e.g. [{op:'invert'},{op:'augment',factor:2}]"),
  key: z.string().optional().describe("default: the composition's key"),
  saveAs: z.string().optional().describe("keep the result as a new motif"),
  write: WriteTarget.optional().describe("also write it into a part"),
});
const Voicing = z.object({
  voices: z.number().int().min(2).max(5).optional().describe("upper voices (default 3)"),
  low: PitchSchema.optional().describe("lowest upper voice (default E3)"),
  high: PitchSchema.optional().describe("highest upper voice (default E5)"),
  bassLow: PitchSchema.optional(),
  bassHigh: PitchSchema.optional(),
});
const PatternEnum = z.enum(["block", "arp_up", "arp_up_down", "broken", "pulse", "root_fifth", "root"]);
const HarmonyWrite = z.object({
  section: z.string(),
  start: z.number().min(0).optional(),
  repeat: z.number().int().min(1).max(64).optional().describe("repeat the progression back to back"),
  upper: z.object({ layer: z.string(), instrument: z.string() }).optional().describe("where the voiced chords go"),
  bass: z.object({ layer: z.string(), instrument: z.string() }).optional().describe("where the bass line goes"),
  mode: z.enum(["replace", "append"]).optional(),
});
export const ProgressionInput = z.object({
  key: z.string().optional().describe("e.g. 'D major', 'A dorian' (default: the composition's key)"),
  chords: z
    .array(z.string())
    .optional()
    .describe("numerals in the key (I, vi, IVmaj7, V7/V, bVII, IV/3) or symbols (Em7, F#m7b5)"),
  preset: z.string().optional().describe("a named progression (omit chords and preset to list them)"),
  every: z.number().positive().optional().describe("beats per chord (default 4)"),
  voicing: Voicing.optional(),
  pattern: PatternEnum.optional().describe("how the upper voices play (default block)"),
  bassPattern: PatternEnum.optional().describe("default root_fifth"),
  subdivision: z.number().positive().optional().describe("beats per note for arp/pulse (default 0.5)"),
  velocity: z.number().min(0).max(1).optional(),
  write: HarmonyWrite.optional(),
});
export const ReharmoniseInput = NotesSource.extend({
  key: z.string().optional(),
  every: z.number().positive().optional().describe("beats per chord (default a bar)"),
  palette: z.enum(["plain", "colour"]).optional().describe("plain = the key's triads; colour adds 7ths, borrowed chords, secondary dominants"),
  voicing: Voicing.optional(),
  pattern: PatternEnum.optional(),
  write: HarmonyWrite.optional(),
});
const RoleEnum = z.enum(["melody", "double", "sparkle", "counter", "bass", "pad", "arp", "pulse", "broken", "drone", "percussion"]);
export const ArrangeInput = z.object({
  section: z.string(),
  melody: NotesSource.optional().describe("the tune (motif, text or notes); omit for accompaniment only"),
  fill: z.boolean().optional().describe("repeat the tune to fill the section (default true)"),
  chords: z.array(z.string()).optional().describe("one per `every` beats, cycling; omit to harmonise the tune"),
  every: z.number().positive().optional(),
  palette: z.enum(["plain", "colour"]).optional(),
  key: z.string().optional(),
  ensemble: z
    .array(
      z.object({
        instrument: z.string().describe("instrument id in the composition"),
        role: RoleEnum,
        layer: z.string(),
        octave: z.number().int().optional(),
        velocity: z.number().min(0).max(1).optional(),
        subdivision: z.number().positive().optional(),
        density: z.enum(["sparse", "normal", "busy"]).optional().describe("percussion"),
      }),
    )
    .optional()
    .describe("who plays what; or musicMd"),
  musicMd: z
    .string()
    .optional()
    .describe("a game's design/music.md with an '## Ensemble' table (Id | Patch | Role | Layer); missing instruments are added"),
  layerIntensity: z
    .record(z.string(), z.number().int().min(1).max(5))
    .optional()
    .describe("minIntensity for layers the ensemble creates (default: calm 1, lively 3, others 2)"),
});
export const LayerFxInput = z.object({
  layer: z.string(),
  eq: z
    .object({
      lowCutHz: z.number().min(20).max(2000).optional().describe("high-pass: clear mud from upper layers"),
      highCutHz: z.number().min(500).max(20000).optional().describe("low-pass: darken, push back"),
      lowDb: z.number().min(-18).max(18).optional().describe("shelf at 200 Hz"),
      midDb: z.number().min(-18).max(18).optional(),
      midHz: z.number().min(100).max(10000).optional(),
      highDb: z.number().min(-18).max(18).optional().describe("shelf at 6 kHz"),
    })
    .optional()
    .describe("replaces the layer's EQ; {} = flat"),
  reverb: z.number().min(0).max(2).optional().describe("scales every reverb send on this layer (1 = as set per instrument)"),
  room: RoomInput.optional().describe("this layer's own hall (default: the shared one)"),
});
export const MasterInput = z.object({
  lufs: z.number().min(-30).max(-8).optional().describe("loudness target (default -16; -18 for an in-game bed)"),
  ceilingDbtp: z.number().min(-6).max(0).optional().describe("true-peak ceiling (default -1)"),
  off: z.boolean().optional().describe("remove mastering"),
});
export const MasterAudioInput = z.object({
  in: z.string().describe("any audio file at 48 kHz (e.g. a rendered preview or an ACE-Step take)"),
  out: z.string().describe(".wav or .mp3"),
  lufs: z.number().min(-30).max(-8).optional(),
  ceilingDbtp: z.number().min(-6).max(0).optional(),
  loop: z.boolean().optional().describe("the file is a seamless loop: keep the wrap seamless"),
});
export const TranscribeInput = z.object({
  audio: z.string().describe("an audio take (wav/mp3/ogg), or a .mid / basic-pitch .csv"),
  out: z.string().optional().describe("where the composition JSON goes (default: a temp file); it becomes the current composition"),
  midi: z.string().optional().describe("also keep basic-pitch's MIDI file here"),
  tempo: z.number().min(40).max(300).optional().describe("default: heard from the audio"),
  key: z.string().optional().describe("default: detected from the notes"),
  bars: z.number().int().min(1).max(64).optional().describe("bars to keep (default 8)"),
  startBar: z.number().int().min(0).optional().describe("first bar to keep (default 0)"),
  from: z.number().min(0).optional().describe("seconds into the audio to start listening"),
  to: z.number().positive().optional().describe("seconds into the audio to stop"),
  name: z.string().optional(),
});
export const DawInput = z.object({
  dir: z.string().describe("a new folder for the DAW project (must not exist)"),
});
export const Empty = z.object({});

/**
 * Notes back into the sequential notation (what compose_write_notes reads),
 * so an agent can read a motif or a generated line at a glance. Notes that
 * start together become a chord; overlaps are shown up to the next onset.
 */
export function formatNoteText(notes: { pitch: string | number; start: number; length: number }[], from = 0): string {
  const groups = new Map<number, { pitch: string | number; length: number }[]>();
  for (const n of notes) {
    const k = Math.round(n.start * 1000) / 1000;
    groups.set(k, [...(groups.get(k) ?? []), n]);
  }
  const starts = [...groups.keys()].sort((a, b) => a - b);
  const out: string[] = [];
  let t = from;
  let len = -1;
  const tok = (head: string, l: number) => {
    const r = Math.round(l * 1000) / 1000;
    out.push(r === len ? head : `${head}/${r}`);
    len = r;
  };
  starts.forEach((s, i) => {
    if (s > t + 1e-6) tok("r", s - t);
    const g = groups.get(s)!;
    const longest = Math.max(...g.map((n) => n.length));
    const next = starts[i + 1];
    const l = next !== undefined ? Math.min(longest, next - s) : longest;
    tok(g.map((n) => String(n.pitch)).join("+"), l);
    t = s + l;
  });
  return out.join(" ");
}

type NoteRow = { pitch: string | number; start: number; length: number; velocity?: number };
type Eq = {
  low_cut_hz?: number;
  high_cut_hz?: number;
  low_db?: number;
  mid_db?: number;
  mid_hz?: number;
  high_db?: number;
};
type Room = { decay_secs: number; predelay_ms: number; damping: number };

type Doc = {
  schema_version: number;
  name: string;
  tempo: number;
  beats_per_bar: number;
  key: string;
  instruments: { id: string; patch: string; gain_db: number; reverb: number; pan?: number }[];
  room?: Room;
  layers: { id: string; min_intensity: number; gain_db: number; eq?: Eq; reverb?: number; room?: Room }[];
  sections: { id: string; role: string; bars: number; next?: string }[];
  parts: {
    section: string;
    layer: string;
    instrument: string;
    notes: NoteRow[];
  }[];
  motifs?: Record<string, NoteRow[]>;
  master?: { lufs: number; ceiling_dbtp: number };
};

function upsert<T extends { id: string }>(list: T[], item: T): "added" | "updated" {
  const i = list.findIndex((x) => x.id === item.id);
  if (i >= 0) {
    list[i] = item;
    return "updated";
  }
  list.push(item);
  return "added";
}

/** Total beats a notation string spans (rests included). */
export function sequenceLength(text: string): number {
  let t = 0;
  let len = 1;
  for (const tok of text.trim().split(/\s+/).filter(Boolean)) {
    const m = /\/([0-9.]+)/.exec(tok);
    if (m) len = Number(m[1]);
    t += len;
  }
  return t;
}

/** Note inputs (chords as `pitches`) to one row per pitch. */
function flatten(given: NoteInput[]): NoteRow[] {
  return given.flatMap((n) => {
    const pitches = n.pitches ?? (n.pitch !== undefined ? [n.pitch] : []);
    if (pitches.length === 0) throw new Error(`note at beat ${n.start}: give pitch or pitches`);
    return pitches.map((p) => {
      const out: NoteRow = { pitch: p, start: n.start, length: n.length };
      if (n.velocity !== undefined) out.velocity = n.velocity;
      return out;
    });
  });
}

function shiftRows(rows: NoteRow[], by: number): NoteRow[] {
  return rows.map((n) => ({ ...n, start: n.start + by }));
}

/** camelCase voicing input to the CLI's snake_case `VoicingOpts`. */
function voicingArg(v: z.infer<typeof Voicing> | undefined): unknown {
  if (!v) return undefined;
  const pitch = (p: string | number | undefined, dflt: number): string | number => p ?? dflt;
  return {
    voices: v.voices ?? 3,
    low: pitch(v.low, 52),
    high: pitch(v.high, 76),
    bass_low: pitch(v.bassLow, 36),
    bass_high: pitch(v.bassHigh, 55),
  };
}

/** One composition per server, persisted to a scratch file the CLI reads. */
export class ComposeStore {
  doc: Doc | null = null;
  readonly scratch = join(tmpdir(), `ccez-compose-${process.pid}-${Math.random().toString(36).slice(2, 8)}.json`);

  private need(): Doc {
    if (!this.doc) throw new Error("no composition: call compose_new (or compose_open) first");
    return this.doc;
  }

  private flush(): string {
    writeFileSync(this.scratch, JSON.stringify(this.need(), null, 2));
    return this.scratch;
  }

  /** Full validation from the Rust model (the export gate). */
  /** `ok` = exportable; `errors` need fixing now; `pending` are things
   *  referenced but not defined yet (normal while building up). */
  validate(): { ok: boolean; errors: string[]; pending: string[] } {
    const r = runCompose(["validate", this.flush()]) as { ok: boolean; errors: string[]; pending?: string[] };
    return { ok: r.ok, errors: r.errors, pending: r.pending ?? [] };
  }

  newDoc(a: z.infer<typeof ComposeNewInput>) {
    this.doc = {
      schema_version: 1,
      name: a.name,
      tempo: a.tempo,
      beats_per_bar: a.beatsPerBar ?? 4,
      key: a.key ?? "",
      instruments: [],
      layers: [],
      sections: [],
      parts: [],
    };
    return this.summary();
  }

  addInstrument(a: z.infer<typeof InstrumentInput>) {
    const d = this.need();
    const inst: Doc["instruments"][number] = {
      id: a.id,
      patch: a.patch,
      gain_db: a.gainDb ?? 0,
      reverb: a.reverb ?? 0.25,
    };
    if (a.pan !== undefined) inst.pan = a.pan;
    const r = upsert(d.instruments, inst);
    return { [r]: a.id, ...this.validate() };
  }

  setRoom(a: z.infer<typeof RoomInput>) {
    const d = this.need();
    d.room = {
      decay_secs: a.decaySecs ?? d.room?.decay_secs ?? 2.4,
      predelay_ms: a.predelayMs ?? d.room?.predelay_ms ?? 22,
      damping: a.damping ?? d.room?.damping ?? 0.45,
    };
    return { room: d.room, ...this.validate() };
  }

  addLayer(a: z.infer<typeof LayerInput>) {
    const d = this.need();
    const old = d.layers.find((l) => l.id === a.id);
    const r = upsert(d.layers, { ...old, id: a.id, min_intensity: a.minIntensity ?? 1, gain_db: a.gainDb ?? 0 });
    return { [r]: a.id, ...this.validate() };
  }

  addSection(a: z.infer<typeof SectionInput>) {
    const d = this.need();
    const s: Doc["sections"][number] = { id: a.id, role: a.role, bars: a.bars };
    if (a.next !== undefined) s.next = a.next;
    const r = upsert(d.sections, s);
    const beats = a.bars * d.beats_per_bar;
    return { [r]: a.id, beats, ...this.validate() };
  }

  writeNotes(a: z.infer<typeof WriteNotesInput>) {
    this.need();
    const given: NoteInput[] = [...(a.notes ?? [])];
    if (a.text !== undefined) {
      const once = parseNoteText(a.text, 0);
      const span = sequenceLength(a.text);
      for (let k = 0; k < (a.repeat ?? 1); k++) {
        for (const n of once) given.push({ ...n, start: n.start + (a.start ?? 0) + k * span });
      }
    }
    if (given.length === 0) throw new Error("give notes or text");
    const notes = flatten(given);
    const n = this.putNotes(a.section, a.layer, a.instrument, notes, a.mode ?? "replace");
    return { notes: n, written: notes.length, ...this.validate() };
  }

  /** Write rows into one part (created if missing); returns its note count. */
  putNotes(section: string, layer: string, instrument: string, notes: NoteRow[], mode: "replace" | "append"): number {
    const d = this.need();
    let part = d.parts.find((p) => p.section === section && p.layer === layer && p.instrument === instrument);
    if (!part) {
      part = { section, layer, instrument, notes: [] };
      d.parts.push(part);
    }
    if (mode === "replace") part.notes = notes;
    else part.notes.push(...notes);
    return part.notes.length;
  }

  summary() {
    const d = this.need();
    const sec = new Map(d.sections.map((s) => [s.id, s.bars * d.beats_per_bar]));
    return {
      name: d.name,
      tempo: d.tempo,
      beatsPerBar: d.beats_per_bar,
      key: d.key,
      instruments: d.instruments,
      layers: d.layers,
      sections: d.sections.map((s) => ({ ...s, beats: sec.get(s.id) })),
      parts: d.parts.map((p) => {
        const end = Math.max(0, ...p.notes.map((n) => n.start + n.length));
        return { section: p.section, layer: p.layer, instrument: p.instrument, notes: p.notes.length, lastBeat: end };
      }),
    };
  }

  get(): unknown {
    return { ...this.summary(), document: this.need(), ...this.validate() };
  }

  preview(a: z.infer<typeof PreviewInput>) {
    const args = ["preview", this.flush(), "--out", resolve(a.out)];
    if (a.plan) args.push("--plan", this.writePlan(a.plan));
    return runCompose(args);
  }

  exportAudioforge(a: z.infer<typeof ExportAudioforgeInput>) {
    const args = ["export", this.flush(), "--out", resolve(a.dir)];
    if (a.plan) args.push("--plan", this.writePlan(a.plan));
    return runCompose(args);
  }

  patches(a: z.infer<typeof PatchesInput> = {}): unknown {
    return runCompose(a.family ? ["patches", "--family", a.family] : ["patches"]);
  }

  instrument(a: z.infer<typeof InstrumentIdInput>): unknown {
    return runCompose(["instrument", a.id]);
  }

  fetch(a: z.infer<typeof FetchInput>): unknown {
    return runCompose(["fetch", ...a.ids]);
  }

  save(a: z.infer<typeof PathInput>) {
    const path = resolve(a.path);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, JSON.stringify(this.need(), null, 2) + "\n");
    return { saved: path };
  }

  open(a: z.infer<typeof PathInput>) {
    const doc = JSON.parse(readFileSync(resolve(a.path), "utf8")) as Doc;
    this.doc = {
      ...doc,
      beats_per_bar: doc.beats_per_bar ?? 4,
      key: doc.key ?? "",
      instruments: doc.instruments ?? [],
      layers: doc.layers ?? [],
      sections: doc.sections ?? [],
      parts: doc.parts ?? [],
    };
    return { ...this.summary(), ...this.validate() };
  }

  private tmp(tag: string, data: unknown): string {
    const path = this.scratch.replace(/\.json$/, `.${tag}.json`);
    writeFileSync(path, JSON.stringify(data));
    return path;
  }

  private keyOf(k: string | undefined): string {
    const key = k ?? this.doc?.key ?? "";
    if (!key.trim()) throw new Error("no key: pass key (e.g. 'D major') or set it with compose_new");
    return key;
  }

  /** Rows from a motif name, notation text, or explicit notes. */
  rowsFrom(src: z.infer<typeof NotesSource>): NoteRow[] {
    if (src.motif !== undefined) {
      const m = this.doc?.motifs?.[src.motif];
      if (!m) throw new Error(`no motif ${JSON.stringify(src.motif)} (state it with compose_motif)`);
      return m.map((n) => ({ ...n }));
    }
    const given: NoteInput[] = [...(src.notes ?? [])];
    if (src.text !== undefined) given.push(...parseNoteText(src.text, 0));
    if (given.length === 0) throw new Error("give a motif, text or notes");
    return flatten(given);
  }

  listen(a: z.infer<typeof ListenInput>): unknown {
    const args = ["listen", a.audio !== undefined ? resolve(a.audio) : this.flush()];
    if (a.reference) args.push("--reference", resolve(a.reference));
    if (a.spectrogram) args.push("--spectrogram", resolve(a.spectrogram));
    if (a.plan && a.audio === undefined) args.push("--plan", this.writePlan(a.plan));
    return runCompose(args);
  }

  compare(a: z.infer<typeof CompareInput>): unknown {
    return runCompose(["compare", resolve(a.a), resolve(a.b)]);
  }

  motif(a: z.infer<typeof MotifInput>) {
    const d = this.need();
    const rows = this.rowsFrom(a);
    const t0 = Math.min(...rows.map((n) => n.start));
    d.motifs = { ...(d.motifs ?? {}), [a.name]: shiftRows(rows, -t0) };
    const beats = Math.max(...rows.map((n) => n.start + n.length)) - t0;
    return { motif: a.name, notes: rows.length, beats, text: formatNoteText(d.motifs[a.name]!), ...this.validate() };
  }

  vary(a: z.infer<typeof VaryInput>) {
    const d = this.need();
    const src = d.motifs?.[a.motif];
    if (!src) throw new Error(`no motif ${JSON.stringify(a.motif)} (state it with compose_motif)`);
    const r = runCompose(["motif", this.tmp("motif", { key: this.keyOf(a.key), notes: src, ops: a.ops })]) as {
      key: string;
      notes: NoteRow[];
    };
    const out: Record<string, unknown> = { key: r.key, notes: r.notes.length, text: formatNoteText(r.notes) };
    if (a.saveAs) {
      d.motifs = { ...(d.motifs ?? {}), [a.saveAs]: r.notes };
      out["saved"] = a.saveAs;
    }
    if (a.write) {
      const w = a.write;
      out["part"] = this.putNotes(w.section, w.layer, w.instrument, shiftRows(r.notes, w.start ?? 0), w.mode ?? "append");
    }
    return { ...out, ...this.validate() };
  }

  /** Write voiced harmony (upper + bass rows) per HarmonyWrite. */
  private writeHarmony(w: z.infer<typeof HarmonyWrite>, r: { upper: NoteRow[]; bass: NoteRow[] }, span: number) {
    const rep = w.repeat ?? 1;
    const mode = w.mode ?? "replace";
    const tile = (rows: NoteRow[]) =>
      Array.from({ length: rep }, (_, k) => shiftRows(rows, (w.start ?? 0) + k * span)).flat();
    const out: Record<string, number> = {};
    if (w.upper) out["upper"] = this.putNotes(w.section, w.upper.layer, w.upper.instrument, tile(r.upper), mode);
    if (w.bass) out["bass"] = this.putNotes(w.section, w.bass.layer, w.bass.instrument, tile(r.bass), mode);
    return out;
  }

  progression(a: z.infer<typeof ProgressionInput>) {
    const every = a.every ?? 4;
    const req = {
      key: this.keyOf(a.key),
      chords: a.chords ?? [],
      preset: a.preset,
      every,
      beats_per_bar: this.doc?.beats_per_bar ?? 4,
      voicing: voicingArg(a.voicing),
      pattern: a.pattern,
      bass_pattern: a.bassPattern,
      subdivision: a.subdivision,
      velocity: a.velocity,
    };
    const r = runCompose(["progression", this.tmp("prog", req)]) as any;
    if (r.presets) return r;
    const out: Record<string, unknown> = {
      key: r.key,
      chords: r.chords,
      parallel_fifths_octaves: r.parallel_fifths_octaves,
      upperText: formatNoteText(r.upper),
      bassText: formatNoteText(r.bass),
    };
    if (a.write) {
      out["written"] = this.writeHarmony(a.write, r, r.chords.length * every);
      Object.assign(out, this.validate());
    }
    return out;
  }

  reharmonise(a: z.infer<typeof ReharmoniseInput>) {
    const melody = this.rowsFrom(a);
    const bpb = this.doc?.beats_per_bar ?? 4;
    const every = a.every ?? bpb;
    const req = {
      key: this.keyOf(a.key),
      melody,
      every,
      beats_per_bar: bpb,
      palette: a.palette ?? "plain",
      voicing: voicingArg(a.voicing),
      pattern: a.pattern,
    };
    const r = runCompose(["reharmonise", this.tmp("reharm", req)]) as any;
    const out: Record<string, unknown> = {
      key: r.key,
      chords: r.chords.map((c: any) => `${c.symbol} (${c.numeral}) @${c.start}`),
      voiced: r.chords,
      parallel_fifths_octaves: r.parallel_fifths_octaves,
    };
    if (a.write) {
      out["written"] = this.writeHarmony(a.write, r, r.chords.length * every);
      Object.assign(out, this.validate());
    }
    return out;
  }

  arrange(a: z.infer<typeof ArrangeInput>) {
    const d = this.need();
    let ensemble = a.ensemble ?? [];
    const added: string[] = [];
    if (a.musicMd) {
      const rows = (runCompose(["ensemble", resolve(a.musicMd)]) as {
        ensemble: { id: string; patch: string; role: string; layer: string }[];
      }).ensemble;
      for (const row of rows) {
        if (!d.instruments.some((i) => i.id === row.id)) {
          d.instruments.push({ id: row.id, patch: row.patch, gain_db: 0, reverb: 0.25 });
          added.push(`instrument ${row.id} (${row.patch})`);
        }
      }
      ensemble = [...ensemble, ...rows.map((r) => ({ instrument: r.id, role: r.role as any, layer: r.layer }))];
    }
    if (ensemble.length === 0) throw new Error("give ensemble, or musicMd with an Ensemble table");
    const dflt: Record<string, number> = { calm: 1, bed: 1, lively: 3, ...(a.layerIntensity ?? {}) };
    for (const m of ensemble) {
      if (!d.layers.some((l) => l.id === m.layer)) {
        d.layers.push({ id: m.layer, min_intensity: dflt[m.layer] ?? 2, gain_db: 0 });
        added.push(`layer ${m.layer} (intensity ${dflt[m.layer] ?? 2}+)`);
      }
    }
    const req = {
      section: a.section,
      key: a.key,
      melody: a.melody ? this.rowsFrom(a.melody) : [],
      fill: a.fill ?? true,
      chords: a.chords ?? [],
      every: a.every,
      palette: a.palette ?? "plain",
      ensemble,
    };
    const r = runCompose(["arrange", this.flush(), this.tmp("arrange", req)]) as {
      key: string;
      chords: { symbol: string; numeral?: string; start: number }[];
      parts: { section: string; layer: string; instrument: string; notes: NoteRow[] }[];
      notes: string[];
    };
    for (const p of r.parts) this.putNotes(p.section, p.layer, p.instrument, p.notes, "replace");
    return {
      key: r.key,
      chords: r.chords.map((c) => `${c.symbol} (${c.numeral ?? "?"}) @${c.start}`),
      parts: r.parts.map((p) => ({ layer: p.layer, instrument: p.instrument, notes: p.notes.length })),
      added,
      notes: r.notes,
      ...this.validate(),
    };
  }

  setLayerFx(a: z.infer<typeof LayerFxInput>) {
    const d = this.need();
    const l = d.layers.find((x) => x.id === a.layer);
    if (!l) throw new Error(`no layer ${JSON.stringify(a.layer)} (add it first)`);
    if (a.eq !== undefined) {
      const e = a.eq;
      const eq: Eq = {};
      if (e.lowCutHz !== undefined) eq.low_cut_hz = e.lowCutHz;
      if (e.highCutHz !== undefined) eq.high_cut_hz = e.highCutHz;
      if (e.lowDb !== undefined) eq.low_db = e.lowDb;
      if (e.midDb !== undefined) eq.mid_db = e.midDb;
      if (e.midHz !== undefined) eq.mid_hz = e.midHz;
      if (e.highDb !== undefined) eq.high_db = e.highDb;
      if (Object.keys(eq).length === 0) delete l.eq;
      else l.eq = eq;
    }
    if (a.reverb !== undefined) l.reverb = a.reverb;
    if (a.room !== undefined) {
      l.room = {
        decay_secs: a.room.decaySecs ?? d.room?.decay_secs ?? 2.4,
        predelay_ms: a.room.predelayMs ?? d.room?.predelay_ms ?? 22,
        damping: a.room.damping ?? d.room?.damping ?? 0.45,
      };
    }
    return { layer: l, ...this.validate() };
  }

  setMaster(a: z.infer<typeof MasterInput>) {
    const d = this.need();
    if (a.off) delete d.master;
    else d.master = { lufs: a.lufs ?? d.master?.lufs ?? -16, ceiling_dbtp: a.ceilingDbtp ?? d.master?.ceiling_dbtp ?? -1 };
    return { master: d.master ?? null, ...this.validate() };
  }

  masterAudio(a: z.infer<typeof MasterAudioInput>): unknown {
    const args = ["master", resolve(a.in), "--out", resolve(a.out)];
    if (a.lufs !== undefined) args.push("--lufs", String(a.lufs));
    if (a.ceilingDbtp !== undefined) args.push("--ceiling", String(a.ceilingDbtp));
    if (a.loop) args.push("--loop");
    return runCompose(args);
  }

  transcribe(a: z.infer<typeof TranscribeInput>) {
    const out = resolve(a.out ?? this.scratch.replace(/\.json$/, ".transcribed.json"));
    const args = ["transcribe", resolve(a.audio), "--out", out];
    const opt = (flag: string, v: string | number | undefined) => {
      if (v !== undefined) args.push(flag, String(v));
    };
    opt("--midi", a.midi && resolve(a.midi));
    opt("--tempo", a.tempo);
    opt("--key", a.key);
    opt("--bars", a.bars);
    opt("--start-bar", a.startBar);
    opt("--from", a.from);
    opt("--to", a.to);
    opt("--name", a.name);
    const r = runCompose(args) as Record<string, unknown>;
    const opened = this.open({ path: out });
    return { ...r, composition: opened };
  }

  openInDaw(a: z.infer<typeof DawInput>): unknown {
    const r = runCompose(["daw", this.flush(), "--out", resolve(a.dir)]) as Record<string, unknown>;
    return { ...r, open: `in the DAW: type this folder into the project path and press Open (or palette: Composition: Open in the DAW): ${resolve(a.dir)}` };
  }

  private writePlan(plan: z.infer<typeof PlanStepSchema>[]): string {
    const path = this.scratch.replace(/\.json$/, ".plan.json");
    writeFileSync(path, JSON.stringify(plan));
    return path;
  }
}

/** Tool rows: name, description, input shape, handler. */
export function composeTools(store: ComposeStore) {
  return [
    {
      name: "compose_new",
      description:
        "Start a new game-music composition (replaces the current one). Then add instruments, layers, sections and notes.",
      input: ComposeNewInput,
      run: (a: any) => store.newDoc(ComposeNewInput.parse(a)),
    },
    {
      name: "compose_patches",
      description:
        "List every instrument you can name in compose_add_instrument: sampled orchestral instruments (strings sustain and pizzicato, woodwinds, brass, piano, harp, kalimba, mallets, wordless choir, orchestral and hand drums; CC0/MIT libraries) and the built-in synth patches. Each row: id, family, what it sounds good for, comfortable MIDI range, default pan, licence, and whether its samples are downloaded yet. Filter with family.",
      input: PatchesInput,
      run: (a: any) => store.patches(PatchesInput.parse(a ?? {})),
    },
    {
      name: "compose_instrument",
      description:
        "Details of one instrument: keys that have recordings, velocity layers, round robins, licence; for drums, which MIDI key plays which hit (e.g. 60 = high bongo). Downloads the samples if needed.",
      input: InstrumentIdInput,
      run: (a: any) => store.instrument(InstrumentIdInput.parse(a)),
    },
    {
      name: "compose_fetch_instruments",
      description:
        'Download sampled instruments into the local cache now (previews and exports also fetch on demand). ids: e.g. ["violins","flute"] or ["all"] (~1.8 GB). Every file is checked against its recorded SHA-256.',
      input: FetchInput,
      run: (a: any) => store.fetch(FetchInput.parse(a)),
    },
    {
      name: "compose_set_room",
      description:
        "Set the hall every instrument shares (reverb sends go here): decaySecs (RT60), predelayMs, damping. Loops stay seamless: the tail folds onto the loop start.",
      input: RoomInput,
      run: (a: any) => store.setRoom(RoomInput.parse(a)),
    },
    {
      name: "compose_add_instrument",
      description:
        "Add or replace an instrument: an id bound to a patch (see compose_patches), with gainDb, reverb send (0-1) and pan (-1..1; default its orchestral seat).",
      input: InstrumentInput,
      run: (a: any) => store.addInstrument(InstrumentInput.parse(a)),
    },
    {
      name: "compose_add_layer",
      description:
        "Add or replace an adaptive layer. minIntensity 1 = always on; higher layers fade in when the game raises intensity (1-5).",
      input: LayerInput,
      run: (a: any) => store.addLayer(LayerInput.parse(a)),
    },
    {
      name: "compose_add_section",
      description:
        "Add or replace a section: intro (plays once, hands off to next), loop (repeats until the game switches), outro (plays once, then silence).",
      input: SectionInput,
      run: (a: any) => store.addSection(SectionInput.parse(a)),
    },
    {
      name: "compose_write_notes",
      description:
        "Write notes for one section+layer+instrument (replace by default). Easiest: text like 'A4/1.5 G4/0.5 F4 E4/0.5@0.6 D3+F3+A3/4 r/2' (LEN beats, sticky; chords with +; r = rest) with start and repeat. Or explicit notes (start/length in beats from the section start; pitch D4 or MIDI; pitches = chord).",
      input: WriteNotesInput,
      run: (a: any) => store.writeNotes(WriteNotesInput.parse(a)),
    },
    {
      name: "compose_get",
      description: "Return the composition summary, the full document, and validation errors.",
      input: Empty,
      run: () => store.get(),
    },
    {
      name: "compose_preview",
      description:
        "Render a stereo preview mix to .wav or .mp3 following a plan of {section, bars?, intensity?} steps; returns loudness per step and per layer, each instrument's level while it plays (instrument_rms_dbfs, for balancing), loop-seam checks and warnings such as notes no recording covers (you cannot listen, so read these).",
      input: PreviewInput,
      run: (a: any) => store.preview(PreviewInput.parse(a)),
    },
    {
      name: "compose_export_audioforge",
      description:
        "Export the audioforge interactive-music package (score.json, mix.json, events, 48 kHz stereo stems, preview scene) to dir; returns loop-seam checks per loop stem.",
      input: ExportAudioforgeInput,
      run: (a: any) => store.exportAudioforge(ExportAudioforgeInput.parse(a)),
    },
    {
      name: "compose_save",
      description: "Save the composition document (JSON) to a path.",
      input: PathInput,
      run: (a: any) => store.save(PathInput.parse(a)),
    },
    {
      name: "compose_open",
      description: "Open a saved composition document (or an export's composition.json).",
      input: PathInput,
      run: (a: any) => store.open(PathInput.parse(a)),
    },
    {
      name: "compose_listen",
      description:
        "Your ears. Renders the current composition (or listens to an audio file) and says what it sounds like: tempo, key, density, brightness, loudness (LUFS, a per-second curve: does it build?), stereo width; per section at low and full intensity; how much each layer adds; how alike the sections are; register clashes between parts; notes outside the key or an instrument's range. With reference: how far it is from another track, aspect by aspect (harmony, timbre, rhythm, brightness, loudness) with plain differences. spectrogram writes a picture you can look at.",
      input: ListenInput,
      run: (a: any) => store.listen(ListenInput.parse(a)),
    },
    {
      name: "compose_compare",
      description: "Compare two audio files aspect by aspect (harmony, timbre, rhythm, brightness, loudness) and list how B differs from A.",
      input: CompareInput,
      run: (a: any) => store.compare(CompareInput.parse(a)),
    },
    {
      name: "compose_motif",
      description:
        "State a motif (a short idea: text like 'D5/0.5 F#5 A5/1 G5/2', explicit notes, or another motif) under a name; it is kept in the composition for compose_vary, compose_reharmonise and compose_arrange.",
      input: MotifInput,
      run: (a: any) => store.motif(MotifInput.parse(a)),
    },
    {
      name: "compose_vary",
      description:
        "Vary a motif in the key: transpose by scale steps, invert (mirror intervals diatonically), retrograde, augment/diminish (factor), sequence (repeat stepping), fragment, displace, octave, dynamics; ops apply in order. Save the result as a new motif and/or write it into a part at a beat.",
      input: VaryInput,
      run: (a: any) => store.vary(VaryInput.parse(a)),
    },
    {
      name: "compose_progression",
      description:
        "Chords in a key and mode with smooth voice leading (common tones kept, no parallel fifths/octaves): numerals (I vi IV V, IVmaj7, V7/V, bVII, IV/3) or symbols, or a named preset (omit both to list presets like royal_road, journey, mixolydian_sail). Returns the voicings and the notes as text; write puts the upper voices and the bass line into parts (pattern: block, arp_up, arp_up_down, broken, pulse).",
      input: ProgressionInput,
      run: (a: any) => store.progression(ProgressionInput.parse(a)),
    },
    {
      name: "compose_reharmonise",
      description:
        "Find chords for a melody (motif, text or notes) in a key: one per `every` beats, chosen for the melody's strong-beat notes and good chord motion; palette colour adds sevenths, borrowed and secondary chords. Returns the progression voiced; write places it like compose_progression.",
      input: ReharmoniseInput,
      run: (a: any) => store.reharmonise(ReharmoniseInput.parse(a)),
    },
    {
      name: "compose_arrange",
      description:
        "Arrange a tune and chords for an ensemble in one section: each instrument gets a role (melody, double, sparkle, counter, bass, pad, arp, pulse, broken, drone, percussion) on a layer, inside its comfortable range. Take the ensemble inline or from a game's design/music.md '## Ensemble' table (Id | Patch | Role | Layer); missing instruments and layers are added (calm 1, lively 3). Omit chords to harmonise the tune.",
      input: ArrangeInput,
      run: (a: any) => store.arrange(ArrangeInput.parse(a)),
    },
    {
      name: "compose_set_layer_fx",
      description:
        "Shape one layer's sound: EQ (lowCutHz to clear mud, highCutHz to darken or push back, low/mid/high shelves and a bell), its reverb amount (scales every send on the layer) and optionally its own hall. Loops stay seamless.",
      input: LayerFxInput,
      run: (a: any) => store.setLayerFx(LayerFxInput.parse(a)),
    },
    {
      name: "compose_set_master",
      description:
        "Master the composition: previews get the full chain (DC, rumble high-pass, loudness to lufs, true-peak limiter at ceilingDbtp), exported stems one shared gain so the loudest full-intensity loop sits at the target. -18 LUFS suits an in-game bed, -16 a standalone listen.",
      input: MasterInput,
      run: (a: any) => store.setMaster(MasterInput.parse(a)),
    },
    {
      name: "compose_master_audio",
      description:
        "Master any 48 kHz audio file (a preview, an ACE-Step take) to a loudness target under a true-peak ceiling; loop keeps a seamless loop seamless. Returns LUFS and peaks before and after.",
      input: MasterAudioInput,
      run: (a: any) => store.masterAudio(MasterAudioInput.parse(a)),
    },
    {
      name: "compose_transcribe",
      description:
        "Turn an AI take (ACE-Step) or any recording into notes and keep the idea as an editable composition: basic-pitch (installed locally on first use) transcribes it, the tempo is heard, notes are quantised, the melody, bass and one chord per bar are pulled out and arranged on calm and lively layers. The result becomes the current composition (the melody is kept as motif transcribed_melody). Choose the stretch with startBar/bars or from/to seconds.",
      input: TranscribeInput,
      run: (a: any) => store.transcribe(TranscribeInput.parse(a)),
    },
    {
      name: "compose_open_in_daw",
      description:
        "Write the composition as a DAW project (timeline + piano roll): one audio track per layer with the rendered stems (play sounds the real orchestration) and one muted MIDI track per layer and instrument with the notes, sections laid end to end. Open the folder in the DAW (project path + Open).",
      input: DawInput,
      run: (a: any) => store.openInDaw(DawInput.parse(a)),
    },
  ] as const;
}

export const COMPOSE_TOOL_NAMES = [
  "compose_new",
  "compose_patches",
  "compose_instrument",
  "compose_fetch_instruments",
  "compose_set_room",
  "compose_add_instrument",
  "compose_add_layer",
  "compose_add_section",
  "compose_write_notes",
  "compose_get",
  "compose_preview",
  "compose_export_audioforge",
  "compose_save",
  "compose_open",
  "compose_listen",
  "compose_compare",
  "compose_motif",
  "compose_vary",
  "compose_progression",
  "compose_reharmonise",
  "compose_arrange",
  "compose_set_layer_fx",
  "compose_set_master",
  "compose_master_audio",
  "compose_transcribe",
  "compose_open_in_daw",
] as const;
