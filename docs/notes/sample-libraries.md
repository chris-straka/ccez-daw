# Sampled instruments for the composer (2026-10-15)

The compose path (`core/src/compose`) plays real recordings next to its
eight synth patches. Agents name them like patches (`"patch": "violins"`);
`ccez-compose patches` / MCP `compose_patches` lists them with what each
is good for, and `compose_instrument` shows velocity layers, round robins
and, for drums, which key plays which hit.

## Libraries and licences (checked 2026-10-15)

Only libraries whose licence lets a commercial game ship music made with
them (and the samples baked into its stems) without binding the game:

| Source | What we use | Licence | Where the licence was read |
|---|---|---|---|
| VS Chamber Orchestra 2: Community Edition (Versilian Studios), `sgossner/VSCO-2-CE` branch `SFZ` @ `6dd651d` | strings (sustain, pizzicato, tremolo, solo violin), woodwinds, brass, harp, xylophone, tubular bells, timpani | CC0 1.0 (public domain dedication) | repo `LICENSE`; GitHub reports `CC0-1.0` |
| Versilian Community Sample Library, `sgossner/VCSL` branch `sfz` @ `dfcf4a4` | Kawai grand piano, kalimba, glockenspiel, marimba, vibraphone, bass drum, snare, cymbal, triangle, tambourine, shaker, bongos, congas, frame drum, cajon, darbuka | CC0 1.0 | repo `LICENSE`; GitHub reports `CC0-1.0` |
| FluidR3_GM SoundFont (Frank Wen), Debian `fluid-soundfont-gm` 3.1-5.3 | wordless choir: Choir Aahs (GM 52), Voice Oohs (GM 53) | MIT | the package's `debian/copyright` quotes the upstream README: "I hereby release Fluid under the MIT license" |

CC0 asks nothing. MIT asks that the copyright and permission notice go
with "copies or substantial portions": a game that ships choir stems keeps
the FluidR3 notice in its credits/licence file. The export manifest lists
each instrument's source and licence (`manifest.json` `instruments`) so a
game can build that page.

Not used: anything non-commercial or share-alike, e.g. Sonatina Symphonic
Orchestra (Creative Commons Sampling Plus 1.0), libraries that mix in
Sonatina samples, and Freesound packs under CC BY-NC.

## Download on demand, checked by checksum

Sample files never go in git. `core/assets/sample-lock.json` records,
for every file each instrument needs (1,339 files, 1.79 GB in all), its
size and SHA-256; the source URLs are the pinned commits above
(`raw.githubusercontent.com/sgossner/<repo>/<commit>/<path>`) and, for
FluidR3, the Debian package (`deb.debian.org` pool, then
`snapshot.debian.org`), whose own SHA-256 is pinned in
`core/src/compose/library.rs`.

- Cache: `$CCEZ_SAMPLES_DIR`, else `$XDG_CACHE_HOME/ccez-daw/samples`,
  else `~/.cache/ccez-daw/samples`.
- A preview or export that uses an instrument downloads what is missing
  first (`curl`, 8 at a time), checks every file's SHA-256 and refuses a
  mismatch. Offline with nothing cached, the render fails with the reason;
  it never substitutes a synth or silence.
- `ccez-compose fetch <id>... | all` (MCP `compose_fetch_instruments`)
  downloads ahead of time; `ccez-compose samples verify` re-hashes the
  cache; `ccez-compose samples lock --out core/assets/sample-lock.json`
  rebuilds the lock after the catalog changes (maintainers only).
- FluidR3 extraction needs `tar` and `xz` (the `.deb` is an `ar` archive
  read by our own code).

## How it plays

`sfz.rs` reads the SFZ subset these libraries use (headers cascade,
`default_path`, key/velocity ranges, `seq_length`/`seq_position` round
robins, `lorand`/`hirand`, velocity cross-fades, tune, offset, loops, amp
envelope; release-trigger regions are skipped). `sf2.rs` reads one SF2
preset (zones, generators, loops). `sampler.rs` picks the regions for a
note, skips each recording's leading silence so notes land on the beat,
repitches with 4-point Hermite interpolation, applies the envelope, loops
sustained recordings with a cross-fade when a note outlasts them, and
places the result on the stereo stage. Every render is deterministic.

Pan law (stage `pan` and SFZ/SF2 region `pan`): the synths' constant-power
law, `L = √2·cos θ`, `R = √2·sin θ` with `θ = (pan + 1)·π/4`, so centre is
unity on both sides and hard left is +3 dB left, silent right. Sampled
voices get the same two gains applied per channel: a mono recording
(both channels identical) pans exactly like a synth, and a stereo
recording gets a constant-power *balance* (each side keeps its own
content; hard left keeps only the left channel). One law for everything,
so moving an instrument between a synth and a sampled patch keeps its
place and level.

Each catalog entry carries a level calibration (`gain_db`, measured with
`ccez-compose samples calibrate`: a mezzo note's first-second rms, sustains
to about -22 dBFS, plucks and hits a little lower, peaks kept under
-3 dBFS) and an orchestral seat (`pan`), both overridable per instrument.

## Catalog

| id | family | sounds like / good for | layers × RR | source |
|---|---|---|---|---|
| `violins` | strings | violin section, sustained with vibrato: warm, singing (melodies, high pads, swells) | 2 × 1 | VSCO 2 CE |
| `violas` | strings | viola section, sustained: darker, reedy middle (inner harmony, counter-lines) | 2 × 1 | VSCO 2 CE |
| `cellos` | strings | cello section, sustained: rich, warm low-mid (bass lines, tender tunes, pads) | 3 × 1 | VSCO 2 CE |
| `basses` | strings | solo contrabass, sustained without vibrato: deep, soft floor (roots, pedal tones) | 2 × 1 | VSCO 2 CE |
| `solo_violin` | strings | solo violin with vibrato: intimate, expressive (lead melody, lonely moments) | 2 × 1 | VSCO 2 CE |
| `violins_trem` | strings | violin section tremolo: shimmering, nervous (tension beds, suspense) | 2 × 1 | VSCO 2 CE |
| `violins_pizz` | strings | violin section pizzicato: light, bouncy plucks (ostinatos, playful, sneaking) | 2 × 1 | VSCO 2 CE |
| `violas_pizz` | strings | viola section pizzicato: rounder mid plucks (off-beat chords, ostinatos) | 2 × 1 | VSCO 2 CE |
| `cellos_pizz` | strings | cello section pizzicato: woody, resonant (walking bass, bouncing low lines) | 2 × 2 | VSCO 2 CE |
| `basses_pizz` | strings | contrabass pizzicato: round, deep pluck (bass lines, jazz-ish walks) | 2 × 1 | VSCO 2 CE |
| `flute` | woodwinds | flute with vibrato: airy, bright, gentle (lead melody, birdlike lines) | 2 × 1 | VSCO 2 CE |
| `flute_stac` | woodwinds | flute staccato: short, breathy dots (playful figures, sparkle) | 4 × 1 | VSCO 2 CE |
| `piccolo` | woodwinds | piccolo: piercing, bright, very high (festive top lines, doubling the tune) | 1 × 1 | VSCO 2 CE |
| `oboe` | woodwinds | oboe with vibrato: plaintive, pastoral, nasal (melody, folk, longing) | 2 × 1 | VSCO 2 CE |
| `oboe_stac` | woodwinds | oboe staccato: pointed, comic (quirky figures) | 3 × 1 | VSCO 2 CE |
| `clarinet` | woodwinds | clarinet, long notes: round, mellow, woody (melody, warm inner lines) | 3 × 1 | VSCO 2 CE |
| `clarinet_stac` | woodwinds | clarinet staccato: soft, bubbly (light rhythms, arpeggios) | 3 × 1 | VSCO 2 CE |
| `bassoon` | woodwinds | bassoon: reedy, gentle low voice, a bit comic (bass lines, grumpy tunes) | 2 × 1 | VSCO 2 CE |
| `horn` | brass | French horn: noble, warm, heroic at forte (themes, warm pads, adventure) | 4 × 1 | VSCO 2 CE |
| `horn_stac` | brass | French horn staccato: short, punchy (rhythmic hits, marches) | 4 × 1 | VSCO 2 CE |
| `trumpet` | brass | trumpet: bright, bold, fanfare (heroic calls, triumph) | 2 × 1 | VSCO 2 CE |
| `trombone` | brass | tenor trombone: broad, solemn (chorales, weight, villains) | 3 × 1 | VSCO 2 CE |
| `tuba` | brass | tuba: big, soft low brass (bass under brass, oom-pah) | 3 × 1 | VSCO 2 CE |
| `piano` | keys | Kawai grand piano, several velocity layers: clear, full (melody, chords, arpeggios) | 4 × 1 | VCSL |
| `harp` | plucked | concert harp: glassy, rippling (arpeggios, glissandi, magic, lullabies) | 1 × 1 | VSCO 2 CE |
| `kalimba` | plucked | Kenyan kalimba: soft metal tines, toylike but warm (cozy loops, puzzles) | 1 × 2 | VCSL |
| `glockenspiel` | mallets | glockenspiel: tiny, bright bells (sparkle, doubling the tune high) | 1 × 1 | VCSL |
| `marimba` | mallets | marimba: warm wooden bars (ostinatos, gentle rhythm, tropical) | 1 × 1 | VCSL |
| `xylophone` | mallets | xylophone: dry, bright wood (quick comic runs, bounce) | 1 × 1 | VSCO 2 CE |
| `vibraphone` | mallets | vibraphone, soft mallets: mellow metal, dreamy (night, mystery, jazz chords) | 2 × 1 | VCSL |
| `tubular_bells` | mallets | tubular bells: church-like chimes (big moments, clocks, ceremonies) | 1 × 1 | VSCO 2 CE |
| `choir_aahs` | choir | wordless choir on 'aah': airy, wide (pads, wonder, sacred or epic swells) | 9 × 1 | FluidR3_GM |
| `choir_oohs` | choir | wordless voices on 'ooh': soft, intimate (gentle pads, lullaby, ghostly) | 9 × 1 | FluidR3_GM |
| `timpani` | percussion | timpani: tuned orchestral drum (heartbeat, rolls of tension, accents) | 3 × 2 | VSCO 2 CE |
| `bass_drum` | percussion | concert bass drum: deep boom (big hits, distant thunder) | 5 × 2 | VCSL |
| `snare` | percussion | concert snare: crisp (marches, rolls, adventure) | 5 × 2 | VCSL |
| `cymbal` | percussion | suspended cymbal: washes and crashes (swells into a section, climaxes) | 4 × 1 | VCSL |
| `triangle` | percussion | triangle: clear ting (sparkle on downbeats, festive) | 2 × 2 | VCSL |
| `tambourine` | percussion | tambourine: jingly shakes and hits (dance, festival, travel) | 3 × 2 | VCSL |
| `shaker` | percussion | small shaker: soft rhythmic hiss (gentle pulse under a groove) | 1 × 2 | VCSL |
| `bongos` | hand_drums | bongos: high, bright hand hits (light grooves, island, playful) | 3 × 2 | VCSL |
| `congas` | hand_drums | congas (quinto, conga, tumba): warm open tones and slaps (grooves) | 4 × 2 | VCSL |
| `frame_drum` | hand_drums | frame drum: soft, deep hand drum (folk pulse, journeys, ritual) | 2 × 2 | VCSL |
| `cajon` | hand_drums | cajon: woody box bass and snappy slap (acoustic grooves) | 3 × 2 | VCSL |
| `darbuka` | hand_drums | darbuka: crisp goblet drum doum/tek (desert, market, fast patterns) | 2 × 2 | VCSL |

Layers × RR: velocity layers and round robins at the busiest key, as `ccez-compose instrument` reports them.
