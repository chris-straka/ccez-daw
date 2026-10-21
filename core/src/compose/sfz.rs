//! Read the subset of SFZ the free orchestral libraries use (VSCO 2 CE,
//! VCSL): `<control>`/`<global>`/`<master>`/`<group>`/`<region>` headers
//! whose opcodes cascade down, `default_path`, key and velocity ranges,
//! round robins (`seq_length`/`seq_position`, `lorand`/`hirand`),
//! velocity cross-fades, tuning, offsets, loops and the amp envelope.
//! Release-trigger and CC-only (`hikey=-1`) regions are skipped (the
//! composer has no key-up noise or controllers); `#define $NAME` values
//! substitute longest name first; a `<` without `>` is an error;
//! unknown opcodes are ignored.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::sampler::{Env, LoopMode, Region, SampleRef};

type Ops = BTreeMap<String, String>;

/// Split SFZ text into (header, opcodes) blocks. Values may contain
/// spaces (`sample=Strings\Violin Section\a.wav`): a value runs until the
/// next `name=` or header. A `<` without its `>` is an error.
fn blocks(text: &str) -> Result<Vec<(String, Ops)>, String> {
    let mut out: Vec<(String, Ops)> = Vec::new();
    // `#define $NAME value`; a later define of the same name wins.
    let mut defines: BTreeMap<String, String> = BTreeMap::new();
    for (ln, raw) in text.lines().enumerate() {
        let line = match raw.find("//") {
            Some(i) => &raw[..i],
            None => raw,
        };
        if let Some(rest) = line.trim().strip_prefix("#define") {
            let mut it = rest.split_whitespace();
            if let (Some(k), Some(v)) = (it.next(), it.next()) {
                defines.insert(k.to_string(), v.to_string());
            }
            continue;
        }
        // Longest names first, so `$VEL` never clobbers part of `$VEL2`.
        let mut by_len: Vec<(&String, &String)> = defines.iter().collect();
        by_len.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        let mut line = line.to_string();
        for (k, v) in by_len {
            line = line.replace(k.as_str(), v);
        }
        // Headers can share a line with opcodes: `<region> sample=a.wav`.
        let mut rest = line.as_str();
        while !rest.trim().is_empty() {
            rest = rest.trim_start();
            let (chunk, next) = match (rest.find('<'), rest.find('>')) {
                (Some(0), Some(e)) => {
                    out.push((rest[1..e].trim().to_string(), Ops::new()));
                    rest = &rest[e + 1..];
                    continue;
                }
                (Some(0), None) => {
                    return Err(format!("line {}: unterminated header {:?}", ln + 1, rest.trim()));
                }
                (Some(s), _) => (&rest[..s], &rest[s..]),
                _ => (rest, ""),
            };
            let mut key: Option<String> = None;
            let mut val = String::new();
            for tok in chunk.split_whitespace() {
                let eq = tok.find('=');
                let is_op = eq.is_some_and(|i| {
                    i > 0
                        && tok[..i]
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_')
                });
                if is_op {
                    let i = eq.unwrap();
                    if let (Some(k), Some(b)) = (key.take(), out.last_mut()) {
                        b.1.insert(k, val.trim().to_string());
                    }
                    key = Some(tok[..i].to_string());
                    val = tok[i + 1..].to_string();
                } else if key.is_some() {
                    val.push(' ');
                    val.push_str(tok);
                }
            }
            if let (Some(k), Some(b)) = (key, out.last_mut()) {
                b.1.insert(k, val.trim().to_string());
            }
            rest = next;
        }
    }
    Ok(out)
}

/// A key: MIDI number or a note name (`c4` = 60, `f#3`, `eb2`).
fn key(v: &str) -> Option<u8> {
    let v = v.trim();
    if let Ok(n) = v.parse::<i32>() {
        return u8::try_from(n.clamp(0, 127)).ok();
    }
    super::parse_pitch(v).ok()
}

fn num(v: &str) -> Option<f32> {
    v.trim().parse::<f32>().ok()
}

/// Parse SFZ `text` that lives in `dir` (sample paths resolve from there).
pub fn parse(text: &str, dir: &Path) -> Result<Vec<Region>, String> {
    let mut control = Ops::new();
    let mut global = Ops::new();
    let mut master = Ops::new();
    let mut group = Ops::new();
    let mut regions = Vec::new();
    for (header, ops) in blocks(text)? {
        match header.as_str() {
            "control" => control.extend(ops),
            "global" => {
                global = ops;
                master.clear();
                group.clear();
            }
            "master" => {
                master = ops;
                group.clear();
            }
            "group" => group = ops,
            "region" => {
                let mut all = global.clone();
                all.extend(master.clone());
                all.extend(group.clone());
                all.extend(ops);
                if let Some(r) = region(&all, &control, dir)? {
                    regions.push(r);
                }
            }
            "curve" | "effect" | "midi" | "sample" => {}
            other => return Err(format!("unknown SFZ header <{other}>")),
        }
    }
    Ok(regions)
}

fn region(o: &Ops, control: &Ops, dir: &Path) -> Result<Option<Region>, String> {
    let Some(sample) = o.get("sample") else {
        return Ok(None);
    };
    if o.get("trigger").is_some_and(|t| t != "attack" && t != "first" && t != "legato") {
        return Ok(None);
    }
    if sample.starts_with('*') {
        return Ok(None); // generated waveforms (*sine): not a recording
    }
    let default_path = control.get("default_path").cloned().unwrap_or_default();
    let rel = format!("{default_path}{sample}").replace('\\', "/");
    let path: PathBuf = dir.join(rel);
    let mut r = Region::new(SampleRef::Wav(path));
    let g = |k: &str| o.get(k).map(String::as_str);
    // `hikey=-1` / `key=-1`: never triggered by a key (CC-only regions).
    let negative = |k: &str| g(k).and_then(num).is_some_and(|v| v < 0.0);
    if negative("hikey") || negative("key") {
        return Ok(None);
    }
    if let Some(k) = g("key").and_then(key) {
        r.lokey = k;
        r.hikey = k;
        r.keycenter = k as f32;
    }
    if let Some(k) = g("lokey").and_then(key) {
        r.lokey = k;
    }
    if let Some(k) = g("hikey").and_then(key) {
        r.hikey = k;
    }
    if let Some(k) = g("pitch_keycenter").and_then(key) {
        r.keycenter = k as f32;
    }
    if let Some(v) = g("lovel").and_then(num) {
        r.lovel = v.clamp(1.0, 127.0) as u8;
    }
    if let Some(v) = g("hivel").and_then(num) {
        r.hivel = v.clamp(0.0, 127.0) as u8;
    }
    let xf = |k: &str, d: u8| g(k).and_then(num).map(|v| v.clamp(0.0, 127.0) as u8).unwrap_or(d);
    if ["xfin_lovel", "xfin_hivel", "xfout_lovel", "xfout_hivel"]
        .iter()
        .any(|k| o.contains_key(*k))
    {
        r.xfade = Some((
            xf("xfin_lovel", 0),
            xf("xfin_hivel", 0),
            xf("xfout_lovel", 127),
            xf("xfout_hivel", 127),
        ));
    }
    r.tune = g("tune").and_then(num).unwrap_or(0.0)
        + 100.0 * g("transpose").and_then(num).unwrap_or(0.0);
    if let Some(v) = g("pitch_keytrack").and_then(num) {
        r.keytrack = v;
    }
    r.volume_db = g("volume").and_then(num).unwrap_or(0.0)
        + g("gain").and_then(num).unwrap_or(0.0);
    r.pan = g("pan").and_then(num).unwrap_or(0.0) / 100.0;
    if let Some(v) = g("seq_length").and_then(num) {
        r.seq_length = v.max(1.0) as u32;
    }
    if let Some(v) = g("seq_position").and_then(num) {
        r.seq_position = v.max(1.0) as u32;
    }
    if let Some(v) = g("lorand").and_then(num) {
        r.lorand = v;
    }
    if let Some(v) = g("hirand").and_then(num) {
        r.hirand = if v >= 1.0 { 1.0001 } else { v };
    }
    if let Some(v) = g("offset").and_then(num) {
        r.offset = v.max(0.0) as usize;
    }
    if let Some(v) = g("end").and_then(num) {
        r.end = Some((v.max(0.0) as usize).saturating_add(1));
    }
    r.loop_mode = match g("loop_mode") {
        Some("one_shot") => LoopMode::OneShot,
        Some("loop_continuous") | Some("loop_sustain") => LoopMode::Loop,
        Some("no_loop") => LoopMode::NoLoop,
        // Unset: loop_continuous if the WAV has its own loop, else no_loop
        // (decided when the recording is read).
        _ => LoopMode::FromFile,
    };
    // Each side on its own; an unset one defaults (file loop, else the
    // recording's edge) when the recording is read. `loop_end` is
    // inclusive in SFZ, exclusive here.
    r.loop_start = g("loop_start").and_then(num).map(|a| a.max(0.0) as usize);
    r.loop_end = g("loop_end")
        .and_then(num)
        .map(|b| (b.max(0.0) as usize).saturating_add(1));
    r.env = Env {
        attack: g("ampeg_attack").and_then(num).unwrap_or(0.0),
        hold: g("ampeg_hold").and_then(num).unwrap_or(0.0),
        decay: g("ampeg_decay").and_then(num).unwrap_or(0.0),
        sustain: g("ampeg_sustain").and_then(num).unwrap_or(100.0) / 100.0,
        release: g("ampeg_release").and_then(num).unwrap_or(0.001),
    };
    if let Some(v) = g("amp_veltrack").and_then(num) {
        r.veltrack = v / 100.0;
    }
    Ok(Some(r))
}

/// Parse an SFZ file from disk.
pub fn load(path: &Path) -> Result<Vec<Region>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let dir = path.parent().unwrap_or(Path::new("."));
    parse(&text, dir).map_err(|e| format!("{}: {e}", path.display()))
}

/// The WAV paths an SFZ references, relative to its own directory
/// (what `ccez-compose samples lock` downloads and checksums).
pub fn referenced_samples(text: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = parse(text, Path::new(""))?
        .into_iter()
        .filter_map(|r| match r.sample {
            SampleRef::Wav(p) => Some(p.to_string_lossy().replace('\\', "/")),
            SampleRef::Sf2 { .. } => None,
        })
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcodes_cascade_and_paths_keep_spaces() {
        let text = "<control>\ndefault_path=Strings\\Violin Section\\susVib\\\n\
            <global> ampeg_release=0.8 volume=-3\n\
            <group> seq_length=2 // a comment\n\
            <region> sample=Vln A3 v1.wav lokey=68 hikey=c#5 pitch_keycenter=69 seq_position=1 hivel=62\n\
            <region>\nsample=b.wav\nkey=60\nseq_position=2\nvolume=2 tune=-12\n\
            <region> sample=rel.wav trigger=release key=60\n";
        let rs = parse(text, Path::new("/lib")).unwrap();
        assert_eq!(rs.len(), 2, "release trigger skipped");
        let a = &rs[0];
        assert_eq!(
            a.sample,
            SampleRef::Wav(PathBuf::from("/lib/Strings/Violin Section/susVib/Vln A3 v1.wav"))
        );
        assert_eq!((a.lokey, a.hikey, a.keycenter), (68, 73, 69.0));
        assert_eq!((a.lovel, a.hivel), (1, 62));
        assert_eq!((a.seq_length, a.seq_position), (2, 1));
        assert_eq!(a.env.release, 0.8);
        assert_eq!(a.volume_db, -3.0, "global volume");
        let b = &rs[1];
        assert_eq!((b.lokey, b.hikey, b.keycenter), (60, 60, 60.0));
        assert_eq!((b.volume_db, b.tune), (2.0, -12.0), "region overrides global");
        let refs = referenced_samples(text).unwrap();
        assert_eq!(refs.len(), 2);
        assert!(refs[0].ends_with("susVib/Vln A3 v1.wav"), "{refs:?}");
    }

    #[test]
    fn velocity_crossfades_and_unknown_headers() {
        let rs = parse(
            "<region> sample=a.wav xfin_lovel=0 xfin_hivel=64 xfout_lovel=100 xfout_hivel=127",
            Path::new(""),
        )
        .unwrap();
        assert_eq!(rs[0].xfade, Some((0, 64, 100, 127)));
        assert!(parse("<bogus> a=1", Path::new("")).is_err());
    }

    /// Malformed text is an error that names the line, never a hang.
    #[test]
    fn unterminated_headers_are_errors() {
        let e = parse("<region sample=a.wav key=60\n", Path::new("")).unwrap_err();
        assert!(e.contains("line 1") && e.contains("unterminated"), "{e}");
        let e = parse("<region> sample=a.wav\nkey=60 volume=<3\n", Path::new("")).unwrap_err();
        assert!(e.contains("line 2"), "{e}");
        // Huge numbers saturate instead of overflowing.
        let rs = parse("<region> sample=a.wav end=1e30 loop_start=0 loop_end=1e30", Path::new("")).unwrap();
        assert_eq!(rs[0].end, Some(usize::MAX));
    }

    #[test]
    fn defines_replace_longest_name_first_and_redefine() {
        let rs = parse(
            "#define $VEL 10\n#define $VEL2 90\n#define $K 50\n#define $K 60\n\
             <region> sample=a.wav lovel=$VEL hivel=$VEL2 key=$K",
            Path::new(""),
        )
        .unwrap();
        assert_eq!((rs[0].lovel, rs[0].hivel), (10, 90));
        assert_eq!(rs[0].lokey, 60, "a later #define wins");
    }

    /// `hikey=-1` (or `key=-1`): the region is never key-triggered (it
    /// answers CCs only), so the composer skips it rather than mapping it
    /// to key 0.
    #[test]
    fn negative_keys_mean_not_key_triggered() {
        let rs = parse(
            "<region> sample=cc.wav lokey=-1 hikey=-1\n<region> sample=k.wav key=-1\n\
             <region> sample=a.wav lokey=-1 hikey=40",
            Path::new(""),
        )
        .unwrap();
        assert_eq!(rs.len(), 1, "{rs:?}");
        assert_eq!((rs[0].lokey, rs[0].hikey), (0, 40));
    }
}
