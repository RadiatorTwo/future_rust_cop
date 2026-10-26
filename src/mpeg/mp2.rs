//! MPEG-1 Layer II audio (ISO/IEC 11172-3), 1152 stereo samples per frame (port of pl_mpeg's
//! plm_audio, itself based on kjmp2; see mod.rs).

use super::Bits;
use super::tables::SYNTHESIS_WINDOW;

pub const SAMPLES_PER_FRAME: usize = 1152;

const SAMPLE_RATE: [u32; 3] = [44100, 48000, 32000];
const BIT_RATE: [u32; 14] = [32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384];
const SCALEFACTOR_BASE: [i32; 3] = [0x0200_0000, 0x0196_5fea, 0x0142_8a30];

const MODE_JOINT_STEREO: u32 = 1;
const MODE_MONO: u32 = 3;

// Quantizer lookup, step 1: bitrate classes (mono; stereo, by bitrate per channel)
const QUANT_LUT_STEP_1: [[u8; 14]; 2] = [[0, 0, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2], [0, 0, 0, 0, 0, 0, 1, 1, 1, 2, 2, 2, 2, 2]];

// Step 2: bitrate class, sample rate -> B.2 table (64: high-rate) | sblimit
const TAB_A: u8 = 27 | 64;
const TAB_B: u8 = 30 | 64;
const TAB_C: u8 = 8;
const TAB_D: u8 = 12;
const QUANT_LUT_STEP_2: [[u8; 3]; 3] = [[TAB_C, TAB_C, TAB_D], [TAB_A, TAB_A, TAB_A], [TAB_B, TAB_A, TAB_B]];

// Step 3: B.2 table, subband -> number of allocation bits << 4 | row of step 4
const QUANT_LUT_STEP_3: [[u8; 32]; 2] = [
    [0x44, 0x44, 0x34, 0x34, 0x34, 0x34, 0x34, 0x34, 0x34, 0x34, 0x34, 0x34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [
        0x43, 0x43, 0x43, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x31, 0x31, 0x31, 0x31, 0x31, 0x31, 0x31, 0x31, 0x31, 0x31, 0x31, 0x31, 0x20,
        0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0, 0,
    ],
];

// Step 4: row, allocation value -> quantizer (1-based index into QUANT_TAB, 0: none)
const QUANT_LUT_STEP_4: [[u8; 16]; 6] = [
    [0, 1, 2, 17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 2, 3, 4, 5, 6, 17, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 17],
    [0, 1, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17],
    [0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 17],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
];

#[derive(Clone, Copy)]
struct Quantizer {
    levels: i32,
    /// three samples grouped in one code
    group: bool,
    bits: u32,
}

const fn q(levels: i32, group: bool, bits: u32) -> Quantizer {
    Quantizer { levels, group, bits }
}

const QUANT_TAB: [Quantizer; 17] = [
    q(3, true, 5),
    q(5, true, 7),
    q(7, false, 3),
    q(9, true, 10),
    q(15, false, 4),
    q(31, false, 5),
    q(63, false, 6),
    q(127, false, 7),
    q(255, false, 8),
    q(511, false, 9),
    q(1023, false, 10),
    q(2047, false, 11),
    q(4095, false, 12),
    q(8191, false, 13),
    q(16383, false, 14),
    q(32767, false, 15),
    q(65535, false, 16),
];

pub struct Decoder<'a> {
    bits: Bits<'a>,
    pub sample_rate: u32,
    has_header: bool,
    bitrate_index: usize,
    samplerate_index: usize,
    mode: u32,
    bound: usize,
    v_pos: usize,
    allocation: [[Option<Quantizer>; 32]; 2],
    scale_factor_info: [[u8; 32]; 2],
    scale_factor: [[[i32; 3]; 32]; 2],
    sample: [[[i32; 3]; 32]; 2],
    d: [f32; 1024],
    v: [[f32; 1024]; 2],
    out: Vec<[f32; 2]>,
}

impl<'a> Decoder<'a> {
    /// A decoder for an audio elementary stream; None without a valid first frame.
    pub fn new(data: &'a [u8]) -> Option<Self> {
        let mut d = [0.0; 1024];
        d[..512].copy_from_slice(&SYNTHESIS_WINDOW);
        d[512..].copy_from_slice(&SYNTHESIS_WINDOW);
        let mut dec = Decoder {
            bits: Bits::new(data),
            sample_rate: 0,
            has_header: false,
            bitrate_index: 0,
            samplerate_index: 0,
            mode: 0,
            bound: 0,
            v_pos: 0,
            allocation: [[None; 32]; 2],
            scale_factor_info: [[0; 32]; 2],
            scale_factor: [[[0; 3]; 32]; 2],
            sample: [[[0; 3]; 32]; 2],
            d,
            v: [[0.0; 1024]; 2],
            out: vec![[0.0; 2]; SAMPLES_PER_FRAME],
        };
        dec.decode_header()?;
        dec.bits.set_byte_pos(0);
        dec.sample_rate = SAMPLE_RATE[dec.samplerate_index];
        Some(dec)
    }

    /// The next frame's samples (stereo, -1..1), None at the end.
    pub fn next_frame(&mut self) -> Option<&[[f32; 2]]> {
        while !self.bits.ended() {
            if let Some(size) = self.decode_header() {
                if !self.bits.has(size * 8) {
                    return None;
                }
                self.decode_frame();
                return Some(&self.out);
            }
        }
        None
    }

    /// Find and read a frame header; the size of the frame's data after it in bytes.
    fn decode_header(&mut self) -> Option<usize> {
        if !self.bits.has(48) {
            self.bits.skip(48);
            return None;
        }
        // skip zero bytes
        self.bits.align();
        while self.bits.byte(self.bits.byte_pos()) == Some(0) {
            self.bits.skip(8);
        }
        if self.bits.read(11) != 0x7ff && !self.find_frame_sync() {
            return None;
        }
        let version = self.bits.read(2);
        let layer = self.bits.read(2);
        let has_crc = !self.bits.bit();
        if version != 3 || layer != 2 {
            return None; // not MPEG-1 Layer II
        }
        let bitrate_index = (self.bits.read(4) as usize).wrapping_sub(1);
        if bitrate_index > 13 {
            return None;
        }
        let samplerate_index = self.bits.read(2) as usize;
        if samplerate_index == 3 {
            return None;
        }
        let padding = self.bits.read(1) as usize;
        self.bits.skip(1); // private
        let mode = self.bits.read(2);
        // a different header after the first: a false sync word
        if self.has_header && (self.bitrate_index != bitrate_index || self.samplerate_index != samplerate_index || self.mode != mode) {
            return None;
        }
        self.bitrate_index = bitrate_index;
        self.samplerate_index = samplerate_index;
        self.mode = mode;
        self.has_header = true;
        if mode == MODE_JOINT_STEREO {
            self.bound = ((self.bits.read(2) + 1) << 2) as usize;
        } else {
            self.bits.skip(2);
            self.bound = if mode == MODE_MONO { 0 } else { 32 };
        }
        self.bits.skip(4); // copyright, original, emphasis
        if has_crc {
            self.bits.skip(16);
        }
        let size = (144000 * BIT_RATE[bitrate_index] / SAMPLE_RATE[samplerate_index]) as usize + padding;
        Some(size - if has_crc { 6 } else { 4 })
    }

    /// Move to just after the next sync word (and the MPEG-1 Layer II bits that follow it).
    fn find_frame_sync(&mut self) -> bool {
        let mut i = self.bits.byte_pos();
        while let (Some(a), Some(b)) = (self.bits.byte(i), self.bits.byte(i + 1)) {
            if a == 0xff && b & 0xfe == 0xfc {
                self.bits.set_byte_pos(i + 1);
                self.bits.skip(3);
                return true;
            }
            i += 1;
        }
        self.bits.set_byte_pos(i + 1);
        false
    }

    fn decode_frame(&mut self) {
        let tab1 = if self.mode == MODE_MONO { 0 } else { 1 };
        let tab2 = QUANT_LUT_STEP_1[tab1][self.bitrate_index] as usize;
        let tab3 = QUANT_LUT_STEP_2[tab2][self.samplerate_index];
        let sblimit = (tab3 & 63) as usize;
        let tab3 = (tab3 >> 6) as usize;
        self.bound = self.bound.min(sblimit);

        // allocation
        for sb in 0..self.bound {
            self.allocation[0][sb] = self.read_allocation(sb, tab3);
            self.allocation[1][sb] = self.read_allocation(sb, tab3);
        }
        for sb in self.bound..sblimit {
            let a = self.read_allocation(sb, tab3);
            self.allocation[0][sb] = a;
            self.allocation[1][sb] = a;
        }

        // scale factor selection
        let channels = if self.mode == MODE_MONO { 1 } else { 2 };
        for sb in 0..sblimit {
            for ch in 0..channels {
                if self.allocation[ch][sb].is_some() {
                    self.scale_factor_info[ch][sb] = self.bits.read(2) as u8;
                }
            }
            if self.mode == MODE_MONO {
                self.scale_factor_info[1][sb] = self.scale_factor_info[0][sb];
            }
        }

        // scale factors
        for sb in 0..sblimit {
            for ch in 0..channels {
                if self.allocation[ch][sb].is_some() {
                    let b = &mut self.bits;
                    let sf = &mut self.scale_factor[ch][sb];
                    match self.scale_factor_info[ch][sb] {
                        0 => *sf = [b.read(6) as i32, b.read(6) as i32, b.read(6) as i32],
                        1 => {
                            let a = b.read(6) as i32;
                            *sf = [a, a, b.read(6) as i32];
                        }
                        2 => *sf = [b.read(6) as i32; 3],
                        _ => {
                            let a = b.read(6) as i32;
                            let c = b.read(6) as i32;
                            *sf = [a, c, c];
                        }
                    }
                }
            }
            if self.mode == MODE_MONO {
                self.scale_factor[1][sb] = self.scale_factor[0][sb];
            }
        }

        // samples and synthesis
        let mut out_pos = 0;
        for part in 0..3 {
            for _granule in 0..4 {
                for sb in 0..self.bound {
                    self.read_samples(0, sb, part);
                    self.read_samples(1, sb, part);
                }
                for sb in self.bound..sblimit {
                    self.read_samples(0, sb, part);
                    self.sample[1][sb] = self.sample[0][sb];
                }
                for sb in sblimit..32 {
                    self.sample[0][sb] = [0; 3];
                    self.sample[1][sb] = [0; 3];
                }
                for p in 0..3 {
                    self.v_pos = self.v_pos.wrapping_sub(64) & 1023;
                    for ch in 0..2 {
                        idct36(&self.sample[ch], p, &mut self.v[ch], self.v_pos);
                        let mut u = [0f32; 32];
                        let v = &self.v[ch];
                        let mut d_index = 512 - (self.v_pos >> 1);
                        let mut v_index = (self.v_pos % 128) >> 1;
                        while v_index < 1024 {
                            for x in u.iter_mut() {
                                *x += self.d[d_index] * v[v_index];
                                d_index += 1;
                                v_index += 1;
                            }
                            v_index += 128 - 32;
                            d_index += 64 - 32;
                        }
                        d_index -= 512 - 32;
                        v_index = (128 - 32 + 1024) - v_index;
                        while v_index < 1024 {
                            for x in u.iter_mut() {
                                *x += self.d[d_index] * v[v_index];
                                d_index += 1;
                                v_index += 1;
                            }
                            v_index += 128 - 32;
                            d_index += 64 - 32;
                        }
                        for (j, x) in u.iter().enumerate() {
                            self.out[out_pos + j][ch] = x / -1_090_519_040.0;
                        }
                    }
                    out_pos += 32;
                }
            }
        }
        self.bits.align();
    }

    fn read_allocation(&mut self, sb: usize, tab3: usize) -> Option<Quantizer> {
        let tab4 = QUANT_LUT_STEP_3[tab3][sb];
        let q = QUANT_LUT_STEP_4[(tab4 & 15) as usize][self.bits.read((tab4 >> 4) as u32) as usize];
        (q != 0).then(|| QUANT_TAB[q as usize - 1])
    }

    fn read_samples(&mut self, ch: usize, sb: usize, part: usize) {
        let Some(q) = self.allocation[ch][sb] else {
            self.sample[ch][sb] = [0; 3];
            return;
        };
        let sf = self.scale_factor[ch][sb][part];
        let sf = if sf == 63 {
            0
        } else {
            let shift = sf / 3;
            (SCALEFACTOR_BASE[(sf % 3) as usize] + ((1 << shift) >> 1)) >> shift
        };
        let adj = q.levels;
        let mut s = if q.group {
            let mut val = self.bits.read(q.bits) as i32;
            let a = val % adj;
            val /= adj;
            [a, val % adj, val / adj]
        } else {
            [self.bits.read(q.bits) as i32, self.bits.read(q.bits) as i32, self.bits.read(q.bits) as i32]
        };
        // postmultiply
        let scale = 65536 / (adj + 1);
        let adj = ((adj + 1) >> 1) - 1;
        for x in s.iter_mut() {
            let val = (adj - *x) * scale;
            *x = (val * (sf >> 12) + ((val * (sf & 4095) + 2048) >> 12)) >> 12;
        }
        self.sample[ch][sb] = s;
    }
}

/// The 32-point matrixing of sub-sample `ss` of all subbands into `d[dp..dp + 64]`.
#[allow(unused_assignments, clippy::excessive_precision, clippy::approx_constant, clippy::identity_op)]
fn idct36(s: &[[i32; 3]; 32], ss: usize, d: &mut [f32; 1024], dp: usize) {
    let mut t01: f32 = (s[0][ss] + s[31][ss]) as f32;
    let mut t02: f32 = (s[0][ss] - s[31][ss]) as f32 * 0.500602998235;
    let mut t03: f32 = (s[1][ss] + s[30][ss]) as f32;
    let mut t04: f32 = (s[1][ss] - s[30][ss]) as f32 * 0.505470959898;
    let mut t05: f32 = (s[2][ss] + s[29][ss]) as f32;
    let mut t06: f32 = (s[2][ss] - s[29][ss]) as f32 * 0.515447309923;
    let mut t07: f32 = (s[3][ss] + s[28][ss]) as f32;
    let mut t08: f32 = (s[3][ss] - s[28][ss]) as f32 * 0.53104259109;
    let mut t09: f32 = (s[4][ss] + s[27][ss]) as f32;
    let mut t10: f32 = (s[4][ss] - s[27][ss]) as f32 * 0.553103896034;
    let mut t11: f32 = (s[5][ss] + s[26][ss]) as f32;
    let mut t12: f32 = (s[5][ss] - s[26][ss]) as f32 * 0.582934968206;
    let mut t13: f32 = (s[6][ss] + s[25][ss]) as f32;
    let mut t14: f32 = (s[6][ss] - s[25][ss]) as f32 * 0.622504123036;
    let mut t15: f32 = (s[7][ss] + s[24][ss]) as f32;
    let mut t16: f32 = (s[7][ss] - s[24][ss]) as f32 * 0.674808341455;
    let mut t17: f32 = (s[8][ss] + s[23][ss]) as f32;
    let mut t18: f32 = (s[8][ss] - s[23][ss]) as f32 * 0.744536271002;
    let mut t19: f32 = (s[9][ss] + s[22][ss]) as f32;
    let mut t20: f32 = (s[9][ss] - s[22][ss]) as f32 * 0.839349645416;
    let mut t21: f32 = (s[10][ss] + s[21][ss]) as f32;
    let mut t22: f32 = (s[10][ss] - s[21][ss]) as f32 * 0.972568237862;
    let mut t23: f32 = (s[11][ss] + s[20][ss]) as f32;
    let mut t24: f32 = (s[11][ss] - s[20][ss]) as f32 * 1.16943993343;
    let mut t25: f32 = (s[12][ss] + s[19][ss]) as f32;
    let mut t26: f32 = (s[12][ss] - s[19][ss]) as f32 * 1.48416461631;
    let mut t27: f32 = (s[13][ss] + s[18][ss]) as f32;
    let mut t28: f32 = (s[13][ss] - s[18][ss]) as f32 * 2.05778100995;
    let mut t29: f32 = (s[14][ss] + s[17][ss]) as f32;
    let mut t30: f32 = (s[14][ss] - s[17][ss]) as f32 * 3.40760841847;
    let mut t31: f32 = (s[15][ss] + s[16][ss]) as f32;
    let mut t32: f32 = (s[15][ss] - s[16][ss]) as f32 * 10.1900081235;
    let mut t33: f32 = t01 + t31;
    t31 = (t01 - t31) * 0.502419286188;
    t01 = t03 + t29;
    t29 = (t03 - t29) * 0.52249861494;
    t03 = t05 + t27;
    t27 = (t05 - t27) * 0.566944034816;
    t05 = t07 + t25;
    t25 = (t07 - t25) * 0.64682178336;
    t07 = t09 + t23;
    t23 = (t09 - t23) * 0.788154623451;
    t09 = t11 + t21;
    t21 = (t11 - t21) * 1.06067768599;
    t11 = t13 + t19;
    t19 = (t13 - t19) * 1.72244709824;
    t13 = t15 + t17;
    t17 = (t15 - t17) * 5.10114861869;
    t15 = t33 + t13;
    t13 = (t33 - t13) * 0.509795579104;
    t33 = t01 + t11;
    t01 = (t01 - t11) * 0.601344886935;
    t11 = t03 + t09;
    t09 = (t03 - t09) * 0.899976223136;
    t03 = t05 + t07;
    t07 = (t05 - t07) * 2.56291544774;
    t05 = t15 + t03;
    t15 = (t15 - t03) * 0.541196100146;
    t03 = t33 + t11;
    t11 = (t33 - t11) * 1.30656296488;
    t33 = t05 + t03;
    t05 = (t05 - t03) * 0.707106781187;
    t03 = t15 + t11;
    t15 = (t15 - t11) * 0.707106781187;
    t03 += t15;
    t11 = t13 + t07;
    t13 = (t13 - t07) * 0.541196100146;
    t07 = t01 + t09;
    t09 = (t01 - t09) * 1.30656296488;
    t01 = t11 + t07;
    t07 = (t11 - t07) * 0.707106781187;
    t11 = t13 + t09;
    t13 = (t13 - t09) * 0.707106781187;
    t11 += t13;
    t01 += t11;
    t11 += t07;
    t07 += t13;
    t09 = t31 + t17;
    t31 = (t31 - t17) * 0.509795579104;
    t17 = t29 + t19;
    t29 = (t29 - t19) * 0.601344886935;
    t19 = t27 + t21;
    t21 = (t27 - t21) * 0.899976223136;
    t27 = t25 + t23;
    t23 = (t25 - t23) * 2.56291544774;
    t25 = t09 + t27;
    t09 = (t09 - t27) * 0.541196100146;
    t27 = t17 + t19;
    t19 = (t17 - t19) * 1.30656296488;
    t17 = t25 + t27;
    t27 = (t25 - t27) * 0.707106781187;
    t25 = t09 + t19;
    t19 = (t09 - t19) * 0.707106781187;
    t25 += t19;
    t09 = t31 + t23;
    t31 = (t31 - t23) * 0.541196100146;
    t23 = t29 + t21;
    t21 = (t29 - t21) * 1.30656296488;
    t29 = t09 + t23;
    t23 = (t09 - t23) * 0.707106781187;
    t09 = t31 + t21;
    t31 = (t31 - t21) * 0.707106781187;
    t09 += t31;
    t29 += t09;
    t09 += t23;
    t23 += t31;
    t17 += t29;
    t29 += t25;
    t25 += t09;
    t09 += t27;
    t27 += t23;
    t23 += t19;
    t19 += t31;
    t21 = t02 + t32;
    t02 = (t02 - t32) * 0.502419286188;
    t32 = t04 + t30;
    t04 = (t04 - t30) * 0.52249861494;
    t30 = t06 + t28;
    t28 = (t06 - t28) * 0.566944034816;
    t06 = t08 + t26;
    t08 = (t08 - t26) * 0.64682178336;
    t26 = t10 + t24;
    t10 = (t10 - t24) * 0.788154623451;
    t24 = t12 + t22;
    t22 = (t12 - t22) * 1.06067768599;
    t12 = t14 + t20;
    t20 = (t14 - t20) * 1.72244709824;
    t14 = t16 + t18;
    t16 = (t16 - t18) * 5.10114861869;
    t18 = t21 + t14;
    t14 = (t21 - t14) * 0.509795579104;
    t21 = t32 + t12;
    t32 = (t32 - t12) * 0.601344886935;
    t12 = t30 + t24;
    t24 = (t30 - t24) * 0.899976223136;
    t30 = t06 + t26;
    t26 = (t06 - t26) * 2.56291544774;
    t06 = t18 + t30;
    t18 = (t18 - t30) * 0.541196100146;
    t30 = t21 + t12;
    t12 = (t21 - t12) * 1.30656296488;
    t21 = t06 + t30;
    t30 = (t06 - t30) * 0.707106781187;
    t06 = t18 + t12;
    t12 = (t18 - t12) * 0.707106781187;
    t06 += t12;
    t18 = t14 + t26;
    t26 = (t14 - t26) * 0.541196100146;
    t14 = t32 + t24;
    t24 = (t32 - t24) * 1.30656296488;
    t32 = t18 + t14;
    t14 = (t18 - t14) * 0.707106781187;
    t18 = t26 + t24;
    t24 = (t26 - t24) * 0.707106781187;
    t18 += t24;
    t32 += t18;
    t18 += t14;
    t26 = t14 + t24;
    t14 = t02 + t16;
    t02 = (t02 - t16) * 0.509795579104;
    t16 = t04 + t20;
    t04 = (t04 - t20) * 0.601344886935;
    t20 = t28 + t22;
    t22 = (t28 - t22) * 0.899976223136;
    t28 = t08 + t10;
    t10 = (t08 - t10) * 2.56291544774;
    t08 = t14 + t28;
    t14 = (t14 - t28) * 0.541196100146;
    t28 = t16 + t20;
    t20 = (t16 - t20) * 1.30656296488;
    t16 = t08 + t28;
    t28 = (t08 - t28) * 0.707106781187;
    t08 = t14 + t20;
    t20 = (t14 - t20) * 0.707106781187;
    t08 += t20;
    t14 = t02 + t10;
    t02 = (t02 - t10) * 0.541196100146;
    t10 = t04 + t22;
    t22 = (t04 - t22) * 1.30656296488;
    t04 = t14 + t10;
    t10 = (t14 - t10) * 0.707106781187;
    t14 = t02 + t22;
    t02 = (t02 - t22) * 0.707106781187;
    t14 += t02;
    t04 += t14;
    t14 += t10;
    t10 += t02;
    t16 += t04;
    t04 += t08;
    t08 += t14;
    t14 += t28;
    t28 += t10;
    t10 += t20;
    t20 += t02;
    t21 += t16;
    t16 += t32;
    t32 += t04;
    t04 += t06;
    t06 += t08;
    t08 += t18;
    t18 += t14;
    t14 += t30;
    t30 += t28;
    t28 += t26;
    t26 += t10;
    t10 += t12;
    t12 += t20;
    t20 += t24;
    t24 += t02;
    d[dp + 48] = -t33;
    d[dp + 49] = -t21;
    d[dp + 47] = -t21;
    d[dp + 50] = -t17;
    d[dp + 46] = -t17;
    d[dp + 51] = -t16;
    d[dp + 45] = -t16;
    d[dp + 52] = -t01;
    d[dp + 44] = -t01;
    d[dp + 53] = -t32;
    d[dp + 43] = -t32;
    d[dp + 54] = -t29;
    d[dp + 42] = -t29;
    d[dp + 55] = -t04;
    d[dp + 41] = -t04;
    d[dp + 56] = -t03;
    d[dp + 40] = -t03;
    d[dp + 57] = -t06;
    d[dp + 39] = -t06;
    d[dp + 58] = -t25;
    d[dp + 38] = -t25;
    d[dp + 59] = -t08;
    d[dp + 37] = -t08;
    d[dp + 60] = -t11;
    d[dp + 36] = -t11;
    d[dp + 61] = -t18;
    d[dp + 35] = -t18;
    d[dp + 62] = -t09;
    d[dp + 34] = -t09;
    d[dp + 63] = -t14;
    d[dp + 33] = -t14;
    d[dp + 32] = -t05;
    d[dp + 0] = t05;
    d[dp + 31] = -t30;
    d[dp + 1] = t30;
    d[dp + 30] = -t27;
    d[dp + 2] = t27;
    d[dp + 29] = -t28;
    d[dp + 3] = t28;
    d[dp + 28] = -t07;
    d[dp + 4] = t07;
    d[dp + 27] = -t26;
    d[dp + 5] = t26;
    d[dp + 26] = -t23;
    d[dp + 6] = t23;
    d[dp + 25] = -t10;
    d[dp + 7] = t10;
    d[dp + 24] = -t15;
    d[dp + 8] = t15;
    d[dp + 23] = -t12;
    d[dp + 9] = t12;
    d[dp + 22] = -t19;
    d[dp + 10] = t19;
    d[dp + 21] = -t20;
    d[dp + 11] = t20;
    d[dp + 20] = -t13;
    d[dp + 12] = t13;
    d[dp + 19] = -t24;
    d[dp + 13] = t24;
    d[dp + 18] = -t31;
    d[dp + 14] = t31;
    d[dp + 17] = -t02;
    d[dp + 15] = t02;
    d[dp + 16] = 0.0;
}
