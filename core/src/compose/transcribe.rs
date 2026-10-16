//! From AI audio to notes: transcribe a generated take (ACE-Step or any
//! recording) to MIDI and rewrite it as an editable composition.
//!
//! Teaching note: the hard part, hearing every note in a full mix, is done
//! by Spotify's basic-pitch (a small neural network, Apache-2.0, run
//! locally; installed on demand into `~/.cache/ccez-daw/tools` with `uv`).
//! It returns note events in seconds. Everything after is ours:
//!
//! 1. **Grid**: the take's tempo (from [`super::listen`]) and the beat
//!    phase that lines most onsets up with the beat; the downbeat is the
//!    beat-in-bar where the most low notes start.
//! 2. **Clean-up**: quantise to sixteenths, drop ghost notes (quiet or
//!    shorter than a sixteenth).
//! 3. **Roles**: the melody is the top line (highest note per onset above
//!    middle C, made monophonic), the bass the lowest line, and the rest
//!    is folded into one chord per bar by template matching in the key.
//! 4. **Composition**: the melody and chords go through the arranger onto
//!    adaptive layers, so the generated idea becomes notes an agent can
//!    edit, vary and re-orchestrate.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::arrange::{self, Member, Role as ArrRole};
use super::theory::{self, Key};
use super::{Composition, Instrument, LayerDef, Note, Role, SectionDef};

/// One transcribed note, in seconds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteEvent {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    /// 0..1
    pub velocity: f32,
}

/// basic-pitch's `--save-note-events` CSV:
/// `start_time_s,end_time_s,pitch_midi,velocity,pitch_bend...`.
pub fn read_basic_pitch_csv(text: &str) -> Result<Vec<NoteEvent>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 && line.starts_with("start") {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 4 {
            continue;
        }
        let num = |k: usize| f[k].trim().parse::<f64>().map_err(|_| format!("csv line {}: bad number {:?}", i + 1, f[k]));
        let (s, e, p, v) = (num(0)?, num(1)?, num(2)?, num(3)?);
        if e > s && (0.0..=127.0).contains(&p) {
            out.push(NoteEvent { start: s, end: e, pitch: p as u8, velocity: (v / 127.0).clamp(0.0, 1.0) as f32 });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(out)
}

/// A Standard MIDI File (format 0 or 1) to note events in seconds, with
/// its tempo map. Returns the events and the first tempo (BPM) if set.
pub fn read_midi(bytes: &[u8]) -> Result<(Vec<NoteEvent>, Option<f32>), String> {
    let be32 = |b: &[u8], i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let be16 = |b: &[u8], i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
    if bytes.len() < 14 || &bytes[0..4] != b"MThd" {
        return Err("not a MIDI file (no MThd)".into());
    }
    let division = be16(bytes, 12);
    if division & 0x8000 != 0 {
        return Err("SMPTE-timed MIDI files are not supported".into());
    }
    let ppq = division.max(1) as f64;
    let mut pos = 8 + be32(bytes, 4) as usize;
    // (tick, kind): kind 0 = tempo(us per quarter), 1 = on, 2 = off
    let mut tempos: Vec<(u64, u32)> = Vec::new();
    let mut raw: Vec<(u64, bool, u8, u8, u8)> = Vec::new(); // tick, on, chan, pitch, vel
    while pos + 8 <= bytes.len() {
        let len = be32(bytes, pos + 4) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        if &bytes[pos..pos + 4] == b"MTrk" {
            let mut i = 0;
            let mut tick = 0u64;
            let mut status = 0u8;
            let vlq = |i: &mut usize| -> Result<u64, String> {
                let mut v = 0u64;
                for _ in 0..4 {
                    let b = *body.get(*i).ok_or("truncated MIDI track")?;
                    *i += 1;
                    v = (v << 7) | (b & 0x7F) as u64;
                    if b & 0x80 == 0 {
                        return Ok(v);
                    }
                }
                Err("bad variable-length number".into())
            };
            while i < body.len() {
                tick += vlq(&mut i)?;
                let mut b = *body.get(i).ok_or("truncated MIDI event")?;
                if b & 0x80 != 0 {
                    status = b;
                    i += 1;
                } else if status == 0 {
                    return Err("running status without a status byte".into());
                }
                b = status;
                match b {
                    0xFF => {
                        let ty = *body.get(i).ok_or("truncated meta")?;
                        i += 1;
                        let l = vlq(&mut i)? as usize;
                        if ty == 0x51 && l == 3 && i + 3 <= body.len() {
                            tempos.push((tick, u32::from_be_bytes([0, body[i], body[i + 1], body[i + 2]])));
                        }
                        i += l;
                        status = 0;
                    }
                    0xF0 | 0xF7 => {
                        let l = vlq(&mut i)? as usize;
                        i += l;
                        status = 0;
                    }
                    _ => {
                        let kind = b & 0xF0;
                        let ch = b & 0x0F;
                        let n = if matches!(kind, 0xC0 | 0xD0) { 1 } else { 2 };
                        if i + n > body.len() {
                            break;
                        }
                        let (d1, d2) = (body[i], if n == 2 { body[i + 1] } else { 0 });
                        i += n;
                        match kind {
                            0x90 if d2 > 0 => raw.push((tick, true, ch, d1, d2)),
                            0x80 | 0x90 => raw.push((tick, false, ch, d1, 0)),
                            _ => {}
                        }
                    }
                }
            }
        }
        pos += 8 + len;
    }
    tempos.sort();
    if tempos.first().is_none_or(|t| t.0 > 0) {
        tempos.insert(0, (0, 500_000));
    }
    let secs = |tick: u64| -> f64 {
        let mut t = 0.0;
        for (k, w) in tempos.windows(2).map(|w| (w[0], w[1])).enumerate() {
            let _ = k;
            if tick <= w.1 .0 {
                return t + (tick - w.0 .0) as f64 / ppq * w.0 .1 as f64 / 1e6;
            }
            t += (w.1 .0 - w.0 .0) as f64 / ppq * w.0 .1 as f64 / 1e6;
        }
        let last = tempos.last().expect("one tempo");
        t + (tick - last.0) as f64 / ppq * last.1 as f64 / 1e6
    };
    raw.sort_by_key(|r| (r.0, r.1));
    let mut open: std::collections::HashMap<(u8, u8), (u64, u8)> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for (tick, on, ch, p, v) in raw {
        if let Some((t0, v0)) = open.remove(&(ch, p)) {
            if tick > t0 {
                out.push(NoteEvent { start: secs(t0), end: secs(tick), pitch: p, velocity: v0 as f32 / 127.0 });
            }
        }
        if on {
            open.insert((ch, p), (tick, v));
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    let bpm = tempos.iter().find(|t| t.0 == 0).map(|t| 60e6 as f32 / t.1 as f32);
    let explicit = tempos.len() > 1 || tempos[0].1 != 500_000;
    Ok((out, explicit.then_some(bpm).flatten()))
}

/// Where the basic-pitch CLI lives (installed on demand).
pub fn basic_pitch_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CCEZ_TOOLS_DIR") {
        return PathBuf::from(d).join("basic-pitch");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache/ccez-daw/tools/basic-pitch")
}

fn basic_pitch_bin() -> PathBuf {
    if let Ok(b) = std::env::var("CCEZ_BASIC_PITCH") {
        return PathBuf::from(b);
    }
    basic_pitch_dir().join("bin/basic-pitch")
}

/// Install basic-pitch 0.4.0 into its own Python 3.11 venv with `uv`
/// (once; ~400 MB). Errors say what to install when `uv` is missing.
pub fn ensure_basic_pitch() -> Result<PathBuf, String> {
    let bin = basic_pitch_bin();
    if bin.exists() {
        return Ok(bin);
    }
    let dir = basic_pitch_dir();
    eprintln!("ccez-compose: installing basic-pitch 0.4.0 into {} (once)", dir.display());
    let run = |cmd: &mut std::process::Command| -> Result<(), String> {
        let st = cmd.status().map_err(|e| format!("transcription needs `uv` (https://docs.astral.sh/uv/) to install basic-pitch: {e}"))?;
        if st.success() {
            Ok(())
        } else {
            Err(format!("installing basic-pitch failed ({st})"))
        }
    };
    if let Some(p) = dir.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    run(std::process::Command::new("uv").args(["venv", "-q", "--python", "3.11"]).arg(&dir))?;
    // resampy (a basic-pitch dependency) still imports pkg_resources.
    run(std::process::Command::new("uv")
        .args(["pip", "install", "-q", "basic-pitch==0.4.0", "setuptools<81"])
        .env("VIRTUAL_ENV", &dir))?;
    if !bin.exists() {
        return Err(format!("basic-pitch installed but {} is missing", bin.display()));
    }
    Ok(bin)
}

/// Run basic-pitch on `audio` (seconds `from..to`, optional) and return
/// its note events plus the MIDI file it wrote (copied to `midi_out`).
pub fn run_basic_pitch(audio: &Path, from: Option<f64>, to: Option<f64>, midi_out: Option<&Path>) -> Result<Vec<NoteEvent>, String> {
    let bin = ensure_basic_pitch()?;
    let work = std::env::temp_dir().join(format!("ccez-bp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let clip = work.join("take.wav");
    let mut ff = std::process::Command::new("ffmpeg");
    ff.args(["-y", "-loglevel", "error"]);
    if let Some(f) = from {
        ff.args(["-ss", &format!("{f}")]);
    }
    ff.arg("-i").arg(audio);
    if let Some(t) = to {
        ff.args(["-t", &format!("{}", t - from.unwrap_or(0.0))]);
    }
    ff.args(["-ac", "1", "-ar", "22050"]).arg(&clip);
    let st = ff.status().map_err(|e| format!("transcription needs ffmpeg on PATH ({e})"))?;
    if !st.success() {
        return Err(format!("ffmpeg could not read {}", audio.display()));
    }
    let out = std::process::Command::new(&bin)
        .arg("--save-note-events")
        .arg(&work)
        .arg(&clip)
        .output()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!("basic-pitch failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")));
    }
    let csv = std::fs::read_to_string(work.join("take_basic_pitch.csv")).map_err(|e| format!("basic-pitch wrote no note events: {e}"))?;
    if let Some(m) = midi_out {
        if let Some(p) = m.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::copy(work.join("take_basic_pitch.mid"), m).map_err(|e| format!("{}: {e}", m.display()))?;
    }
    let _ = std::fs::remove_dir_all(&work);
    read_basic_pitch_csv(&csv)
}

/// How to turn events into a composition.
#[derive(Debug, Clone)]
pub struct Options {
    pub name: String,
    pub tempo: f32,
    pub beats_per_bar: u32,
    /// Key override (default: detected from the notes).
    pub key: Option<String>,
    /// Bars to keep (default 8) starting at `start_bar` of the take's grid.
    pub bars: u32,
    pub start_bar: u32,
    /// Beats per chord (default: a bar).
    pub every: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transcription {
    pub events: usize,
    pub kept: usize,
    pub tempo: f32,
    /// Seconds from the start of the audio to beat 1 of bar 0.
    pub downbeat_secs: f64,
    pub key: String,
    pub chords: Vec<String>,
    pub melody_notes: usize,
    pub bass_notes: usize,
    pub notes: Vec<String>,
}

fn q(x: f64, grid: f64) -> f64 {
    (x / grid).round() * grid
}

/// Beat phase (seconds) that best lines onsets up with the beat grid.
fn beat_phase(ev: &[NoteEvent], beat: f64) -> f64 {
    let mut best = (f64::MIN, 0.0);
    for k in 0..64 {
        let off = beat * k as f64 / 64.0;
        let s: f64 = ev
            .iter()
            .map(|e| e.velocity as f64 * (std::f64::consts::TAU * (e.start - off) / beat).cos())
            .sum();
        if s > best.0 {
            best = (s, off);
        }
    }
    best.1
}

/// Rewrite transcribed events as a composition: melody, bass, chords on
/// adaptive layers (`calm`: tune, chords, bass; `lively`: a bell double
/// and a harp arpeggio). Returns the composition and what was heard.
pub fn to_composition(events: &[NoteEvent], o: &Options) -> Result<(Composition, Transcription), String> {
    if !(40.0..=300.0).contains(&o.tempo) {
        return Err(format!("tempo {}: want 40..=300 (pass one, or let listen find it)", o.tempo));
    }
    let beat = 60.0 / o.tempo as f64;
    let bpb = o.beats_per_bar.max(1);
    let ev: Vec<&NoteEvent> = events
        .iter()
        .filter(|e| e.velocity >= 0.2 && (e.end - e.start) >= beat / 4.0 * 0.75 && (28..=100).contains(&e.pitch))
        .collect();
    if ev.is_empty() {
        return Err("no usable notes in the transcription (all too quiet or too short)".into());
    }
    let phase = beat_phase(&ev.iter().map(|e| (*e).clone()).collect::<Vec<_>>(), beat);
    // Downbeat: the beat-in-bar where low notes start most.
    let mut weight = vec![0.0f64; bpb as usize];
    for e in &ev {
        let b = ((e.start - phase) / beat).round() as i64;
        let w = if e.pitch < 55 { 2.0 } else { 0.5 } * e.velocity as f64 * (e.end - e.start).min(2.0);
        weight[b.rem_euclid(bpb as i64) as usize] += w;
    }
    let down = weight.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|x| x.0).unwrap_or(0);
    let downbeat_secs = phase + down as f64 * beat;
    let to_beats = |t: f64| (t - downbeat_secs) / beat;
    let start_beat = (o.start_bar * bpb) as f64;
    let total = (o.bars * bpb) as f64;
    // Quantised notes in the window, beats from its start.
    let mut notes: Vec<Note> = ev
        .iter()
        .filter_map(|e| {
            let s = q(to_beats(e.start), 0.25) - start_beat;
            let len = (q(to_beats(e.end) - to_beats(e.start), 0.25)).max(0.25);
            (s >= 0.0 && s < total).then(|| Note { pitch: e.pitch, start: s as f32, length: len.min(total - s) as f32, velocity: e.velocity.clamp(0.3, 1.0) })
        })
        .collect();
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
    notes.dedup_by(|a, b| a.start == b.start && a.pitch == b.pitch);
    if notes.is_empty() {
        return Err(format!("no notes between bar {} and bar {}", o.start_bar, o.start_bar + o.bars));
    }
    // Key from note durations.
    let mut w = [0.0f64; 12];
    for n in &notes {
        w[(n.pitch % 12) as usize] += (n.length as f64) * n.velocity as f64;
    }
    let key = match &o.key {
        Some(k) => Key::parse(k)?,
        None => theory::detect_key_from_weights(&w).first().map(|k| k.0).ok_or("no key")?,
    };
    // Melody: highest note >= C4 per onset, monophonic.
    let mut melody: Vec<Note> = Vec::new();
    let mut t = -1.0f32;
    for n in notes.iter().filter(|n| n.pitch >= 60) {
        if n.start == t {
            continue; // a lower note at the same onset
        }
        t = n.start;
        if let Some(last) = melody.last_mut() {
            if last.start + last.length > n.start {
                last.length = (n.start - last.start).max(0.25);
            }
        }
        melody.push(n.clone());
    }
    // Bass: lowest note < G3 per onset, monophonic.
    let mut bass: Vec<Note> = Vec::new();
    let mut lows: Vec<&Note> = notes.iter().filter(|n| n.pitch < 55).collect();
    lows.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    let mut t = -1.0f32;
    for n in lows {
        if n.start == t {
            continue;
        }
        t = n.start;
        if let Some(last) = bass.last_mut() {
            if last.start + last.length > n.start {
                last.length = (n.start - last.start).max(0.25);
            }
        }
        bass.push(n.clone());
    }
    // Chords: per `every` beats, from everything sounding (bass weighs more).
    let every = o.every.unwrap_or(bpb as f32).max(1.0);
    let n_ch = (total as f32 / every).ceil() as usize;
    let mut chords = Vec::new();
    let mut last_sym = String::new();
    for i in 0..n_ch {
        let (a, b) = (i as f32 * every, (i as f32 + 1.0) * every);
        let mut cw = [0.0f64; 12];
        for n in &notes {
            let ov = (n.start + n.length).min(b) - n.start.max(a);
            if ov > 0.0 {
                cw[(n.pitch % 12) as usize] += ov as f64 * n.velocity as f64 * if n.pitch < 55 { 1.5 } else { 1.0 };
            }
        }
        let sym = theory::chord_from_weights(&cw, &key).map(|c| c.0.symbol).unwrap_or_else(|| last_sym.clone());
        let sym = if sym.is_empty() { theory::pc_name(key.tonic).to_string() } else { sym };
        last_sym = sym.clone();
        chords.push(sym);
    }
    // The composition: one loop, two layers, the arranger's ensemble.
    let mut c = Composition::new(&o.name, o.tempo, bpb);
    c.key = key.name();
    for (id, patch, rev) in [("lead", "flute", 0.3), ("chords", "piano", 0.3), ("low", "cellos_pizz", 0.2), ("bells", "glockenspiel", 0.35), ("ripple", "harp", 0.35)] {
        c.instruments.push(Instrument { id: id.into(), patch: patch.into(), gain_db: 0.0, reverb: rev, pan: None });
    }
    c.layers.push(LayerDef { id: "calm".into(), min_intensity: 1, ..Default::default() });
    c.layers.push(LayerDef { id: "lively".into(), min_intensity: 3, ..Default::default() });
    c.sections.push(SectionDef { id: "main".into(), role: Role::Loop, bars: o.bars, next: None });
    let member = |i: &str, role: ArrRole, layer: &str| Member {
        instrument: i.into(),
        role,
        layer: layer.into(),
        octave: None,
        velocity: None,
        subdivision: None,
        keys: None,
        density: None,
    };
    let req = arrange::Request {
        section: "main".into(),
        key: Some(key.name()),
        melody: melody.clone(),
        fill: false,
        chords: chords.clone(),
        every: Some(every),
        palette: Default::default(),
        ensemble: vec![
            member("lead", ArrRole::Melody, "calm"),
            member("chords", ArrRole::Pad, "calm"),
            member("bells", ArrRole::Sparkle, "lively"),
            member("ripple", ArrRole::Arp, "lively"),
        ],
    };
    let arr = arrange::arrange(&c, &req)?;
    c.parts = arr.parts;
    // The transcribed bass line itself when there is one, else roots.
    let low_range = arrange::range_of(&c, "low").unwrap_or((36, 76));
    let bass_part: Vec<Note> = if bass.len() >= o.bars as usize {
        bass.iter()
            .map(|n| {
                let mut p = n.pitch as i32;
                while p < low_range.0 as i32 {
                    p += 12;
                }
                while p > low_range.1 as i32 {
                    p -= 12;
                }
                Note { pitch: p as u8, velocity: 0.7, ..n.clone() }
            })
            .collect()
    } else {
        let r = arrange::arrange(&c, &arrange::Request { ensemble: vec![member("low", ArrRole::Bass, "calm")], ..req.clone() })?;
        r.parts.into_iter().next().map(|p| p.notes).unwrap_or_default()
    };
    c.parts.push(super::Part { section: "main".into(), layer: "calm".into(), instrument: "low".into(), notes: bass_part });
    c.motifs.insert("transcribed_melody".into(), melody.clone());
    let mut notes_out = arr.notes;
    if melody.len() < 4 {
        notes_out.push("little or no melody above middle C was heard; write one, or lower the split".into());
    }
    let tr = Transcription {
        events: events.len(),
        kept: notes.len(),
        tempo: o.tempo,
        downbeat_secs,
        key: key.name(),
        chords,
        melody_notes: melody.len(),
        bass_notes: bass.len(),
        notes: notes_out,
    };
    Ok((c, tr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: f64, len: f64, pitch: u8) -> NoteEvent {
        NoteEvent { start, end: start + len, pitch, velocity: 0.7 }
    }

    /// A take at 120 BPM that starts 0.13 s in: melody D5 F#5 A5 F#5 per
    /// beat over D (bars 0, 2) and G (bars 1, 3) chords with roots.
    fn take() -> Vec<NoteEvent> {
        let (beat, off) = (0.5, 0.13);
        let mut v = Vec::new();
        for bar in 0..4 {
            let g = bar % 2 == 1;
            let t0 = off + bar as f64 * 4.0 * beat;
            v.push(ev(t0, 4.0 * beat * 0.95, if g { 43 } else { 38 }));
            for (k, p) in (if g { [67u8, 71, 74, 71] } else { [62, 66, 69, 66] }).iter().enumerate() {
                v.push(ev(t0 + k as f64 * beat, beat * 0.9, *p)); // inner chord tone
                v.push(ev(t0 + k as f64 * beat, beat * 0.9, *p + 12)); // tune an octave up
            }
        }
        v
    }

    #[test]
    fn rewrites_a_take_as_a_layered_composition() {
        let o = Options { name: "take".into(), tempo: 120.0, beats_per_bar: 4, key: None, bars: 4, start_bar: 0, every: None };
        let (c, t) = to_composition(&take(), &o).unwrap();
        assert!((t.downbeat_secs - 0.13).abs() < 0.02, "{t:?}");
        assert_eq!(t.key, "D major");
        assert_eq!(t.chords, ["D", "G", "D", "G"]);
        assert_eq!(t.melody_notes, 16);
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        let lead = c.parts.iter().find(|p| p.instrument == "lead").unwrap();
        assert_eq!(lead.notes[0].start, 0.0);
        assert_eq!(lead.notes[0].pitch % 12, 2);
        let low = c.parts.iter().find(|p| p.instrument == "low").unwrap();
        assert_eq!(low.notes.len(), 4);
        assert_eq!(c.layers.len(), 2);
        assert!(c.motifs.contains_key("transcribed_melody"));
    }

    #[test]
    fn reads_basic_pitch_csv_and_midi() {
        let csv = "start_time_s,end_time_s,pitch_midi,velocity,pitch_bend\n0.5,1.0,62,100,0,1\n0.1,0.4,60,64\n";
        let e = read_basic_pitch_csv(csv).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].pitch, 60);
        // Format 0, 96 ppq, tempo 100 BPM (600000 us), one note C4 for a quarter at tick 96.
        let mut trk = vec![0x00, 0xFF, 0x51, 0x03, 0x09, 0x27, 0xC0];
        trk.extend([0x60, 0x90, 60, 90, 0x60, 0x80, 60, 0, 0x00, 0xFF, 0x2F, 0x00]);
        let mut f = b"MThd".to_vec();
        f.extend([0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
        f.extend(b"MTrk");
        f.extend((trk.len() as u32).to_be_bytes());
        f.extend(trk);
        let (ev, bpm) = read_midi(&f).unwrap();
        assert_eq!(ev.len(), 1);
        assert!((bpm.unwrap() - 100.0).abs() < 0.01);
        assert!((ev[0].start - 0.6).abs() < 1e-6 && (ev[0].end - 1.2).abs() < 1e-6, "{ev:?}");
        assert!(read_midi(b"nope").is_err());
    }
}
