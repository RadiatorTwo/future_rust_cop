//! MPEG-1 video (ISO/IEC 11172-2): I, P and B pictures into YCbCr 4:2:0 frames (port of
//! pl_mpeg's plm_video, see mod.rs).

use super::Bits;
use super::tables::*;

const PICTURE_I: u32 = 1;
const PICTURE_P: u32 = 2;
const PICTURE_B: u32 = 3;

const START_SEQUENCE: u8 = 0xb3;
const START_PICTURE: u8 = 0x00;
const START_EXTENSION: u8 = 0xb5;
const START_USER_DATA: u8 = 0xb2;

fn is_slice(c: u8) -> bool {
    (0x01..=0xaf).contains(&c)
}

/// A decoded picture: planes in whole macroblocks (`width`/`height` is the visible part).
#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// luma plane width; the chroma planes are half as wide and high
    pub stride: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
}

impl Frame {
    /// The frame as 0RGB words (BT.601), w x h.
    pub fn to_rgb(&self, out: &mut Vec<u32>) {
        let (w, h) = (self.width & !1, self.height & !1);
        out.clear();
        out.resize(w * h, 0);
        let cw = self.stride / 2;
        let clamp = |v: i32| v.clamp(0, 255) as u32;
        for row in 0..h / 2 {
            for col in 0..w / 2 {
                let c = row * cw + col;
                let cr = self.cr[c] as i32 - 128;
                let cb = self.cb[c] as i32 - 128;
                let r = (cr * 104597) >> 16;
                let g = (cb * 25674 + cr * 53278) >> 16;
                let b = (cb * 132201) >> 16;
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let (x, yy) = (col * 2 + dx, row * 2 + dy);
                    let y = ((self.y[yy * self.stride + x] as i32 - 16) * 76309) >> 16;
                    out[yy * w + x] = 0xff00_0000 | clamp(y + r) << 16 | clamp(y - g) << 8 | clamp(y + b);
                }
            }
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Motion {
    full_px: bool,
    is_set: bool,
    r_size: u32,
    h: i32,
    v: i32,
}

pub struct Decoder<'a> {
    bits: Bits<'a>,
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    mb_width: usize,
    mb_height: usize,
    mb_size: i32,
    frames: [Frame; 3],
    /// the frame being decoded and the two reference frames (indices into `frames`)
    current: usize,
    forward: usize,
    backward: usize,
    start_code: Option<u8>,
    picture_type: u32,
    motion_forward: Motion,
    motion_backward: Motion,
    quantizer_scale: i32,
    slice_begin: bool,
    macroblock_address: i32,
    mb_row: usize,
    mb_col: usize,
    macroblock_type: i16,
    macroblock_intra: bool,
    dc_predictor: [i32; 3],
    block_data: [i32; 64],
    intra_quant_matrix: [u8; 64],
    non_intra_quant_matrix: [u8; 64],
    has_reference_frame: bool,
}

impl<'a> Decoder<'a> {
    /// A decoder for a video elementary stream; None without a sequence header.
    pub fn new(data: &'a [u8]) -> Option<Self> {
        let mut bits = Bits::new(data);
        while bits.next_start_code()? != START_SEQUENCE {}
        let width = bits.read(12) as usize;
        let height = bits.read(12) as usize;
        if width == 0 || height == 0 {
            return None;
        }
        bits.skip(4); // pixel aspect ratio
        let fps = PICTURE_RATE[bits.read(4) as usize];
        bits.skip(18 + 1 + 10 + 1); // bit rate, marker, buffer size, constrained
        let mut intra_quant_matrix = INTRA_QUANT_MATRIX;
        if bits.bit() {
            for &z in &ZIG_ZAG {
                intra_quant_matrix[z as usize] = bits.read(8) as u8;
            }
        }
        let mut non_intra_quant_matrix = [16; 64];
        if bits.bit() {
            for &z in &ZIG_ZAG {
                non_intra_quant_matrix[z as usize] = bits.read(8) as u8;
            }
        }
        let (mb_width, mb_height) = ((width + 15) >> 4, (height + 15) >> 4);
        let stride = mb_width * 16;
        let luma = stride * mb_height * 16;
        let frame = Frame { width, height, stride, y: vec![0; luma], cb: vec![128; luma / 4], cr: vec![128; luma / 4] };
        Some(Decoder {
            bits,
            width,
            height,
            fps: if fps > 0.0 { fps } else { 30.0 },
            mb_width,
            mb_height,
            mb_size: (mb_width * mb_height) as i32,
            frames: [frame.clone(), frame.clone(), frame],
            current: 0,
            forward: 1,
            backward: 2,
            start_code: None,
            picture_type: 0,
            motion_forward: Motion::default(),
            motion_backward: Motion::default(),
            quantizer_scale: 0,
            slice_begin: false,
            macroblock_address: 0,
            mb_row: 0,
            mb_col: 0,
            macroblock_type: 0,
            macroblock_intra: false,
            dc_predictor: [128; 3],
            block_data: [0; 64],
            intra_quant_matrix,
            non_intra_quant_matrix,
            has_reference_frame: false,
        })
    }

    /// The next frame in display order, None at the end.
    pub fn next_frame(&mut self) -> Option<&Frame> {
        let shown = loop {
            if self.start_code != Some(START_PICTURE) {
                self.start_code = self.find_start_code(START_PICTURE);
                if self.start_code.is_none() {
                    // the last reference frame is still to be shown
                    if self.has_reference_frame && matches!(self.picture_type, PICTURE_I | PICTURE_P) {
                        self.has_reference_frame = false;
                        break self.backward;
                    }
                    return None;
                }
            }
            self.decode_picture();
            if self.picture_type == PICTURE_B {
                break self.current;
            } else if self.has_reference_frame {
                break self.forward;
            }
            self.has_reference_frame = true;
        };
        Some(&self.frames[shown])
    }

    fn find_start_code(&mut self, code: u8) -> Option<u8> {
        loop {
            let c = self.bits.next_start_code()?;
            if c == code {
                return Some(c);
            }
        }
    }

    fn decode_picture(&mut self) {
        self.bits.skip(10); // temporal reference
        self.picture_type = self.bits.read(3);
        self.bits.skip(16); // vbv delay
        // D pictures or unknown types
        if !(PICTURE_I..=PICTURE_B).contains(&self.picture_type) {
            self.start_code = None;
            return;
        }
        if self.picture_type != PICTURE_I {
            self.motion_forward.full_px = self.bits.bit();
            let f_code = self.bits.read(3);
            if f_code == 0 {
                self.start_code = None;
                return;
            }
            self.motion_forward.r_size = f_code - 1;
        }
        if self.picture_type == PICTURE_B {
            self.motion_backward.full_px = self.bits.bit();
            let f_code = self.bits.read(3);
            if f_code == 0 {
                self.start_code = None;
                return;
            }
            self.motion_backward.r_size = f_code - 1;
        }
        let reference = self.picture_type != PICTURE_B;
        let old_forward = self.forward;
        if reference {
            self.forward = self.backward;
        }
        // the first slice; extensions and user data are skipped
        loop {
            self.start_code = self.bits.next_start_code();
            if !matches!(self.start_code, Some(START_EXTENSION | START_USER_DATA)) {
                break;
            }
        }
        while let Some(c) = self.start_code.filter(|&c| is_slice(c)) {
            self.decode_slice(c as i32);
            if self.macroblock_address >= self.mb_size - 1 {
                break;
            }
            self.start_code = self.bits.next_start_code();
        }
        if reference {
            self.backward = self.current;
            self.current = old_forward;
        }
    }

    fn decode_slice(&mut self, slice: i32) {
        self.slice_begin = true;
        self.macroblock_address = (slice - 1) * self.mb_width as i32 - 1;
        self.motion_forward.h = 0;
        self.motion_forward.v = 0;
        self.motion_backward.h = 0;
        self.motion_backward.v = 0;
        self.dc_predictor = [128; 3];
        self.quantizer_scale = self.bits.read(5) as i32;
        while self.bits.bit() {
            self.bits.skip(8);
        }
        loop {
            self.decode_macroblock();
            if self.macroblock_address >= self.mb_size - 1 || !self.bits.has(23) || self.bits.peek(23) == 0 {
                break;
            }
        }
    }

    fn decode_macroblock(&mut self) {
        let mut increment = 0;
        let mut t = self.bits.vlc(MACROBLOCK_ADDRESS_INCREMENT);
        // stuffing
        while t == 34 {
            t = self.bits.vlc(MACROBLOCK_ADDRESS_INCREMENT);
        }
        // escape
        while t == 35 {
            increment += 33;
            t = self.bits.vlc(MACROBLOCK_ADDRESS_INCREMENT);
        }
        increment += t as i32;

        if self.slice_begin {
            // the first increment of a slice is relative to the start of the row before
            self.slice_begin = false;
            self.macroblock_address += increment;
        } else {
            if self.macroblock_address + increment >= self.mb_size {
                return;
            }
            if increment > 1 {
                // skipped macroblocks reset the DC predictors, in P pictures the motion too
                self.dc_predictor = [128; 3];
                if self.picture_type == PICTURE_P {
                    self.motion_forward.h = 0;
                    self.motion_forward.v = 0;
                }
            }
            while increment > 1 {
                self.macroblock_address += 1;
                self.set_position();
                self.predict_macroblock();
                increment -= 1;
            }
            self.macroblock_address += 1;
        }
        if self.macroblock_address < 0 {
            return;
        }
        self.set_position();
        if self.mb_col >= self.mb_width || self.mb_row >= self.mb_height {
            return;
        }

        let table = match self.picture_type {
            PICTURE_I => MACROBLOCK_TYPE_INTRA,
            PICTURE_P => MACROBLOCK_TYPE_PREDICTIVE,
            _ => MACROBLOCK_TYPE_B,
        };
        self.macroblock_type = self.bits.vlc(table);
        self.macroblock_intra = self.macroblock_type & 0x01 != 0;
        self.motion_forward.is_set = self.macroblock_type & 0x08 != 0;
        self.motion_backward.is_set = self.macroblock_type & 0x04 != 0;
        if self.macroblock_type & 0x10 != 0 {
            self.quantizer_scale = self.bits.read(5) as i32;
        }
        if self.macroblock_intra {
            self.motion_forward.h = 0;
            self.motion_forward.v = 0;
            self.motion_backward.h = 0;
            self.motion_backward.v = 0;
        } else {
            self.dc_predictor = [128; 3];
            self.decode_motion_vectors();
            self.predict_macroblock();
        }

        let cbp = if self.macroblock_type & 0x02 != 0 {
            self.bits.vlc(CODE_BLOCK_PATTERN)
        } else if self.macroblock_intra {
            0x3f
        } else {
            0
        };
        for block in 0..6 {
            if cbp & (0x20 >> block) != 0 {
                self.decode_block(block);
            }
        }
    }

    fn set_position(&mut self) {
        let a = self.macroblock_address as usize;
        self.mb_row = a / self.mb_width;
        self.mb_col = a % self.mb_width;
    }

    fn decode_motion_vectors(&mut self) {
        if self.motion_forward.is_set {
            let r = self.motion_forward.r_size;
            self.motion_forward.h = self.decode_motion_vector(r, self.motion_forward.h);
            self.motion_forward.v = self.decode_motion_vector(r, self.motion_forward.v);
        } else if self.picture_type == PICTURE_P {
            self.motion_forward.h = 0;
            self.motion_forward.v = 0;
        }
        if self.motion_backward.is_set {
            let r = self.motion_backward.r_size;
            self.motion_backward.h = self.decode_motion_vector(r, self.motion_backward.h);
            self.motion_backward.v = self.decode_motion_vector(r, self.motion_backward.v);
        }
    }

    fn decode_motion_vector(&mut self, r_size: u32, motion: i32) -> i32 {
        let fscale = 1 << r_size;
        let m_code = self.bits.vlc(MOTION) as i32;
        let d = if m_code != 0 && fscale != 1 {
            let r = self.bits.read(r_size) as i32;
            let d = ((m_code.abs() - 1) << r_size) + r + 1;
            if m_code < 0 { -d } else { d }
        } else {
            m_code
        };
        let mut motion = motion + d;
        if motion > (fscale << 4) - 1 {
            motion -= fscale << 5;
        } else if motion < -(fscale << 4) {
            motion += fscale << 5;
        }
        motion
    }

    fn predict_macroblock(&mut self) {
        let mut fw = (self.motion_forward.h, self.motion_forward.v);
        if self.motion_forward.full_px {
            fw = (fw.0 << 1, fw.1 << 1);
        }
        if self.picture_type == PICTURE_B {
            let mut bw = (self.motion_backward.h, self.motion_backward.v);
            if self.motion_backward.full_px {
                bw = (bw.0 << 1, bw.1 << 1);
            }
            if self.motion_forward.is_set {
                self.copy_macroblock(self.forward, fw, false);
                if self.motion_backward.is_set {
                    self.copy_macroblock(self.backward, bw, true);
                }
            } else {
                self.copy_macroblock(self.backward, bw, false);
            }
        } else {
            self.copy_macroblock(self.forward, fw, false);
        }
    }

    /// Predict the current macroblock from frame `src` moved by `motion` (half pixels),
    /// averaged with what is there with `interpolate`.
    fn copy_macroblock(&mut self, src: usize, motion: (i32, i32), interpolate: bool) {
        let (mh, mv) = motion;
        let [a, b, c] = &mut self.frames;
        let (s, d) = match (src, self.current) {
            (0, 1) => (&*a, b),
            (0, 2) => (&*a, c),
            (1, 0) => (&*b, a),
            (1, 2) => (&*b, c),
            (2, 0) => (&*c, a),
            (2, 1) => (&*c, b),
            _ => return,
        };
        let geo = (self.mb_width, self.mb_height, self.mb_row, self.mb_col);
        process_macroblock(geo, &s.y, &mut d.y, mh, mv, 16, interpolate);
        process_macroblock(geo, &s.cr, &mut d.cr, mh / 2, mv / 2, 8, interpolate);
        process_macroblock(geo, &s.cb, &mut d.cb, mh / 2, mv / 2, 8, interpolate);
    }

    fn decode_block(&mut self, block: usize) {
        let mut n = 0;
        let intra = self.macroblock_intra;
        if intra {
            // DC coefficient, predicted
            let plane = block.saturating_sub(3);
            let size = self.bits.vlc(if plane == 0 { DCT_SIZE_LUMINANCE } else { DCT_SIZE_CHROMINANCE }) as u32;
            let predictor = self.dc_predictor[plane];
            self.block_data[0] = if size > 0 {
                let diff = self.bits.read(size) as i32;
                if diff & (1 << (size - 1)) != 0 { predictor + diff } else { predictor + (-(1 << size) | (diff + 1)) }
            } else {
                predictor
            };
            self.dc_predictor[plane] = self.block_data[0];
            // dequantised and premultiplied
            self.block_data[0] <<= 3 + 5;
            n = 1;
        }
        let quant_matrix = if intra { &self.intra_quant_matrix } else { &self.non_intra_quant_matrix };

        // AC coefficients (and the DC of non-intra blocks)
        loop {
            let coeff = self.bits.vlc(DCT_COEFF);
            if coeff == 0x0001 && n > 0 && !self.bits.bit() {
                break; // end of block
            }
            let (run, mut level);
            if coeff == 0xffff {
                // escape
                run = self.bits.read(6) as usize;
                level = self.bits.read(8) as i32;
                if level == 0 {
                    level = self.bits.read(8) as i32;
                } else if level == 128 {
                    level = self.bits.read(8) as i32 - 256;
                } else if level > 128 {
                    level -= 256;
                }
            } else {
                run = (coeff >> 8) as usize;
                level = (coeff & 0xff) as i32;
                if self.bits.bit() {
                    level = -level;
                }
            }
            n += run;
            if n >= 64 {
                return; // invalid
            }
            let z = ZIG_ZAG[n] as usize;
            n += 1;
            // dequantise, make odd, clip
            level <<= 1;
            if !intra {
                level += if level < 0 { -1 } else { 1 };
            }
            level = (level * self.quantizer_scale * quant_matrix[z] as i32) >> 4;
            if level & 1 == 0 {
                level -= if level > 0 { 1 } else { -1 };
            }
            level = level.clamp(-2048, 2047);
            self.block_data[z] = level * PREMULTIPLIER_MATRIX[z] as i32;
        }

        // put the block in its place
        let frame = &mut self.frames[self.current];
        let (plane, dw, mut di) = if block < 4 {
            let dw = frame.stride;
            let mut di = (self.mb_row * dw + self.mb_col) << 4;
            if block & 1 != 0 {
                di += 8;
            }
            if block & 2 != 0 {
                di += dw << 3;
            }
            (&mut frame.y, dw, di)
        } else {
            let dw = frame.stride / 2;
            let di = self.mb_row * 8 * dw + (self.mb_col << 3);
            (if block == 4 { &mut frame.cb } else { &mut frame.cr }, dw, di)
        };
        let s = &mut self.block_data;
        let dc_only = n == 1;
        if dc_only {
            let v = (s[0] + 128) >> 8;
            s[0] = 0;
            for _ in 0..8 {
                for x in 0..8 {
                    let p = &mut plane[di + x];
                    *p = if intra { v } else { *p as i32 + v }.clamp(0, 255) as u8;
                }
                di += dw;
            }
        } else {
            idct(s);
            for y in 0..8 {
                for x in 0..8 {
                    let p = &mut plane[di + x];
                    let v = s[y * 8 + x];
                    *p = if intra { v } else { *p as i32 + v }.clamp(0, 255) as u8;
                }
                di += dw;
            }
            *s = [0; 64];
        }
    }
}

/// Copy (or average in) a block_size square of plane `s` moved by half pixels to `d`.
fn process_macroblock(geo: (usize, usize, usize, usize), s: &[u8], d: &mut [u8], mh: i32, mv: i32, bs: usize, interpolate: bool) {
    let (mb_width, mb_height, mb_row, mb_col) = geo;
    let dw = (mb_width * bs) as i64;
    let (hp, vp) = ((mh >> 1) as i64, (mv >> 1) as i64);
    let (odd_h, odd_v) = (mh & 1 == 1, mv & 1 == 1);
    let si = ((mb_row * bs) as i64 + vp) * dw + (mb_col * bs) as i64 + hp;
    let di = (mb_row as i64 * dw + mb_col as i64) * bs as i64;
    let max = dw * (mb_height * bs - bs + 1) as i64 - bs as i64;
    if si < 0 || si > max || di > max {
        return; // corrupt video
    }
    let (mut si, mut di, dw) = (si as usize, di as usize, dw as usize);
    for _ in 0..bs {
        for x in 0..bs {
            let (a, k) = (si + x, di + x);
            let p = match (odd_h, odd_v) {
                (false, false) => s[a] as u32,
                (false, true) => (s[a] as u32 + s[a + dw] as u32 + 1) >> 1,
                (true, false) => (s[a] as u32 + s[a + 1] as u32 + 1) >> 1,
                (true, true) => (s[a] as u32 + s[a + 1] as u32 + s[a + dw] as u32 + s[a + dw + 1] as u32 + 2) >> 2,
            };
            d[k] = if interpolate { (d[k] as u32 + p + 1) >> 1 } else { p } as u8;
        }
        si += dw;
        di += dw;
    }
}

/// The inverse DCT of premultiplied coefficients, in place.
fn idct(block: &mut [i32; 64]) {
    // one 8-point transform (a column, then a row)
    fn transform(v: [i32; 8]) -> [i32; 8] {
        let b1 = v[4];
        let b3 = v[2] + v[6];
        let b4 = v[5] - v[3];
        let tmp1 = v[1] + v[7];
        let tmp2 = v[3] + v[5];
        let b6 = v[1] - v[7];
        let b7 = tmp1 + tmp2;
        let m0 = v[0];
        let x4 = ((b6 * 473 - b4 * 196 + 128) >> 8) - b7;
        let x0 = x4 - (((tmp1 - tmp2) * 362 + 128) >> 8);
        let x1 = m0 - b1;
        let x2 = (((v[2] - v[6]) * 362 + 128) >> 8) - b3;
        let x3 = m0 + b1;
        let y3 = x1 + x2;
        let y4 = x3 + b3;
        let y5 = x1 - x2;
        let y6 = x3 - b3;
        let y7 = -x0 - ((b4 * 473 + b6 * 196 + 128) >> 8);
        [b7 + y4, x4 + y3, y5 - x0, y6 - y7, y6 + y7, x0 + y5, y3 - x4, y4 - b7]
    }
    for i in 0..8 {
        let col = transform(std::array::from_fn(|k| block[k * 8 + i]));
        for k in 0..8 {
            block[k * 8 + i] = col[k];
        }
    }
    for i in (0..64).step_by(8) {
        let row = transform(std::array::from_fn(|k| block[i + k]));
        for k in 0..8 {
            block[i + k] = (row[k] + 128) >> 8;
        }
    }
}
