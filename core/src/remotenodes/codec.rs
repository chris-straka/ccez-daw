//! Wire codec: length-prefixed binary messages over TCP.
//!
//! Teaching note: TCP is a *stream* — no message boundaries. Every frame
//! here is `u32 LE length + payload`, so the reader always knows exactly
//! how many bytes form one message. Inside the payload integers are
//! little-endian fixed-width and audio is `f32` LE triples, i.e. the same
//! layout `bincode` (fixed-int) / `postcard` (raw `f32` LE) would emit for
//! these shapes; the [`Message`] enum is the single seam where a
//! `postcard` derive can replace this hand codec later without touching
//! the transport. Control frames (Hello/Ping/Pong) stay tiny; only
//! Render frames carry audio.
//!
//! ```text
//! frame:  [len:u32 LE][payload len bytes]
//! payload:[tag:u8][body...]
//! tag 1 Hello   {name_len:u32, name}
//! tag 2 Ping    {seq:u64, t_send_ms:u64}
//! tag 3 Pong    {seq:u64, t_send_ms:u64}   (echoes the Ping's stamp)
//! tag 4 RenderRequest  {block:u64, seq:u64, frames:u32,
//!                       n_bufs:u32, bufs[name_len:u32,name,frames f32 LE]*}
//! tag 5 RenderResponse {block:u64, seq:u64, degraded:u8,
//!                       n_bufs:u32, bufs...}
//! tag 6 RenderError    {block:u64, msg_len:u32, msg}
//! ```

use std::collections::BTreeMap;
use std::io::{self, Read, Write};

/// One decoded message on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Hello { name: String },
    Ping { seq: u64, t_send_ms: u64 },
    Pong { seq: u64, t_send_ms: u64 },
    RenderRequest {
        block: u64,
        seq: u64,
        frames: u32,
        inputs: BTreeMap<String, Vec<f32>>,
    },
    RenderResponse {
        block: u64,
        seq: u64,
        degraded: bool,
        outputs: BTreeMap<String, Vec<f32>>,
    },
    RenderError { block: u64, msg: String },
}

#[derive(Debug)]
pub enum CodecError {
    Io(io::Error),
    BadTag(u8),
    Truncated,
    TooLarge(u32),
    Utf8(std::string::FromUtf8Error),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "codec io: {e}"),
            Self::BadTag(t) => write!(f, "unknown message tag {t}"),
            Self::Truncated => write!(f, "frame ended early"),
            Self::TooLarge(n) => write!(f, "frame too large: {n} bytes"),
            Self::Utf8(e) => write!(f, "bad utf8: {e}"),
        }
    }
}

impl std::error::Error for CodecError {}

impl From<io::Error> for CodecError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<std::string::FromUtf8Error> for CodecError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        Self::Utf8(e)
    }
}

/// Hard cap per frame: 256 MiB of f32 is ~67M samples, far above any
/// localhost DSP block; the cap turns a corrupt length into an error
/// instead of an allocation.
pub const MAX_FRAME: u32 = 256 * 1024 * 1024;

struct Writer(Vec<u8>);

impl Writer {
    fn new(tag: u8) -> Self {
        Self(vec![tag])
    }
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn audio(&mut self, samples: &[f32]) {
        self.u32(samples.len() as u32);
        for s in samples {
            self.0.extend_from_slice(&s.to_le_bytes());
        }
    }
    fn bufs(&mut self, bufs: &BTreeMap<String, Vec<f32>>) {
        self.u32(bufs.len() as u32);
        for (name, samples) in bufs {
            self.str(name);
            self.audio(samples);
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if self.buf.len() < n {
            return Err(CodecError::Truncated);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, CodecError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, CodecError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn str(&mut self) -> Result<String, CodecError> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8(self.take(n)?.to_vec())?)
    }
    fn audio(&mut self) -> Result<Vec<f32>, CodecError> {
        let n = self.u32()? as usize;
        let raw = self.take(n * 4)?;
        let mut out = Vec::with_capacity(n);
        for chunk in raw.chunks_exact(4) {
            out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }
    fn bufs(&mut self) -> Result<BTreeMap<String, Vec<f32>>, CodecError> {
        let n = self.u32()? as usize;
        let mut map = BTreeMap::new();
        for _ in 0..n {
            let name = self.str()?;
            let samples = self.audio()?;
            map.insert(name, samples);
        }
        Ok(map)
    }
    fn end(&self) -> Result<(), CodecError> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(CodecError::Truncated)
        }
    }
}

fn encode_payload(msg: &Message) -> Vec<u8> {
    match msg {
        Message::Hello { name } => {
            let mut w = Writer::new(1);
            w.str(name);
            w.0
        }
        Message::Ping { seq, t_send_ms } => {
            let mut w = Writer::new(2);
            w.u64(*seq);
            w.u64(*t_send_ms);
            w.0
        }
        Message::Pong { seq, t_send_ms } => {
            let mut w = Writer::new(3);
            w.u64(*seq);
            w.u64(*t_send_ms);
            w.0
        }
        Message::RenderRequest {
            block,
            seq,
            frames,
            inputs,
        } => {
            let mut w = Writer::new(4);
            w.u64(*block);
            w.u64(*seq);
            w.u32(*frames);
            w.bufs(inputs);
            w.0
        }
        Message::RenderResponse {
            block,
            seq,
            degraded,
            outputs,
        } => {
            let mut w = Writer::new(5);
            w.u64(*block);
            w.u64(*seq);
            w.u8(u8::from(*degraded));
            w.bufs(outputs);
            w.0
        }
        Message::RenderError { block, msg } => {
            let mut w = Writer::new(6);
            w.u64(*block);
            w.str(msg);
            w.0
        }
    }
}

fn decode_payload(payload: &[u8]) -> Result<Message, CodecError> {
    let mut r = Reader::new(payload);
    match r.u8()? {
        1 => {
            let name = r.str()?;
            r.end()?;
            Ok(Message::Hello { name })
        }
        2 => {
            let seq = r.u64()?;
            let t_send_ms = r.u64()?;
            r.end()?;
            Ok(Message::Ping { seq, t_send_ms })
        }
        3 => {
            let seq = r.u64()?;
            let t_send_ms = r.u64()?;
            r.end()?;
            Ok(Message::Pong { seq, t_send_ms })
        }
        4 => {
            let block = r.u64()?;
            let seq = r.u64()?;
            let frames = r.u32()?;
            let inputs = r.bufs()?;
            r.end()?;
            Ok(Message::RenderRequest {
                block,
                seq,
                frames,
                inputs,
            })
        }
        5 => {
            let block = r.u64()?;
            let seq = r.u64()?;
            let degraded = r.u8()? != 0;
            let outputs = r.bufs()?;
            r.end()?;
            Ok(Message::RenderResponse {
                block,
                seq,
                degraded,
                outputs,
            })
        }
        6 => {
            let block = r.u64()?;
            let msg = r.str()?;
            r.end()?;
            Ok(Message::RenderError { block, msg })
        }
        t => Err(CodecError::BadTag(t)),
    }
}

/// Write one length-prefixed frame.
pub fn write_message(w: &mut impl Write, msg: &Message) -> Result<(), CodecError> {
    let payload = encode_payload(msg);
    if payload.len() > MAX_FRAME as usize {
        return Err(CodecError::TooLarge(payload.len() as u32));
    }
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(&payload)?;
    w.flush()?;
    Ok(())
}

/// Read one length-prefixed frame.
pub fn read_message(r: &mut impl Read) -> Result<Message, CodecError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            CodecError::Truncated
        } else {
            CodecError::Io(e)
        }
    })?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(CodecError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).map_err(|_| CodecError::Truncated)?;
    decode_payload(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bufs() -> BTreeMap<String, Vec<f32>> {
        let mut m = BTreeMap::new();
        m.insert("dly".to_string(), vec![0.0, 0.5, -1.25, f32::MIN_POSITIVE]);
        m.insert("mix".to_string(), vec![1.0; 64]);
        m
    }

    #[test]
    fn roundtrip_every_variant() {
        let cases = vec![
            Message::Hello {
                name: "farm-0".to_string(),
            },
            Message::Ping {
                seq: 7,
                t_send_ms: 123456,
            },
            Message::Pong {
                seq: 7,
                t_send_ms: 123456,
            },
            Message::RenderRequest {
                block: 3,
                seq: 42,
                frames: 64,
                inputs: bufs(),
            },
            Message::RenderResponse {
                block: 3,
                seq: 42,
                degraded: true,
                outputs: bufs(),
            },
            Message::RenderError {
                block: 9,
                msg: "boom".to_string(),
            },
        ];
        for msg in cases {
            let mut wire = Vec::new();
            write_message(&mut wire, &msg).unwrap();
            // Length prefix is LE u32 = payload len.
            let len = u32::from_le_bytes(wire[..4].try_into().unwrap()) as usize;
            assert_eq!(len, wire.len() - 4);
            let back = read_message(&mut &wire[..]).unwrap();
            assert_eq!(msg, back);
        }
    }

    #[test]
    fn audio_bits_survive_exactly() {
        // NaN payloads and subnormals must round-trip bit-identically —
        // loopback render-equivalence depends on it.
        let mut m = BTreeMap::new();
        m.insert("x".to_string(), vec![f32::NAN, -0.0, 1e-30, 3.1415927]);
        let msg = Message::RenderRequest {
            block: 0,
            seq: 0,
            frames: 4,
            inputs: m,
        };
        let mut wire = Vec::new();
        write_message(&mut wire, &msg).unwrap();
        let Message::RenderRequest { inputs, .. } = read_message(&mut &wire[..]).unwrap() else {
            panic!("wrong variant");
        };
        let got = &inputs["x"];
        assert!(got[0].is_nan());
        assert_eq!(got[1].to_bits(), (-0.0f32).to_bits());
        assert_eq!(got[2].to_bits(), 1e-30f32.to_bits());
        assert_eq!(got[3].to_bits(), 3.1415927f32.to_bits());
    }

    #[test]
    fn corrupt_tag_and_truncation_error() {
        let mut wire = Vec::new();
        write_message(
            &mut wire,
            &Message::Hello {
                name: "x".to_string(),
            },
        )
        .unwrap();
        wire[4] = 99; // clobber tag
        assert!(matches!(
            read_message(&mut &wire[..]),
            Err(CodecError::BadTag(99))
        ));
        assert!(matches!(
            read_message(&mut &wire[..3]),
            Err(CodecError::Truncated)
        ));
    }
}
