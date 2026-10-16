#!/usr/bin/env bun
/**
 * An agent session, kept as a script: an island-travel theme composed
 * only through the MCP compose_* tools (motif, variations, progression,
 * re-harmonising, arranging to an ensemble, layer EQ, mastering, ears),
 * on two adaptive layers: `calm` (always) and `lively` (intensity 3+).
 *
 *   bun mcp/scripts/island-travel-session.ts <out dir> [reference audio] [lufs]
 *
 * Writes the composition, a preview (calm pass, then lively pass), the
 * audioforge package, a DAW project, and what the ears heard, including
 * how far the theme is from the reference track.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { InMemoryBackend } from "../src/backend";
import { ComposeStore } from "../src/compose";
import { GameAudioStore } from "../src/gameaudio";
import { createServer } from "../src/index";

const out = resolve(process.argv[2] ?? "island_travel_out");
const reference = process.argv[3] ? resolve(process.argv[3]) : undefined;
const lufs = Number(process.argv[4] ?? -18);
mkdirSync(out, { recursive: true });

const [ct, st] = InMemoryTransport.createLinkedPair();
await createServer(new InMemoryBackend(), new GameAudioStore(), new ComposeStore()).connect(st);
const client = new Client({ name: "island-travel-session", version: "0" });
await client.connect(ct);
const log: { tool: string; args: unknown; result: unknown }[] = [];
async function call(tool: string, args: Record<string, unknown> = {}): Promise<any> {
  const r = (await client.callTool({ name: tool, arguments: args })) as any;
  const text = r.content?.[0]?.text ?? "";
  if (r.isError) throw new Error(`${tool}: ${text}`);
  const result = JSON.parse(text);
  log.push({ tool, args, result });
  return result;
}

// --- The frame: D major at a walking 104 BPM, one 24-bar loop (A A' B).
await call("compose_new", { name: "island_travel", tempo: 104, key: "D major" });
await call("compose_set_room", { decaySecs: 2.2, predelayMs: 24, damping: 0.5 });
await call("compose_add_section", { id: "travel", role: "loop", bars: 24 });

// --- The idea: a skipping one-bar motif that falls to the fifth below
// and climbs back. Its sequence (twice, a step down each time) makes
// bars 1-3, the way a storybook tune walks down the scale.
await call("compose_motif", { name: "hook", text: "F#5/0.75 E5/0.25 D5/0.5 A4/0.5 D5/1 F#5/1" });
const walk = await call("compose_vary", { motif: "hook", ops: [{ op: "sequence", times: 2, steps: -1 }], saveAs: "walk" });
// B section: the hook turned upside down (mirrored in the key around its
// first note) and played twice as slow: the same idea, now soaring.
const soar = await call("compose_vary", {
  motif: "hook",
  ops: [{ op: "invert" }, { op: "augment", factor: 2 }],
  saveAs: "soar",
});
// ... and that soaring line again, a third lower, answering it.
const answer = await call("compose_vary", { motif: "soar", ops: [{ op: "transpose", steps: -2 }], saveAs: "answer" });

const bar4 = "B4/0.75 A4/0.25 G4/0.5 B4/0.5 D5/2";
const climb = "A4/0.5 D5/0.5 F#5/1 A5/1.5 F#5/0.5 G5/0.75 F#5/0.25 E5/0.5 D5/0.5 B4/1 D5/1";
const halfCadence = "E5/0.75 F#5/0.25 G5/1 B5/1 G5/1 A5/1.5 G5/0.5 F#5/0.5 E5/0.5 C#5/1";
const fullCadence = "E5/0.5 F#5/0.5 G5/1 E5/1 C#5/1 D5/2 A4/0.5 B4/0.5 C#5/0.5 E5/0.5";
const homeward = "B5/1 A5/0.5 G5/0.5 F#5/1 E5/1 F#5/1 E5/0.5 D5/0.5 A4/2 G5/0.75 F#5/0.25 E5/0.5 D5/0.5 E5/1 G5/1 A5/2 G5/0.5 F#5/0.5 E5/0.5 C#5/0.5";
const A = `${walk.text} ${bar4} ${climb} ${halfCadence}`;
const A2 = `${walk.text} ${bar4} ${climb} ${fullCadence}`;
const B = `${soar.text} ${answer.text} ${homeward}`;
await call("compose_motif", { name: "tune", text: `${A} ${A2} ${B}` });

// --- Harmony. A and A' walk the bass down like Pachelbel; the B section's
// chords are found for its tune by the re-harmoniser, in colour.
const chordsA = ["I", "V/3", "vi", "IV", "I/3", "IV", "ii7", "V7"];
const chordsA2 = ["I", "V/3", "vi", "IV", "I/3", "IV", "V7", "I"];
await call("compose_motif", { name: "b_tune", text: B });
const reharm = await call("compose_reharmonise", { motif: "b_tune", every: 4, palette: "colour" });
const chordsB: string[] = reharm.voiced.map((c: any) => c.numeral ?? c.symbol);
// The last B bar turns home: the dominant, so the loop lands on I.
chordsB[chordsB.length - 1] = "V7";
const chords = [...chordsA, ...chordsA2, ...chordsB];

// --- Arrange it for the ensemble in the design doc (instruments, roles, layers).
const arranged = await call("compose_arrange", {
  section: "travel",
  melody: { motif: "tune" },
  fill: false,
  chords,
  every: 4,
  musicMd: resolve(import.meta.dir, "../../examples/island_travel/music.md"),
  layerIntensity: { calm: 1, lively: 3 },
});

// --- Sound: the lively layer sits above the calm bed (no low mud), a
// little drier so the rhythm stays crisp; the calm bed a touch warmer.
await call("compose_set_layer_fx", { layer: "lively", eq: { lowCutHz: 220, highDb: 1.5 }, reverb: 0.8 });
await call("compose_set_layer_fx", { layer: "calm", eq: { lowDb: 1, highCutHz: 14000 } });
// Seats on the stage and balance.
await call("compose_add_instrument", { id: "clarinet", patch: "clarinet", reverb: 0.3, pan: -0.15 });
await call("compose_add_instrument", { id: "harp", patch: "harp", reverb: 0.35, pan: 0.4, gainDb: -2 });
await call("compose_add_instrument", { id: "violas", patch: "violas", reverb: 0.45, pan: -0.25, gainDb: -6 });
await call("compose_add_instrument", { id: "cello", patch: "cellos_pizz", reverb: 0.2, pan: 0.25 });
await call("compose_add_instrument", { id: "flute", patch: "flute", reverb: 0.3, pan: 0.2 });
await call("compose_add_instrument", { id: "glock", patch: "glockenspiel", reverb: 0.4, pan: 0.3, gainDb: -4 });
await call("compose_add_instrument", { id: "horn", patch: "horn", reverb: 0.4, pan: -0.3, gainDb: -3 });
await call("compose_add_instrument", { id: "pizz", patch: "violins_pizz", reverb: 0.25, pan: -0.45, gainDb: -3 });
await call("compose_add_instrument", { id: "bongos", patch: "bongos", reverb: 0.15, pan: 0.15 });
await call("compose_add_instrument", { id: "shaker", patch: "shaker", reverb: 0.15, pan: -0.3, gainDb: -1 });
await call("compose_set_master", { lufs, ceilingDbtp: -1 });

// --- Ears: what it sounds like, layer by layer, and against the reference.
const ears = await call("compose_listen", {
  plan: [
    { section: "travel", intensity: 1 },
    { section: "travel", intensity: 4 },
  ],
  spectrogram: join(out, "spectrogram.jpg"),
  ...(reference ? { reference } : {}),
});
const preview = await call("compose_preview", {
  out: join(out, "preview.mp3"),
  plan: [
    { section: "travel", intensity: 1 },
    { section: "travel", intensity: 4 },
  ],
});
await call("compose_preview", { out: join(out, "calm.mp3"), plan: [{ section: "travel", intensity: 1 }] });
await call("compose_preview", { out: join(out, "lively.mp3"), plan: [{ section: "travel", intensity: 4 }] });
await call("compose_save", { path: join(out, "island_travel.composition.json") });
await call("compose_export_audioforge", { dir: join(out, "audioforge") });
const daw = await call("compose_open_in_daw", { dir: join(out, `daw-project-${Date.now()}`) });

writeFileSync(join(out, "session-log.json"), JSON.stringify(log, null, 2));
writeFileSync(
  join(out, "heard.json"),
  JSON.stringify({ ears, master: preview.master, warnings: preview.stats.warnings, daw }, null, 2),
);
console.log(
  JSON.stringify(
    {
      chords,
      reharmonised_b: reharm.chords,
      arranged: arranged.parts,
      arrange_notes: arranged.notes,
      summary: ears.composition.summary,
      preview: ears.preview.summary,
      reference: ears.reference && {
        summary: ears.reference.summary,
        overall: ears.reference.comparison.overall,
        differences: ears.reference.comparison.differences,
      },
      master: preview.master,
      warnings: preview.stats.warnings,
      daw: daw.dir,
    },
    null,
    2,
  ),
);
await client.close();
