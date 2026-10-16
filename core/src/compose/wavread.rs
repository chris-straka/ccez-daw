//! Read the WAV files sample libraries ship: PCM 8/16/24/32-bit or 32-bit
//! float, mono or stereo (extra channels are dropped), any sample rate,
//! plus the first `smpl` loop if there is one.

/// Decoded audio: one `Vec` per channel (1 or 2), `[-1, 1]` floats.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
    /// `smpl` loop (start, end) in frames, end exclusive.
    pub sample_loop: Option<(usize, usize)>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
    pub fn left(&self) -> &[f32] {
        &self.channels[0]
    }
    pub fn right(&self) -> &[f32] {
        self.channels.last().expect("at least one channel")
    }
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

pub fn decode(bytes: &[u8]) -> Result<Audio, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // tag, channels, rate, bits
    let mut data: Option<&[u8]> = None;
    let mut sample_loop = None;
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32le(bytes, i + 4) as usize;
        let body = &bytes[i + 8..(i + 8 + len).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let mut tag = u16le(body, 0);
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = u16le(body, 24); // WAVE_FORMAT_EXTENSIBLE subformat
                }
                fmt = Some((tag, u16le(body, 2), u32le(body, 4), u16le(body, 14)));
            }
            b"data" => data = Some(body),
            b"smpl" if body.len() >= 36 + 24 && u32le(body, 28) > 0 => {
                let start = u32le(body, 36 + 8) as usize;
                let end = u32le(body, 36 + 12) as usize;
                if end > start {
                    sample_loop = Some((start, end + 1));
                }
            }
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    let (tag, ch, rate, bits) = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    if ch == 0 || rate == 0 {
        return Err(format!("bad format: {ch} channels at {rate} Hz"));
    }
    let width = (bits as usize).div_ceil(8);
    let read: fn(&[u8]) -> f32 = match (tag, bits) {
        (1, 8) => |b| (b[0] as f32 - 128.0) / 128.0,
        (1, 16) => |b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        (1, 24) => |b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        (1, 32) => |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        (3, 32) => |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        _ => return Err(format!("unsupported WAV encoding (tag {tag}, {bits}-bit)")),
    };
    let frame = width * ch as usize;
    let n = data.len() / frame;
    let keep = (ch as usize).min(2);
    let mut channels = vec![Vec::with_capacity(n); keep];
    for f in 0..n {
        for (c, out) in channels.iter_mut().enumerate() {
            let at = f * frame + c * width;
            out.push(read(&data[at..at + width]));
        }
    }
    // smpl ends are inclusive by the spec; some writers (ours included)
    // store the exclusive end. Clamp either way.
    let sample_loop = sample_loop
        .map(|(a, e)| (a, e.min(n)))
        .filter(|&(a, e)| e > a);
    Ok(Audio {
        sample_rate: rate,
        channels,
        sample_loop,
    })
}

pub fn read(path: &std::path::Path) -> Result<Audio, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// 16-bit PCM WAV writer for tests and fixtures (`channels` interleaved).
pub fn encode_pcm16(sample_rate: u32, channels: &[Vec<f32>]) -> Vec<u8> {
    let ch = channels.len() as u16;
    let n = channels.first().map_or(0, Vec::len);
    let data_len = (n * ch as usize * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2 * ch as u32).to_le_bytes());
    out.extend_from_slice(&(2 * ch).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        for c in channels {
            out.extend_from_slice(&((c[i].clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_stereo_round_trips() {
        let l: Vec<f32> = (0..100).map(|i| (i as f32 / 50.0) - 1.0).collect();
        let r: Vec<f32> = l.iter().map(|x| -x * 0.5).collect();
        let a = decode(&encode_pcm16(44_100, &[l.clone(), r.clone()])).unwrap();
        assert_eq!((a.sample_rate, a.channels.len(), a.frames()), (44_100, 2, 100));
        assert!((a.left()[10] - l[10]).abs() < 1e-4);
        assert!((a.right()[10] - r[10]).abs() < 1e-4);
        assert!(decode(b"RIFF....WAVEjunk").is_err());
    }

    #[test]
    fn reads_24_bit() {
        let mut b = encode_pcm16(48_000, &[vec![0.0; 2]]);
        // Rewrite as 24-bit mono with one frame at half scale.
        b.truncate(36);
        b[32..34].copy_from_slice(&3u16.to_le_bytes()); // block align
        b[34..36].copy_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&[0x00, 0x00, 0x40]);
        let a = decode(&b).unwrap();
        assert!((a.left()[0] - 0.5).abs() < 1e-6, "{:?}", a.left());
    }
}
