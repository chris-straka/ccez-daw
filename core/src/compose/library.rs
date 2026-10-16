//! The sampled-instrument catalog: free libraries whose licences allow
//! shipping the music (and the samples inside it) in a commercial game,
//! downloaded on demand into a cache and checked against
//! `core/assets/sample-lock.json` (source URL + SHA-256 per file).
//!
//! Sources (licences checked 2026-10-15; see `docs/notes/sample-libraries.md`):
//! - `vsco2ce`: VS Chamber Orchestra 2 Community Edition (Versilian
//!   Studios), CC0 1.0. Strings, woodwinds, brass, harp, mallets, timpani.
//! - `vcsl`: Versilian Community Sample Library, CC0 1.0. Grand piano,
//!   kalimba, vibraphone, glockenspiel, marimba, orchestral and hand drums.
//! - `fluidr3`: FluidR3_GM SoundFont (Frank Wen), MIT (keep the notice in
//!   the game's credits). Wordless choir: aahs and oohs. Taken from
//!   Debian's `fluid-soundfont-gm` package.
//!
//! Big files never go in git: the cache lives at `$CCEZ_SAMPLES_DIR`, else
//! `$XDG_CACHE_HOME/ccez-daw/samples`, else `~/.cache/ccez-daw/samples`.
//! Downloads go through `curl` (and `tar`/`xz` for the Debian archive),
//! the way previews go through `ffmpeg`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::sampler::{PlayStyle, Region, SampleRef, SampledInstrument};
use super::sha256::sha256_file;

/// Where a library comes from and what it may be used for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Source {
    pub id: &'static str,
    pub name: &'static str,
    pub licence: &'static str,
    pub licence_url: &'static str,
    pub homepage: &'static str,
    /// Files are fetched from `base_url` + their path (git sources).
    pub base_url: &'static str,
}

pub const SOURCES: &[Source] = &[
    Source {
        id: "vsco2ce",
        name: "VS Chamber Orchestra 2: Community Edition (Versilian Studios)",
        licence: "CC0-1.0",
        licence_url: "https://github.com/sgossner/VSCO-2-CE/blob/6dd651d55dde97fd4028699be9d4481f26917891/LICENSE",
        homepage: "https://github.com/sgossner/VSCO-2-CE",
        base_url: "https://raw.githubusercontent.com/sgossner/VSCO-2-CE/6dd651d55dde97fd4028699be9d4481f26917891/",
    },
    Source {
        id: "vcsl",
        name: "Versilian Community Sample Library (Versilian Studios)",
        licence: "CC0-1.0",
        licence_url: "https://github.com/sgossner/VCSL/blob/dfcf4a4918771eee884b96ad4493de82ef84daf6/LICENSE",
        homepage: "https://github.com/sgossner/VCSL",
        base_url: "https://raw.githubusercontent.com/sgossner/VCSL/dfcf4a4918771eee884b96ad4493de82ef84daf6/",
    },
    Source {
        id: "fluidr3",
        name: "FluidR3_GM SoundFont (Frank Wen), Debian fluid-soundfont-gm 3.1-5.3",
        licence: "MIT",
        licence_url: "https://sources.debian.org/src/fluid-soundfont/3.1-5.3/debian/copyright/",
        homepage: "https://packages.debian.org/fluid-soundfont-gm",
        base_url: "",
    },
];

pub fn source(id: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.id == id)
}

/// FluidR3 ships inside a Debian package: fetch it, pull the .sf2 out.
pub const FLUIDR3_DEB_URLS: &[&str] = &[
    "https://deb.debian.org/debian/pool/main/f/fluid-soundfont/fluid-soundfont-gm_3.1-5.3_all.deb",
    "https://snapshot.debian.org/archive/debian/20211010T000000Z/pool/main/f/fluid-soundfont/fluid-soundfont-gm_3.1-5.3_all.deb",
];
pub const FLUIDR3_DEB_SHA256: &str = "6f531493ac4e4d9772fd96b2488ea1790af81c196135fbdd25997da0781fc60e";
pub const FLUIDR3_MEMBER: &str = "./usr/share/sounds/sf2/FluidR3_GM.sf2";
pub const FLUIDR3_FILE: &str = "FluidR3_GM.sf2";

/// One instrument an agent can pick by id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub id: &'static str,
    pub family: &'static str,
    /// What it sounds like and what it is good for.
    pub description: &'static str,
    pub source: &'static str,
    /// SFZ path in the source, or the .sf2 file.
    pub file: &'static str,
    /// SF2 (bank, program).
    pub preset: Option<(u16, u16)>,
    /// Comfortable MIDI range (notes outside may be silent: no region).
    pub range: (u8, u8),
    /// Default seat on the stage, -1 left ..= 1 right.
    pub pan: f32,
    /// Level calibration so a mezzo note sits near the others.
    pub gain_db: f32,
    pub style: PlayStyle,
}

const SUS: PlayStyle = PlayStyle {
    auto_loop: true,
    min_release: 0.35,
    one_shot: false,
};
const PLUCK: PlayStyle = PlayStyle {
    auto_loop: false,
    min_release: 0.25,
    one_shot: false,
};
const RING: PlayStyle = PlayStyle {
    auto_loop: false,
    min_release: 0.0,
    one_shot: true,
};

const fn e(
    id: &'static str,
    family: &'static str,
    description: &'static str,
    source: &'static str,
    file: &'static str,
    range: (u8, u8),
    pan: f32,
    gain_db: f32,
    style: PlayStyle,
) -> Entry {
    Entry {
        id,
        family,
        description,
        source,
        file,
        preset: None,
        range,
        pan,
        gain_db,
        style,
    }
}

pub const CATALOG: &[Entry] = &[
    // Strings (VSCO 2 CE): sections with vibrato, and pizzicato.
    e("violins", "strings", "violin section, sustained with vibrato: warm, singing (melodies, high pads, swells)", "vsco2ce", "ViolinEnsSusVib.sfz", (55, 86), -0.45, 5.0, SUS),
    e("violas", "strings", "viola section, sustained: darker, reedy middle (inner harmony, counter-lines)", "vsco2ce", "ViolaEnsSusVib.sfz", (48, 86), -0.1, -4.0, SUS),
    e("cellos", "strings", "cello section, sustained: rich, warm low-mid (bass lines, tender tunes, pads)", "vsco2ce", "CelloEnsSusVib.sfz", (36, 76), 0.3, -2.5, SUS),
    e("basses", "strings", "solo contrabass, sustained without vibrato: deep, soft floor (roots, pedal tones)", "vsco2ce", "ContrabassSusNV.sfz", (28, 60), 0.45, 6.0, SUS),
    e("solo_violin", "strings", "solo violin with vibrato: intimate, expressive (lead melody, lonely moments)", "vsco2ce", "SViolinVib.sfz", (55, 96), -0.2, 1.0, SUS),
    e("violins_trem", "strings", "violin section tremolo: shimmering, nervous (tension beds, suspense)", "vsco2ce", "ViolinEnsTrem.sfz", (55, 86), -0.45, 2.5, SUS),
    e("violins_pizz", "strings", "violin section pizzicato: light, bouncy plucks (ostinatos, playful, sneaking)", "vsco2ce", "ViolinEnsPizz.sfz", (55, 86), -0.45, 1.5, PLUCK),
    e("violas_pizz", "strings", "viola section pizzicato: rounder mid plucks (off-beat chords, ostinatos)", "vsco2ce", "ViolaEnsPizz.sfz", (48, 86), -0.1, 0.5, PLUCK),
    e("cellos_pizz", "strings", "cello section pizzicato: woody, resonant (walking bass, bouncing low lines)", "vsco2ce", "CelloEnsPizz.sfz", (36, 76), 0.3, 3.5, PLUCK),
    e("basses_pizz", "strings", "contrabass pizzicato: round, deep pluck (bass lines, jazz-ish walks)", "vsco2ce", "ContrabassPizz.sfz", (28, 60), 0.45, 6.5, PLUCK),
    // Woodwinds (VSCO 2 CE).
    e("flute", "woodwinds", "flute with vibrato: airy, bright, gentle (lead melody, birdlike lines)", "vsco2ce", "FluteSusVib.sfz", (60, 96), -0.15, 3.5, SUS),
    e("flute_stac", "woodwinds", "flute staccato: short, breathy dots (playful figures, sparkle)", "vsco2ce", "FluteStac.sfz", (65, 96), -0.15, 12.0, PLUCK),
    e("piccolo", "woodwinds", "piccolo: piercing, bright, very high (festive top lines, doubling the tune)", "vsco2ce", "PiccoloSus.sfz", (74, 91), -0.2, -2.0, SUS),
    e("oboe", "woodwinds", "oboe with vibrato: plaintive, pastoral, nasal (melody, folk, longing)", "vsco2ce", "OboeSusVib.sfz", (58, 89), 0.1, 5.0, SUS),
    e("oboe_stac", "woodwinds", "oboe staccato: pointed, comic (quirky figures)", "vsco2ce", "OboeStac.sfz", (58, 89), 0.1, 4.0, PLUCK),
    e("clarinet", "woodwinds", "clarinet, long notes: round, mellow, woody (melody, warm inner lines)", "vsco2ce", "ClarinetSus.sfz", (50, 89), 0.2, 2.5, SUS),
    e("clarinet_stac", "woodwinds", "clarinet staccato: soft, bubbly (light rhythms, arpeggios)", "vsco2ce", "ClarinetStac.sfz", (50, 89), 0.2, 6.0, PLUCK),
    e("bassoon", "woodwinds", "bassoon: reedy, gentle low voice, a bit comic (bass lines, grumpy tunes)", "vsco2ce", "BassoonSus.sfz", (34, 72), 0.25, 3.0, SUS),
    // Brass (VSCO 2 CE).
    e("horn", "brass", "French horn: noble, warm, heroic at forte (themes, warm pads, adventure)", "vsco2ce", "FHornSus.sfz", (41, 77), -0.3, 2.0, SUS),
    e("horn_stac", "brass", "French horn staccato: short, punchy (rhythmic hits, marches)", "vsco2ce", "FHornStac.sfz", (41, 77), -0.3, -0.5, PLUCK),
    e("trumpet", "brass", "trumpet: bright, bold, fanfare (heroic calls, triumph)", "vsco2ce", "TrumpetSus.sfz", (55, 82), 0.15, -6.5, SUS),
    e("trombone", "brass", "tenor trombone: broad, solemn (chorales, weight, villains)", "vsco2ce", "TromboneSus.sfz", (40, 65), 0.3, 8.5, SUS),
    e("tuba", "brass", "tuba: big, soft low brass (bass under brass, oom-pah)", "vsco2ce", "TubaSus.sfz", (29, 58), 0.35, 3.0, SUS),
    // Keys and plucked.
    e("piano", "keys", "Kawai grand piano, several velocity layers: clear, full (melody, chords, arpeggios)", "vcsl", "Chordophones/Zithers/Grand Piano, Kawai.sfz", (21, 108), 0.0, 4.5, PLUCK),
    e("harp", "plucked", "concert harp: glassy, rippling (arpeggios, glissandi, magic, lullabies)", "vsco2ce", "Harp.sfz", (28, 101), 0.35, 6.5, PLUCK),
    e("kalimba", "plucked", "Kenyan kalimba: soft metal tines, toylike but warm (cozy loops, puzzles)", "vcsl", "Idiophones/Plucked Idiophones/Kalimba, Kenya.sfz", (60, 84), 0.15, 10.0, RING),
    // Mallets.
    e("glockenspiel", "mallets", "glockenspiel: tiny, bright bells (sparkle, doubling the tune high)", "vcsl", "Idiophones/Struck Idiophones/Glockenspiel.sfz", (79, 97), 0.2, 4.0, RING),
    e("marimba", "mallets", "marimba: warm wooden bars (ostinatos, gentle rhythm, tropical)", "vcsl", "Idiophones/Struck Idiophones/Marimba.sfz", (45, 96), -0.2, -2.0, RING),
    e("xylophone", "mallets", "xylophone: dry, bright wood (quick comic runs, bounce)", "vsco2ce", "Xylophone.sfz", (65, 96), 0.25, 0.5, RING),
    e("vibraphone", "mallets", "vibraphone, soft mallets: mellow metal, dreamy (night, mystery, jazz chords)", "vcsl", "Idiophones/Struck Idiophones/Vibraphone - Soft Mallets.sfz", (53, 89), -0.15, -3.5, RING),
    e("tubular_bells", "mallets", "tubular bells: church-like chimes (big moments, clocks, ceremonies)", "vsco2ce", "TubularBells.sfz", (60, 77), 0.1, -4.5, RING),
    // Wordless choir (FluidR3_GM, MIT).
    Entry {
        id: "choir_aahs",
        family: "choir",
        description: "wordless choir on 'aah': airy, wide (pads, wonder, sacred or epic swells)",
        source: "fluidr3",
        file: FLUIDR3_FILE,
        preset: Some((0, 52)),
        range: (48, 79),
        pan: 0.0,
        gain_db: -1.5,
        style: SUS,
    },
    Entry {
        id: "choir_oohs",
        family: "choir",
        description: "wordless voices on 'ooh': soft, intimate (gentle pads, lullaby, ghostly)",
        source: "fluidr3",
        file: FLUIDR3_FILE,
        preset: Some((0, 53)),
        range: (48, 79),
        pan: 0.0,
        gain_db: -4.0,
        style: SUS,
    },
    // Orchestral percussion.
    e("timpani", "percussion", "timpani: tuned orchestral drum (heartbeat, rolls of tension, accents)", "vsco2ce", "Timpani.sfz", (38, 57), 0.0, 7.5, RING),
    e("bass_drum", "percussion", "concert bass drum: deep boom (big hits, distant thunder)", "vcsl", "Membranophones/Struck Membranophones/Bass Drum 2.sfz", (0, 127), 0.0, -9.0, RING),
    e("snare", "percussion", "concert snare: crisp (marches, rolls, adventure)", "vcsl", "Membranophones/Struck Membranophones/Snare Drum, Modern 1.sfz", (0, 127), 0.1, 0.5, RING),
    e("cymbal", "percussion", "suspended cymbal: washes and crashes (swells into a section, climaxes)", "vcsl", "Idiophones/Struck Idiophones/Suspended Cymbal 1.sfz", (0, 127), 0.2, -1.0, RING),
    e("triangle", "percussion", "triangle: clear ting (sparkle on downbeats, festive)", "vcsl", "Idiophones/Struck Idiophones/Triangles.sfz", (0, 127), 0.3, 4.0, RING),
    e("tambourine", "percussion", "tambourine: jingly shakes and hits (dance, festival, travel)", "vcsl", "Idiophones/Struck Idiophones/Tambourine 1.sfz", (0, 127), -0.3, 3.5, RING),
    e("shaker", "percussion", "small shaker: soft rhythmic hiss (gentle pulse under a groove)", "vcsl", "Idiophones/Struck Idiophones/Shaker, Small.sfz", (0, 127), 0.35, 6.5, RING),
    // Hand drums (VCSL).
    e("bongos", "hand_drums", "bongos: high, bright hand hits (light grooves, island, playful)", "vcsl", "Membranophones/Struck Membranophones/Bongos.sfz", (0, 127), 0.25, -2.5, RING),
    e("congas", "hand_drums", "congas (quinto, conga, tumba): warm open tones and slaps (grooves)", "vcsl", "Membranophones/Struck Membranophones/Conga.sfz", (0, 127), -0.25, -0.5, RING),
    e("frame_drum", "hand_drums", "frame drum: soft, deep hand drum (folk pulse, journeys, ritual)", "vcsl", "Membranophones/Struck Membranophones/Frame Drum.sfz", (0, 127), 0.0, -4.0, RING),
    e("cajon", "hand_drums", "cajon: woody box bass and snappy slap (acoustic grooves)", "vcsl", "Idiophones/Struck Idiophones/Cajon.sfz", (0, 127), 0.0, 9.0, RING),
    e("darbuka", "hand_drums", "darbuka: crisp goblet drum doum/tek (desert, market, fast patterns)", "vcsl", "Membranophones/Struck Membranophones/Darbuka.sfz", (0, 127), 0.15, 3.5, RING),
];

pub fn entry(id: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.id == id)
}

// ---------------------------------------------------------------- lock --

/// `core/assets/sample-lock.json`: every file the catalog needs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    /// source id -> path in source -> file.
    pub sources: BTreeMap<String, BTreeMap<String, LockedFile>>,
    /// instrument id -> the files (paths in its source) it needs.
    pub instruments: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedFile {
    pub bytes: u64,
    pub sha256: String,
}

pub const LOCK_JSON: &str = include_str!("../../assets/sample-lock.json");

pub fn lock() -> Lock {
    serde_json::from_str(LOCK_JSON).expect("core/assets/sample-lock.json parses")
}

// --------------------------------------------------------------- cache --

pub fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("CCEZ_SAMPLES_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("ccez-daw").join("samples")
}

pub fn local_path(root: &Path, source: &str, rel: &str) -> PathBuf {
    root.join(source).join(rel)
}

/// Percent-encode a path for a URL (keeps `/`).
pub fn url_path(rel: &str) -> String {
    let mut out = String::new();
    for b in rel.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The files (path in source) an entry needs, from the lock: its SFZ (or
/// SF2) and every recording the SFZ references.
pub fn entry_files(lock: &Lock, e: &Entry) -> Vec<String> {
    lock.instruments.get(e.id).cloned().unwrap_or_default()
}

/// Bytes still to download for an entry (0 when cached).
pub fn missing_bytes(root: &Path, lock: &Lock, e: &Entry) -> u64 {
    let Some(files) = lock.sources.get(e.source) else {
        return 0;
    };
    entry_files(lock, e)
        .iter()
        .filter_map(|f| files.get(f).map(|lf| (f, lf)))
        .filter(|(f, lf)| {
            std::fs::metadata(local_path(root, e.source, f)).map(|m| m.len()).ok() != Some(lf.bytes)
        })
        .map(|(_, lf)| lf.bytes)
        .sum()
}

fn curl_many(jobs: &[(String, PathBuf)]) -> Result<(), String> {
    if jobs.is_empty() {
        return Ok(());
    }
    let mut cfg = String::new();
    for (url, out) in jobs {
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        cfg.push_str(&format!(
            "url = \"{}\"\noutput = \"{}\"\n",
            url.replace('"', "%22"),
            out.display().to_string().replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    let cfg_path = std::env::temp_dir().join(format!(
        "ccez-fetch-{}-{}.curl",
        std::process::id(),
        jobs.len()
    ));
    std::fs::write(&cfg_path, cfg).map_err(|e| e.to_string())?;
    let status = Command::new("curl")
        .args([
            "--parallel",
            "--parallel-max",
            "8",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
            "--config",
        ])
        .arg(&cfg_path)
        .status();
    let _ = std::fs::remove_file(&cfg_path);
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("curl failed ({s}) fetching {} file(s)", jobs.len())),
        Err(e) => Err(format!("sample download needs curl on PATH ({e})")),
    }
}

/// Make sure every file of `e` is in the cache and matches the lock,
/// downloading what is missing. Returns the bytes downloaded.
pub fn ensure(root: &Path, lock: &Lock, e: &Entry) -> Result<u64, String> {
    let files = lock
        .sources
        .get(e.source)
        .ok_or_else(|| format!("{}: source {:?} is not in the sample lock", e.id, e.source))?;
    if e.source == "fluidr3" {
        return ensure_fluidr3(root, files);
    }
    let src = source(e.source).ok_or_else(|| format!("unknown source {}", e.source))?;
    let mut jobs = Vec::new();
    let mut want = Vec::new();
    for f in entry_files(lock, e) {
        let lf = files
            .get(&f)
            .ok_or_else(|| format!("{}: {f} is not in the sample lock", e.id))?;
        let path = local_path(root, e.source, &f);
        if std::fs::metadata(&path).map(|m| m.len()).ok() == Some(lf.bytes) {
            continue;
        }
        let part = path.with_extension("part");
        jobs.push((format!("{}{}", src.base_url, url_path(&f)), part.clone()));
        want.push((part, path, lf.clone(), f));
    }
    if !jobs.is_empty() {
        let mb: u64 = want.iter().map(|w| w.2.bytes).sum::<u64>() / 1_000_000;
        eprintln!(
            "ccez-compose: downloading {} ({} files, {mb} MB) into {}",
            e.id,
            jobs.len(),
            root.display()
        );
    }
    curl_many(&jobs)?;
    let mut got = 0;
    for (part, path, lf, f) in want {
        let sum = sha256_file(&part).map_err(|err| format!("{}: {err}", part.display()))?;
        if sum != lf.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "{}: {f} checksum mismatch (got {sum}, lock says {}); not using it",
                e.id, lf.sha256
            ));
        }
        std::fs::rename(&part, &path).map_err(|err| format!("{}: {err}", path.display()))?;
        got += lf.bytes;
    }
    Ok(got)
}

/// `ar` archive member by name (Debian packages are `ar` archives).
pub fn ar_member<'a>(ar: &'a [u8], name: &str) -> Option<&'a [u8]> {
    if !ar.starts_with(b"!<arch>\n") {
        return None;
    }
    let mut i = 8;
    while i + 60 <= ar.len() {
        let h = &ar[i..i + 60];
        let n = std::str::from_utf8(&h[0..16]).ok()?.trim().trim_end_matches('/');
        let size: usize = std::str::from_utf8(&h[48..58]).ok()?.trim().parse().ok()?;
        let body = ar.get(i + 60..i + 60 + size)?;
        if n == name {
            return Some(body);
        }
        i += 60 + size + (size & 1);
    }
    None
}

fn ensure_fluidr3(root: &Path, files: &BTreeMap<String, LockedFile>) -> Result<u64, String> {
    let lf = files
        .get(FLUIDR3_FILE)
        .ok_or("fluidr3: FluidR3_GM.sf2 is not in the sample lock")?;
    let path = local_path(root, "fluidr3", FLUIDR3_FILE);
    if std::fs::metadata(&path).map(|m| m.len()).ok() == Some(lf.bytes) {
        return Ok(0);
    }
    let dir = root.join("fluidr3");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let deb = dir.join("fluid-soundfont-gm.deb.part");
    eprintln!("ccez-compose: downloading the FluidR3_GM choir (Debian package, 120 MB)");
    let mut last = String::new();
    let mut ok = false;
    for url in FLUIDR3_DEB_URLS {
        match curl_many(&[(url.to_string(), deb.clone())]) {
            Ok(()) => {
                let sum = sha256_file(&deb).map_err(|e| e.to_string())?;
                if sum == FLUIDR3_DEB_SHA256 {
                    ok = true;
                    break;
                }
                last = format!("{url}: checksum {sum} != {FLUIDR3_DEB_SHA256}");
            }
            Err(e) => last = e,
        }
    }
    if !ok {
        let _ = std::fs::remove_file(&deb);
        return Err(format!("fluidr3: no mirror worked ({last})"));
    }
    let bytes = std::fs::read(&deb).map_err(|e| e.to_string())?;
    let data = ar_member(&bytes, "data.tar.xz").ok_or("fluidr3: data.tar.xz not in the package")?;
    let tar = dir.join("data.tar.xz");
    std::fs::write(&tar, data).map_err(|e| e.to_string())?;
    drop(bytes);
    let x = dir.join("x");
    std::fs::create_dir_all(&x).map_err(|e| e.to_string())?;
    let st = Command::new("tar")
        .arg("-xJf")
        .arg(&tar)
        .arg("-C")
        .arg(&x)
        .arg(FLUIDR3_MEMBER)
        .status()
        .map_err(|e| format!("extracting FluidR3 needs tar and xz on PATH ({e})"))?;
    let _ = std::fs::remove_file(&tar);
    let _ = std::fs::remove_file(&deb);
    if !st.success() {
        return Err(format!("fluidr3: tar failed ({st})"));
    }
    let got = x.join(FLUIDR3_MEMBER);
    let sum = sha256_file(&got).map_err(|e| e.to_string())?;
    // An empty lock hash only happens while `samples lock` builds the lock.
    if !lf.sha256.is_empty() && sum != lf.sha256 {
        let _ = std::fs::remove_dir_all(&x);
        return Err(format!("fluidr3: sf2 checksum {sum} != lock {}", lf.sha256));
    }
    std::fs::rename(&got, &path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&x);
    Ok(lf.bytes)
}

/// Lexically normalise `a/b/../c` (SFZ paths may climb out of their dir).
fn normalise(p: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().to_string()),
            _ => {}
        }
    }
    parts.join("/")
}

/// Maintainer command behind `ccez-compose samples lock`: download every
/// catalog instrument (SFZ, the recordings it names, the FluidR3 package),
/// hash each file and return the lock to commit. Trusts the network once
/// (the Debian package is still checked against its published hash);
/// every later fetch is checked against what this recorded.
pub fn build_lock(root: &Path) -> Result<Lock, String> {
    let mut lock = Lock::default();
    for e in CATALOG {
        if e.source == "fluidr3" {
            let path = local_path(root, "fluidr3", FLUIDR3_FILE);
            if !path.exists() {
                let mut tmp = BTreeMap::new();
                tmp.insert(
                    FLUIDR3_FILE.to_string(),
                    LockedFile {
                        bytes: 0,
                        sha256: String::new(),
                    },
                );
                // No sf2 hash yet: keep what the verified .deb holds.
                match ensure_fluidr3(root, &tmp) {
                    Err(m) if m.contains("sf2 checksum") => {}
                    Err(m) => return Err(m),
                    Ok(_) => {}
                }
            }
            let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            lock.sources.entry("fluidr3".into()).or_default().insert(
                FLUIDR3_FILE.into(),
                LockedFile {
                    bytes: meta.len(),
                    sha256: sha256_file(&path).map_err(|e| e.to_string())?,
                },
            );
            lock.instruments.insert(e.id.into(), vec![FLUIDR3_FILE.into()]);
            continue;
        }
        let src = source(e.source).ok_or("unknown source")?;
        let sfz = local_path(root, e.source, e.file);
        if !sfz.exists() {
            let part = sfz.with_extension("part");
            curl_many(&[(format!("{}{}", src.base_url, url_path(e.file)), part.clone())])?;
            std::fs::rename(&part, &sfz).map_err(|err| format!("{}: {err}", sfz.display()))?;
        }
        let text = String::from_utf8_lossy(&std::fs::read(&sfz).map_err(|e| e.to_string())?).to_string();
        let dir = Path::new(e.file).parent().unwrap_or(Path::new(""));
        let mut files = vec![e.file.to_string()];
        for r in super::sfz::referenced_samples(&text)? {
            files.push(normalise(&dir.join(r)));
        }
        files.sort();
        files.dedup();
        let jobs: Vec<(String, PathBuf)> = files
            .iter()
            .filter(|f| !local_path(root, e.source, f).exists())
            .map(|f| {
                let p = local_path(root, e.source, f);
                (format!("{}{}", src.base_url, url_path(f)), p.with_extension("part"))
            })
            .collect();
        if !jobs.is_empty() {
            eprintln!("ccez-compose: lock: fetching {} ({} files)", e.id, jobs.len());
        }
        curl_many(&jobs)?;
        for (_, part) in &jobs {
            let done = part.with_extension(
                files
                    .iter()
                    .map(|f| local_path(root, e.source, f))
                    .find(|p| p.with_extension("part") == *part)
                    .and_then(|p| p.extension().map(|x| x.to_string_lossy().to_string()))
                    .unwrap_or_default(),
            );
            std::fs::rename(part, &done).map_err(|err| format!("{}: {err}", done.display()))?;
        }
        let table = lock.sources.entry(e.source.into()).or_default();
        for f in &files {
            let p = local_path(root, e.source, f);
            let meta = std::fs::metadata(&p).map_err(|err| format!("{}: {err}", p.display()))?;
            table.insert(
                f.clone(),
                LockedFile {
                    bytes: meta.len(),
                    sha256: sha256_file(&p).map_err(|e| e.to_string())?,
                },
            );
        }
        lock.instruments.insert(e.id.into(), files);
    }
    Ok(lock)
}

// ---------------------------------------------------------------- load --

/// Regions of an entry from the cache (call [`ensure`] first).
pub fn regions(root: &Path, e: &Entry) -> Result<Vec<Region>, String> {
    let path = local_path(root, e.source, e.file);
    let mut rs = match e.preset {
        Some((bank, prog)) => super::sf2::load_preset(&path, bank, prog)?,
        None => super::sfz::load(&path)?,
    };
    for r in &mut rs {
        r.volume_db += e.gain_db;
    }
    Ok(rs)
}

/// Load a catalog instrument, downloading it first if needed.
pub fn load(e: &Entry) -> Result<SampledInstrument, String> {
    let root = cache_root();
    ensure(&root, &lock(), e)?;
    Ok(SampledInstrument::new(regions(&root, e)?, e.style))
}

/// A local SFZ or SF2 the composition names directly: `sfz:<path>` or
/// `sf2:<path>#<bank>:<program>` (relative paths resolve from the
/// composition's working directory). For one-off and test instruments;
/// shipped pieces should use catalog ids so anyone can re-render them.
pub fn local_spec(patch: &str) -> Option<Result<(Vec<Region>, PlayStyle), String>> {
    if let Some(p) = patch.strip_prefix("sfz:") {
        return Some(super::sfz::load(Path::new(p)).map(|r| (r, SUS)));
    }
    let rest = patch.strip_prefix("sf2:")?;
    let parsed = (|| {
        let (p, bp) = rest.rsplit_once('#')?;
        let (b, pr) = bp.split_once(':')?;
        Some((p, b.parse::<u16>().ok()?, pr.parse::<u16>().ok()?))
    })();
    Some(match parsed {
        Some((p, b, pr)) => super::sf2::load_preset(Path::new(p), b, pr).map(|r| (r, SUS)),
        None => Err(format!("{patch:?}: want sf2:<path>#<bank>:<program>")),
    })
}

/// Hits of an unpitched instrument: key -> recording name (round-robin
/// and velocity tags stripped), so an agent knows which key is which.
pub fn hits(regions: &[Region]) -> BTreeMap<u8, String> {
    let mut out = BTreeMap::new();
    for r in regions {
        let name = match &r.sample {
            SampleRef::Wav(p) => p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            SampleRef::Sf2 { .. } => continue,
        };
        let clean: Vec<&str> = name
            .split(['_', ' '])
            .filter(|t| {
                let l = t.to_ascii_lowercase();
                !(l.starts_with("rr")
                    || (l.starts_with('v') && l[1..].chars().all(|c| c.is_ascii_digit()))
                    || (l.starts_with("vl") && l[2..].chars().all(|c| c.is_ascii_digit()))
                    || ["pp", "p", "mp", "mf", "f", "ff", "fff", "sum", "mid", "close", "main"]
                        .contains(&l.as_str()))
            })
            .collect();
        for k in r.lokey..=r.hikey {
            out.entry(k).or_insert_with(|| clean.join(" "));
        }
    }
    out
}

/// (velocity layers, round robins) the regions offer, at their busiest
/// key: a layer is a velocity band where the same recordings answer.
pub fn layer_counts(regions: &[Region]) -> (usize, usize) {
    let mut layers = 0;
    for key in 0..=127u8 {
        let mut bands = 0;
        let mut last: Option<Vec<usize>> = None;
        for vel in 1..=127u8 {
            let set: Vec<usize> = regions
                .iter()
                .enumerate()
                .filter(|(_, r)| key >= r.lokey && key <= r.hikey && vel >= r.lovel && vel <= r.hivel)
                .map(|(i, _)| i)
                .collect();
            if !set.is_empty() && last.as_ref() != Some(&set) {
                bands += 1;
            }
            last = Some(set);
        }
        layers = layers.max(bands);
    }
    let rr = regions.iter().map(|r| r.seq_length as usize).max().unwrap_or(1);
    (layers, rr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_the_families_and_is_locked() {
        for fam in [
            "strings",
            "woodwinds",
            "brass",
            "keys",
            "plucked",
            "mallets",
            "choir",
            "percussion",
            "hand_drums",
        ] {
            assert!(CATALOG.iter().any(|e| e.family == fam), "{fam}");
        }
        for id in ["violins", "violins_pizz", "flute", "clarinet", "oboe", "piano", "harp", "glockenspiel", "choir_aahs", "timpani", "bongos"] {
            assert!(entry(id).is_some(), "{id}");
        }
        let l = lock();
        let mut ids = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(ids.insert(e.id), "{} twice", e.id);
            assert!(super::super::synth::patch(e.id).is_none(), "{} shadows a synth patch", e.id);
            let s = source(e.source).unwrap();
            assert!(s.licence == "CC0-1.0" || s.licence == "MIT", "{}", s.licence);
            let files = entry_files(&l, e);
            assert!(files.len() > 1 || e.source == "fluidr3", "{}: {files:?}", e.id);
            for f in files {
                let lf = &l.sources[e.source][&f];
                assert_eq!(lf.sha256.len(), 64, "{f}");
                assert!(lf.bytes > 0, "{f}");
            }
        }
    }

    #[test]
    fn urls_are_encoded_and_ar_members_found() {
        assert_eq!(
            url_path("Chordophones/Zithers/Grand Piano, Kawai.sfz"),
            "Chordophones/Zithers/Grand%20Piano%2C%20Kawai.sfz"
        );
        let mut ar = b"!<arch>\n".to_vec();
        for (name, body) in [("debian-binary", &b"2.0\n"[..]), ("data.tar.xz", &b"xyz"[..])] {
            ar.extend(format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", name, 0, 0, 0, 100644, body.len()).bytes());
            ar.extend_from_slice(body);
            if body.len() % 2 == 1 {
                ar.push(b'\n');
            }
        }
        assert_eq!(ar_member(&ar, "data.tar.xz"), Some(&b"xyz"[..]));
        assert_eq!(ar_member(&ar, "nope"), None);
    }

    #[test]
    fn velocity_layers_count_bands_not_regions() {
        let mk = |lo: u8, hi: u8, pan: f32| {
            let mut r = Region::new(SampleRef::Wav("a.wav".into()));
            r.lovel = lo;
            r.hivel = hi;
            r.pan = pan;
            r
        };
        // A stereo pair (two regions, same band) plus a loud layer: 2 layers.
        let rs = [mk(1, 80, -1.0), mk(1, 80, 1.0), mk(81, 127, 0.0)];
        assert_eq!(layer_counts(&rs), (2, 1));
    }

    #[test]
    fn hit_names_drop_velocity_and_round_robin_tags() {
        let mut r = Region::new(SampleRef::Wav("Bongos/BongoH_Hit1_v1_rr1_Mid.wav".into()));
        r.lokey = 60;
        r.hikey = 60;
        assert_eq!(hits(&[r])[&60], "BongoH Hit1");
    }
}
