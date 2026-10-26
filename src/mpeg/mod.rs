//! MPEG-1 movies (the game's videos): the program stream is split into its video and audio
//! streams, which `video` (MPEG-1 video) and `mp2` (MPEG-1 Layer II audio) decode.
//!
//! The decoders are a Rust port of pl_mpeg by Dominic Szablewski (MIT licence,
//! https://github.com/phoboslab/pl_mpeg), whose audio part is based on kjmp2 by Martin J.
//! Fiedler; the tables in `tables.rs` are taken from it.

pub mod mp2;
mod tables;
pub mod video;

/// The elementary streams of a program stream and the time stamp of their first packet.
#[derive(Default)]
pub struct Streams {
    pub video: Vec<u8>,
    pub audio: Vec<u8>,
    pub video_start: Option<f64>,
    pub audio_start: Option<f64>,
}

/// Split an MPEG-1 (or MPEG-2) program stream into its first video and first audio stream.
pub fn demux(data: &[u8]) -> Streams {
    let mut s = Streams::default();
    let mut i = 0;
    while let Some(k) = find_start(data, i) {
        let code = data[k + 3];
        i = k + 4;
        match code {
            0xba => {
                // pack header: MPEG-2 ('01' bits) has 10 bytes and stuffing, MPEG-1 8
                if data.get(i).is_some_and(|b| b & 0xc0 == 0x40) {
                    i += 10 + data.get(i + 9).map_or(0, |b| (b & 7) as usize);
                } else {
                    i += 8;
                }
            }
            0xbb..=0xff => {
                let Some(len) = data.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) else { break };
                let end = (i + 2 + len).min(data.len());
                let p = &data[i + 2..end];
                i = end;
                // the first video and the first audio stream
                if code != 0xe0 && code != 0xc0 {
                    continue;
                }
                let (pts, payload) = packet(p);
                let (buf, start) = if code == 0xe0 { (&mut s.video, &mut s.video_start) } else { (&mut s.audio, &mut s.audio_start) };
                if start.is_none() {
                    *start = pts;
                }
                buf.extend_from_slice(payload);
            }
            _ => {}
        }
    }
    s
}

/// The time stamp (seconds) and payload of a packet's body.
fn packet(p: &[u8]) -> (Option<f64>, &[u8]) {
    let pts = |b: &[u8]| -> Option<f64> {
        let b = b.get(..5)?;
        let v = ((b[0] as u64 >> 1) & 7) << 30 | (b[1] as u64) << 22 | (b[2] as u64 >> 1) << 15 | (b[3] as u64) << 7 | b[4] as u64 >> 1;
        Some(v as f64 / 90000.0)
    };
    if p.first().is_some_and(|b| b & 0xc0 == 0x80) {
        // MPEG-2 PES header
        let Some(&hl) = p.get(2) else { return (None, &[]) };
        let body = p.get(3 + hl as usize..).unwrap_or(&[]);
        let t = if p[1] & 0x80 != 0 { pts(&p[3..]) } else { None };
        return (t, body);
    }
    let mut i = 0;
    while p.get(i) == Some(&0xff) {
        i += 1;
    }
    if p.get(i).is_some_and(|b| b & 0xc0 == 0x40) {
        // STD buffer size
        i += 2;
    }
    let Some(&b) = p.get(i) else { return (None, &[]) };
    let (t, skip) = match b >> 4 {
        2 => (pts(&p[i..]), 5),
        3 => (pts(&p[i..]), 10),
        _ => (None, 1),
    };
    (t, p.get(i + skip..).unwrap_or(&[]))
}

/// The position of the next start code (00 00 01 xx) at or after `from`.
fn find_start(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 < data.len() {
        if data[i + 2] > 1 {
            i += 3;
        } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

/// Reads bits from a stream, most significant first; past the end it reads zeros.
pub struct Bits<'a> {
    data: &'a [u8],
    /// position in bits
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0 }
    }

    pub fn has(&self, n: usize) -> bool {
        self.data.len() * 8 - self.pos.min(self.data.len() * 8) >= n
    }

    pub fn ended(&self) -> bool {
        self.pos >= self.data.len() * 8
    }

    pub fn read(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.data.get(self.pos >> 3).copied().unwrap_or(0);
            v = v << 1 | (byte >> (7 - (self.pos & 7))) as u32 & 1;
            self.pos += 1;
        }
        v
    }

    pub fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    pub fn peek(&mut self, n: u32) -> u32 {
        let p = self.pos;
        let v = self.read(n);
        self.pos = p;
        v
    }

    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    pub fn byte_pos(&self) -> usize {
        self.pos >> 3
    }

    pub fn set_byte_pos(&mut self, p: usize) {
        self.pos = p << 3;
    }

    pub fn byte(&self, p: usize) -> Option<u8> {
        self.data.get(p).copied()
    }

    /// The code of the next start code, positioned after it.
    pub fn next_start_code(&mut self) -> Option<u8> {
        self.align();
        let k = find_start(self.data, self.pos >> 3)?;
        self.pos = (k + 4) << 3;
        Some(self.data[k + 3])
    }

    /// Read a variable-length code: `table` is a binary tree of (index of the next pair, value)
    /// pairs, a leaf has index 0 (or -1: an invalid code, value 0).
    pub fn vlc<T: Copy>(&mut self, table: &[(i16, T)]) -> T {
        let mut s = table[self.read(1) as usize];
        while s.0 > 0 {
            s = table[s.0 as usize + self.read(1) as usize];
        }
        s.1
    }
}
