//! Read one preset of a SoundFont 2 file into sampler regions.
//!
//! Teaching note: an SF2 is a RIFF file with a pool of 16-bit samples
//! (`sdta/smpl`) and a "hydra" of tables (`pdta`) describing presets →
//! instruments → zones. A zone is a list of *generators* (key range,
//! velocity range, root key, tuning, attenuation, envelope times, the
//! sample it plays). Preset-level generators *add* to the instrument's
//! (ranges intersect), and a zone with no sample/instrument is the
//! global default for its siblings. We flatten all that into
//! [`Region`]s; samples load lazily by slice ([`read_slice`]).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::sampler::{Env, LoopMode, Region, SampleRef};
use super::wavread::Audio;

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Byte offset and length of `sdta/smpl` plus the `pdta` body.
struct Layout {
    smpl_at: u64,
    smpl_len: u64,
    pdta: Vec<u8>,
}

fn layout(path: &Path) -> Result<Layout, String> {
    let err = |m: String| format!("{}: {m}", path.display());
    let mut f = std::fs::File::open(path).map_err(|e| err(e.to_string()))?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).map_err(|e| err(e.to_string()))?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"sfbk" {
        return Err(err("not a SoundFont 2 file".into()));
    }
    let total = f.metadata().map_err(|e| err(e.to_string()))?.len();
    let (mut smpl, mut pdta) = (None, None);
    let mut at = 12u64;
    while at + 12 <= total {
        f.seek(SeekFrom::Start(at)).map_err(|e| err(e.to_string()))?;
        let mut h = [0u8; 12];
        f.read_exact(&mut h).map_err(|e| err(e.to_string()))?;
        let len = u32le(&h, 4) as u64;
        if &h[0..4] == b"LIST" {
            match &h[8..12] {
                b"sdta" => {
                    // Sub-chunks: smpl (and maybe sm24).
                    let mut sub = at + 12;
                    while sub + 8 <= at + 8 + len {
                        f.seek(SeekFrom::Start(sub)).map_err(|e| err(e.to_string()))?;
                        let mut c = [0u8; 8];
                        f.read_exact(&mut c).map_err(|e| err(e.to_string()))?;
                        let clen = u32le(&c, 4) as u64;
                        if &c[0..4] == b"smpl" {
                            smpl = Some((sub + 8, clen));
                        }
                        sub += 8 + clen + (clen & 1);
                    }
                }
                b"pdta" => {
                    let mut body = vec![0u8; len as usize - 4];
                    f.read_exact(&mut body).map_err(|e| err(e.to_string()))?;
                    pdta = Some(body);
                }
                _ => {}
            }
        }
        at += 8 + len + (len & 1);
    }
    let (smpl_at, smpl_len) = smpl.ok_or_else(|| err("no smpl chunk".into()))?;
    Ok(Layout {
        smpl_at,
        smpl_len,
        pdta: pdta.ok_or_else(|| err("no pdta list".into()))?,
    })
}

fn sub_chunks(pdta: &[u8]) -> std::collections::HashMap<[u8; 4], &[u8]> {
    let mut out = std::collections::HashMap::new();
    let mut i = 0;
    while i + 8 <= pdta.len() {
        let id = [pdta[i], pdta[i + 1], pdta[i + 2], pdta[i + 3]];
        let len = u32le(pdta, i + 4) as usize;
        out.insert(id, &pdta[i + 8..(i + 8 + len).min(pdta.len())]);
        i += 8 + len + (len & 1);
    }
    out
}

type Gens = Vec<(u16, [u8; 2])>;

fn gens_of(bags: &[u8], gens: &[u8], bag: usize) -> Gens {
    let g0 = u16le(bags, bag * 4) as usize;
    let g1 = u16le(bags, (bag + 1) * 4) as usize;
    (g0..g1)
        .filter(|g| (g + 1) * 4 <= gens.len())
        .map(|g| (u16le(gens, g * 4), [gens[g * 4 + 2], gens[g * 4 + 3]]))
        .collect()
}

fn amount(v: [u8; 2]) -> i32 {
    i16::from_le_bytes(v) as i32
}

const INSTRUMENT: u16 = 41;
const KEY_RANGE: u16 = 43;
const VEL_RANGE: u16 = 44;
const SAMPLE_ID: u16 = 53;

/// Zones of a bag range: the leading global zone (no terminal generator)
/// is merged under each later one.
fn zones(bags: &[u8], gens: &[u8], b0: usize, b1: usize, terminal: u16) -> Vec<Gens> {
    let mut out = Vec::new();
    let mut global: Gens = Vec::new();
    for bag in b0..b1 {
        let g = gens_of(bags, gens, bag);
        if g.iter().any(|(op, _)| *op == terminal) {
            let mut z = global.clone();
            for (op, v) in g {
                z.retain(|(o, _)| *o != op);
                z.push((op, v));
            }
            out.push(z);
        } else if bag == b0 {
            global = g;
        }
    }
    out
}

fn get(g: &Gens, op: u16) -> Option<[u8; 2]> {
    g.iter().rev().find(|(o, _)| *o == op).map(|(_, v)| *v)
}

fn timecents(g: &Gens, op: u16, add: i32) -> f32 {
    let tc = get(g, op).map(amount).unwrap_or(-12000) + add;
    2f32.powf(tc as f32 / 1200.0)
}

/// Every (bank, program, name) preset in the file.
pub fn presets(path: &Path) -> Result<Vec<(u16, u16, String)>, String> {
    let l = layout(path)?;
    let c = sub_chunks(&l.pdta);
    let phdr = c.get(b"phdr").ok_or("no phdr")?;
    let n = phdr.len() / 38;
    Ok((0..n.saturating_sub(1))
        .map(|i| {
            let r = &phdr[i * 38..];
            let name = String::from_utf8_lossy(&r[..20])
                .trim_end_matches('\0')
                .to_string();
            (u16le(r, 22), u16le(r, 20), name)
        })
        .collect())
}

/// Regions of preset `(bank, program)`.
pub fn load_preset(path: &Path, bank: u16, program: u16) -> Result<Vec<Region>, String> {
    let l = layout(path)?;
    let c = sub_chunks(&l.pdta);
    let need = |id: &[u8; 4]| {
        c.get(id)
            .copied()
            .ok_or_else(|| format!("{}: missing {}", path.display(), String::from_utf8_lossy(id)))
    };
    let (phdr, pbag, pgen) = (need(b"phdr")?, need(b"pbag")?, need(b"pgen")?);
    let (inst, ibag, igen, shdr) = (need(b"inst")?, need(b"ibag")?, need(b"igen")?, need(b"shdr")?);
    let np = phdr.len() / 38;
    let pi = (0..np.saturating_sub(1))
        .find(|&i| u16le(phdr, i * 38 + 20) == program && u16le(phdr, i * 38 + 22) == bank)
        .ok_or_else(|| format!("{}: no preset {bank}:{program}", path.display()))?;
    let (pb0, pb1) = (
        u16le(phdr, pi * 38 + 24) as usize,
        u16le(phdr, (pi + 1) * 38 + 24) as usize,
    );
    let pool_frames = (l.smpl_len / 2) as u32;
    let mut out = Vec::new();
    for pz in zones(pbag, pgen, pb0, pb1, INSTRUMENT) {
        let ii = amount(get(&pz, INSTRUMENT).unwrap()) as usize;
        if (ii + 1) * 22 > inst.len() {
            continue;
        }
        let (ib0, ib1) = (
            u16le(inst, ii * 22 + 20) as usize,
            u16le(inst, (ii + 1) * 22 + 20) as usize,
        );
        let range = |g: &Gens, op: u16| get(g, op).map(|v| (v[0], v[1])).unwrap_or((0, 127));
        let (pk, pv) = (range(&pz, KEY_RANGE), range(&pz, VEL_RANGE));
        // Preset generators add to the instrument's.
        let padd = |op: u16| get(&pz, op).map(amount).unwrap_or(0);
        for iz in zones(ibag, igen, ib0, ib1, SAMPLE_ID) {
            let si = amount(get(&iz, SAMPLE_ID).unwrap()) as usize;
            if (si + 1) * 46 > shdr.len() {
                continue;
            }
            let s = &shdr[si * 46..];
            let (k, v) = (range(&iz, KEY_RANGE), range(&iz, VEL_RANGE));
            let (lokey, hikey) = (k.0.max(pk.0), k.1.min(pk.1));
            let (lovel, hivel) = (v.0.max(pv.0), v.1.min(pv.1));
            if lokey > hikey || lovel > hivel {
                continue;
            }
            let ia = |op: u16| get(&iz, op).map(amount).unwrap_or(0) + padd(op);
            let start = u32le(s, 20) as i64 + ia(0) as i64 + 32_768 * ia(4) as i64;
            let end = u32le(s, 24) as i64 + ia(1) as i64 + 32_768 * ia(12) as i64;
            let ls = u32le(s, 28) as i64 + ia(2) as i64 + 32_768 * ia(45) as i64;
            let le = u32le(s, 32) as i64 + ia(3) as i64 + 32_768 * ia(50) as i64;
            let rate = u32le(s, 36);
            if end <= start || end as u64 > pool_frames as u64 || rate == 0 {
                continue;
            }
            let orig = s[40] as f32;
            let corr = s[41] as i8 as f32;
            let stype = u16le(s, 44);
            let root = get(&iz, 58)
                .map(amount)
                .filter(|r| (0..=127).contains(r))
                .map(|r| r as f32)
                .unwrap_or(orig);
            let mut r = Region::new(SampleRef::Sf2 {
                file: path.to_path_buf(),
                start: start as u32,
                end: end as u32,
                rate,
                loop_start: ls.clamp(start, end) as u32,
                loop_end: le.clamp(start, end) as u32,
            });
            r.lokey = lokey;
            r.hikey = hikey;
            r.lovel = lovel.max(1);
            r.hivel = hivel;
            r.keycenter = root;
            r.tune = 100.0 * ia(51) as f32 + ia(52) as f32 - corr;
            r.keytrack = get(&iz, 56).map(amount).unwrap_or(100) as f32;
            // EMU/FluidSynth convention: attenuation counts 0.4 dB per cB.
            r.volume_db = -0.04 * ia(48) as f32;
            let pan = ia(17) as f32 / 500.0;
            r.pan = match (stype, pan) {
                (4, p) if p == 0.0 => -1.0,
                (2, p) if p == 0.0 => 1.0,
                (_, p) => p.clamp(-1.0, 1.0),
            };
            let modes = get(&iz, 54).map(amount).unwrap_or(0);
            if modes == 1 || modes == 3 {
                r.loop_mode = LoopMode::Loop;
                r.loop_points = Some(((ls - start).max(0) as usize, (le - start).max(0) as usize));
            }
            let sustain_cb = (get(&iz, 37).map(amount).unwrap_or(0) + padd(37)).clamp(0, 1440);
            r.env = Env {
                attack: timecents(&iz, 34, padd(34)),
                hold: timecents(&iz, 35, padd(35)),
                decay: timecents(&iz, 36, padd(36)),
                sustain: 10f32.powf(-(sustain_cb as f32) / 200.0),
                release: timecents(&iz, 38, padd(38)),
            };
            out.push(r);
        }
    }
    Ok(out)
}

/// Decode pool frames `start..end` (mono) with the loop relative to start.
pub fn read_slice(
    path: &Path,
    start: u32,
    end: u32,
    rate: u32,
    loop_start: u32,
    loop_end: u32,
) -> Result<Audio, String> {
    let l = layout(path)?;
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut f = std::fs::File::open(path).map_err(err)?;
    f.seek(SeekFrom::Start(l.smpl_at + start as u64 * 2)).map_err(err)?;
    let mut raw = vec![0u8; (end - start) as usize * 2];
    f.read_exact(&mut raw).map_err(err)?;
    let s: Vec<f32> = raw
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect();
    let lp = (loop_end > loop_start)
        .then(|| ((loop_start - start) as usize, (loop_end - start) as usize));
    Ok(Audio {
        sample_rate: rate,
        channels: vec![s],
        sample_loop: lp,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn list(kind: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
        let mut body = kind.to_vec();
        for p in parts {
            body.extend_from_slice(p);
        }
        chunk(b"LIST", &body)
    }

    fn name20(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.resize(20, 0);
        v
    }

    /// A one-preset SF2: program 52 "Choir", one instrument zone (keys
    /// 48-72, root 60, looped) over a 1 s 220 Hz... sine at 22.05 kHz.
    pub(crate) fn tiny_sf2(path: &Path) {
        let sr = 22_050u32;
        let n = sr as usize;
        let mut pool: Vec<u8> = Vec::new();
        for i in 0..n {
            let v = (0.5 * (std::f32::consts::TAU * 261.63 * i as f32 / sr as f32).sin() * 32767.0) as i16;
            pool.extend_from_slice(&v.to_le_bytes());
        }
        pool.extend(std::iter::repeat_n(0u8, 92)); // 46 zero frames after a sample
        let gen = |op: u16, v: [u8; 2]| {
            let mut b = op.to_le_bytes().to_vec();
            b.extend_from_slice(&v);
            b
        };
        let mut phdr = Vec::new();
        for (name, prog, bag) in [("Choir", 52u16, 0u16), ("EOP", 0, 1)] {
            phdr.extend(name20(name));
            phdr.extend_from_slice(&prog.to_le_bytes());
            phdr.extend_from_slice(&0u16.to_le_bytes());
            phdr.extend_from_slice(&bag.to_le_bytes());
            phdr.extend_from_slice(&[0u8; 12]);
        }
        let pbag = [0u16, 0, 1, 0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let mut pgen = gen(INSTRUMENT, 0i16.to_le_bytes());
        pgen.extend(gen(0, [0, 0]));
        let mut inst = name20("ChoirInst");
        inst.extend_from_slice(&0u16.to_le_bytes());
        inst.extend(name20("EOI"));
        inst.extend_from_slice(&1u16.to_le_bytes());
        let ibag = [0u16, 0, 4, 0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let mut igen = gen(KEY_RANGE, [48, 72]);
        igen.extend(gen(54, 1i16.to_le_bytes())); // loop
        igen.extend(gen(38, (-1200i16).to_le_bytes())); // release 0.5 s
        igen.extend(gen(SAMPLE_ID, 0i16.to_le_bytes()));
        igen.extend(gen(0, [0, 0]));
        let mut shdr = name20("sine");
        for v in [0u32, n as u32, 2205, n as u32 - 2205, sr] {
            shdr.extend_from_slice(&v.to_le_bytes());
        }
        shdr.extend_from_slice(&[60, 0]);
        shdr.extend_from_slice(&0u16.to_le_bytes());
        shdr.extend_from_slice(&1u16.to_le_bytes());
        shdr.extend(name20("EOS"));
        shdr.extend_from_slice(&[0u8; 26]);
        let pdta = list(
            b"pdta",
            &[
                chunk(b"phdr", &phdr),
                chunk(b"pbag", &pbag),
                chunk(b"pmod", &[0u8; 10]),
                chunk(b"pgen", &pgen),
                chunk(b"inst", &inst),
                chunk(b"ibag", &ibag),
                chunk(b"imod", &[0u8; 10]),
                chunk(b"igen", &igen),
                chunk(b"shdr", &shdr),
            ],
        );
        let sdta = list(b"sdta", &[chunk(b"smpl", &pool)]);
        let info = list(b"INFO", &[chunk(b"ifil", &[2, 0, 1, 0])]);
        let mut body = b"sfbk".to_vec();
        body.extend(info);
        body.extend(sdta);
        body.extend(pdta);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, chunk(b"RIFF", &body)).unwrap();
    }

    #[test]
    fn reads_a_preset_into_looped_regions() {
        let p = std::env::temp_dir().join(format!("ccez-sf2-{}/t.sf2", std::process::id()));
        tiny_sf2(&p);
        assert_eq!(presets(&p).unwrap(), vec![(0, 52, "Choir".to_string())]);
        let rs = load_preset(&p, 0, 52).unwrap();
        assert_eq!(rs.len(), 1);
        let r = &rs[0];
        assert_eq!((r.lokey, r.hikey, r.keycenter), (48, 72, 60.0));
        assert_eq!(r.loop_mode, LoopMode::Loop);
        assert!((r.env.release - 0.5).abs() < 1e-3, "{:?}", r.env);
        assert!(load_preset(&p, 0, 1).is_err());
        let SampleRef::Sf2 { start, end, rate, loop_start, loop_end, .. } = r.sample.clone() else {
            panic!()
        };
        let a = read_slice(&p, start, end, rate, loop_start, loop_end).unwrap();
        assert_eq!((a.frames(), a.sample_rate), (22_050, 22_050));
        // A 4 s note from a 1 s looped sample keeps sounding.
        let mut inst = super::super::sampler::SampledInstrument::new(rs, Default::default());
        let (l, _) = inst.render_note(60, 0.8, 4.0, 48_000, 0.0).unwrap();
        assert!(l.len() >= 4 * 48_000);
        let late = &l[3 * 48_000..3 * 48_000 + 4800];
        assert!(late.iter().map(|x| x * x).sum::<f32>() / 4800.0 > 0.01);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
