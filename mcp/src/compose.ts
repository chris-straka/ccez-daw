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

/**
 * MIDI number for a pitch, mirroring core `compose::parse_pitch` /
 * `de_pitch` exactly (C4 = 60, `#`/`b` accidentals, octaves -1..=9,
 * integers 0..=127), so a pitch the Rust CLI cannot read is rejected here
 * before it ever reaches the document.
 */
export function midiPitch(p: string | number): number {
  if (typeof p === "number") {
    if (!Number.isInteger(p) || p < 0 || p > 127) throw new Error(`pitch ${p} is outside MIDI 0..=127`);
    return p;
  }
  const t = String(p).trim();
  const base = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 }[t.charAt(0).toUpperCase()];
  if (base === undefined) throw new Error(`pitch ${JSON.stringify(p)}: want a note name like D4 or F#3`);
  let rest = t.slice(1);
  let acc = 0;
  if (rest.startsWith("#")) [acc, rest] = [1, rest.slice(1)];
  else if (rest.startsWith("b")) [acc, rest] = [-1, rest.slice(1)];
  if (!/^[+-]?\d+$/.test(rest)) throw new Error(`pitch ${JSON.stringify(p)}: missing octave (want e.g. D4)`);
  const n = (Number(rest) + 1) * 12 + base + acc;
  if (n < 0 || n > 127) throw new Error(`pitch ${JSON.stringify(p)} is outside MIDI 0..=127`);
  return n;
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
export const Empty = z.object({});

type Doc = {
  schema_version: number;
  name: string;
  tempo: number;
  beats_per_bar: number;
  key: string;
  instruments: { id: string; patch: string; gain_db: number; reverb: number; pan?: number }[];
  room?: { decay_secs: number; predelay_ms: number; damping: number };
  layers: { id: string; min_intensity: number; gain_db: number }[];
  sections: { id: string; role: string; bars: number; next?: string }[];
  parts: {
    section: string;
    layer: string;
    instrument: string;
    notes: { pitch: string | number; start: number; length: number; velocity?: number }[];
  }[];
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

/** One composition per server, persisted to a scratch file the CLI reads. */
export class ComposeStore {
  doc: Doc | null = null;
  readonly scratch = join(tmpdir(), `ccez-compose-${process.pid}-${Math.random().toString(36).slice(2, 8)}.json`);

  private need(): Doc {
    if (!this.doc) throw new Error("no composition: call compose_new (or compose_open) first");
    return this.doc;
  }

  /**
   * Run one mutation transactionally: snapshot the document, apply `f`
   * (which mutates and validates), and restore the snapshot if anything
   * throws, so one bad call (e.g. something the Rust CLI cannot load)
   * can never leave the session holding an unreadable document.
   */
  private mutate<T>(f: (d: Doc) => T): T {
    const d = this.need();
    const snapshot = structuredClone(d);
    try {
      return f(d);
    } catch (e) {
      this.doc = snapshot;
      throw e;
    }
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
    return this.mutate((d) => {
      const inst: Doc["instruments"][number] = {
        id: a.id,
        patch: a.patch,
        gain_db: a.gainDb ?? 0,
        reverb: a.reverb ?? 0.25,
      };
      if (a.pan !== undefined) inst.pan = a.pan;
      const r = upsert(d.instruments, inst);
      return { [r]: a.id, ...this.validate() };
    });
  }

  setRoom(a: z.infer<typeof RoomInput>) {
    return this.mutate((d) => {
      d.room = {
        decay_secs: a.decaySecs ?? d.room?.decay_secs ?? 2.4,
        predelay_ms: a.predelayMs ?? d.room?.predelay_ms ?? 22,
        damping: a.damping ?? d.room?.damping ?? 0.45,
      };
      return { room: d.room, ...this.validate() };
    });
  }

  addLayer(a: z.infer<typeof LayerInput>) {
    return this.mutate((d) => {
      const r = upsert(d.layers, { id: a.id, min_intensity: a.minIntensity ?? 1, gain_db: a.gainDb ?? 0 });
      return { [r]: a.id, ...this.validate() };
    });
  }

  addSection(a: z.infer<typeof SectionInput>) {
    return this.mutate((d) => {
      const s: Doc["sections"][number] = { id: a.id, role: a.role, bars: a.bars };
      if (a.next !== undefined) s.next = a.next;
      const r = upsert(d.sections, s);
      const beats = a.bars * d.beats_per_bar;
      return { [r]: a.id, beats, ...this.validate() };
    });
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
    // Every pitch (explicit or from text) is checked against core's rules
    // here, before the document is touched.
    const notes = given.flatMap((n) => {
      const pitches = n.pitches ?? (n.pitch !== undefined ? [n.pitch] : []);
      if (pitches.length === 0) throw new Error(`note at beat ${n.start}: give pitch or pitches`);
      return pitches.map((p) => {
        midiPitch(p);
        const out: Doc["parts"][number]["notes"][number] = { pitch: p, start: n.start, length: n.length };
        if (n.velocity !== undefined) out.velocity = n.velocity;
        return out;
      });
    });
    return this.mutate((d) => {
      let part = d.parts.find((p) => p.section === a.section && p.layer === a.layer && p.instrument === a.instrument);
      if (!part) {
        part = { section: a.section, layer: a.layer, instrument: a.instrument, notes: [] };
        d.parts.push(part);
      }
      if ((a.mode ?? "replace") === "replace") part.notes = notes;
      else part.notes.push(...notes);
      return { notes: part.notes.length, written: notes.length, ...this.validate() };
    });
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
    const previous = this.doc;
    this.doc = {
      ...doc,
      beats_per_bar: doc.beats_per_bar ?? 4,
      key: doc.key ?? "",
      instruments: doc.instruments ?? [],
      layers: doc.layers ?? [],
      sections: doc.sections ?? [],
      parts: doc.parts ?? [],
    };
    // A file the CLI cannot load must not replace the working document.
    try {
      return { ...this.summary(), ...this.validate() };
    } catch (e) {
      this.doc = previous;
      throw e;
    }
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
] as const;
