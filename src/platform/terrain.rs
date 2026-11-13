//! The terrain pass's vertices in Rust: 0x4534f0 (one terrain block's grid points projected
//! into the shared vertex buffer) with 0x453740 (one vertex) inside it. Made after the game's
//! code instruction by instruction (all 32-bit integer arithmetic, no x87), the same writes in
//! the same order; the widescreen scale and the ambient occlusion's points as the hook on
//! 0x453740 (`wide::terrain_vertex`) gave them, which is no longer called from here.
//!
//! A block is a 17x17 grid of points with 3 height layers (s8, -128 none) at sect + 0xc. Rows and
//! columns are projected only where they touch a visible 4x4-cell group (the mask's bits 31..16,
//! bit 31 the first group of a group row, rows of groups 4 points apart). Vertex: 8 bytes {sx,
//! sy, z >> 4 (s16), _, flags}; flags 1 = not projected (no vertex, or z < 0x600), else the
//! outcode (2 inside, 4 left, 8 right, 0x10 top, 0x20 bottom).
//!
//! `FCOP_TERRAIN=0` runs the game's code; `FCOP_TERRAIN=check` runs both on every block (ours
//! first, without the AO's points, the buffer put back after it) and logs where the vertex
//! buffers differ.
use super::{log, mem, wide};
use crate::rt::{Cpu, ESP, Memory, call_original};
use crate::ver::Us;
use std::sync::OnceLock;

pub const BLOCK_VERTICES: Us = Us(0x4534f0);
/// Nonzero in split screen.
const SPLIT: Us = Us(0x511f54);
/// The buffer's size: 17 * 17 * 3 vertices of 8 bytes.
const VBUF_LEN: u32 = 867 * 8;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Game,
    Rust,
    Check,
}

fn mode() -> Mode {
    static M: OnceLock<Mode> = OnceLock::new();
    *M.get_or_init(|| match std::env::var("FCOP_TERRAIN").as_deref() {
        Ok("0") => Mode::Game,
        Ok("check") => Mode::Check,
        _ => Mode::Rust,
    })
}

/// Hook of 0x4534f0(sect, vbuf, pos, mask, view) (cdecl).
pub fn block_vertices(c: &mut Cpu) {
    let m = mem();
    let sp = c.r[ESP];
    let a = |k: u32| m.rd32(sp + 4 + 4 * k);
    let (sect, vbuf, pos, mask, view) = (a(0), a(1), a(2), a(3), a(4));
    match mode() {
        Mode::Game => call_original(c, BLOCK_VERTICES.at()),
        Mode::Rust => {
            project_block(m, sect, vbuf, pos, mask, view, true);
            // as its ret leaves it
            c.r[ESP] = sp + 4;
        }
        Mode::Check => {
            let before: Vec<u8> = (0..VBUF_LEN).map(|k| m.rd8(vbuf + k)).collect();
            project_block(m, sect, vbuf, pos, mask, view, false);
            let ours: Vec<u8> = (0..VBUF_LEN).map(|k| m.rd8(vbuf + k)).collect();
            for (k, &b) in before.iter().enumerate() {
                m.wr8(vbuf + k as u32, b);
            }
            call_original(c, BLOCK_VERTICES.at());
            compare(m, vbuf, &ours);
        }
    }
}

fn compare(m: &Memory, vbuf: u32, ours: &[u8]) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SAME: AtomicU32 = AtomicU32::new(0);
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    match (0..VBUF_LEN).find(|&k| m.rd8(vbuf + k) != ours[k as usize]) {
        None => {
            let n = SAME.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                log!("terrain check: {n} blocks the same");
            }
        }
        Some(k) => {
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 20 {
                let v = k & !7;
                let game: Vec<u8> = (0..8).map(|i| m.rd8(vbuf + v + i)).collect();
                log!("terrain check: vertex {} differs: ours {:02x?}, game {:02x?}", v / 8, &ours[v as usize..v as usize + 8], game);
            }
        }
    }
}

/// 0x4534f0: the block's points, rows 0..16, columns left to right, layers 0..2. With `ao`, each
/// projected vertex goes to the ambient occlusion's points (as the hook on 0x453740 did).
fn project_block(m: &Memory, sect: u32, vbuf: u32, pos: u32, mask: u32, view: u32, ao: bool) {
    let rd = |a: u32| m.rd32(a) as i32;
    let (u, cs) = ([rd(view + 0xc), rd(view + 0x10), rd(view + 0x14)], [rd(view + 0x18), rd(view + 0x1c), rd(view + 0x20)]);
    let step = [rd(view), rd(view + 4), rd(view + 8)];
    let mut row = [rd(pos), rd(pos + 4), rd(pos + 8)];
    let k = wide::k();
    let split = m.rd8(SPLIT.at()) != 0;
    let ao = ao && super::ao::depth_wanted();
    let mut hp = sect + 0xc;
    let mut out = vbuf;
    let mut prev = 0u32;
    for r in 0..17u32 {
        let cur = (mask << (r & !3)) & 0xf000_0000;
        let mut bits = cur | prev;
        prev = cur;
        if bits == 0 {
            hp += 51;
            out += 51 * 8;
        } else {
            let [mut x, mut y, mut z] = row;
            let mut pend = 0;
            let mut cols = 17i32;
            'row: loop {
                let n;
                if (bits as i32) < 0 {
                    n = 4;
                    pend = 1;
                } else {
                    n = pend;
                    pend = 0;
                }
                bits <<= 1;
                let mut cnt = n;
                if n > 0 {
                    loop {
                        for _ in 0..3 {
                            m.wr8(out + 7, 1);
                            let h = m.rd8(hp) as i8 as i32;
                            hp += 1;
                            if h != -128 {
                                let vz = z.wrapping_add(u[2].wrapping_mul(h) >> 8);
                                if vz >= 0x600 {
                                    let vx = x.wrapping_add(u[0].wrapping_mul(h) >> 8);
                                    let vy = y.wrapping_add(u[1].wrapping_mul(h) >> 8);
                                    project(m, out, vx, vy, vz, k, split, ao);
                                }
                            }
                            out += 8;
                        }
                        cols -= 1;
                        if cols == 0 {
                            break 'row;
                        }
                        x = x.wrapping_add(cs[0]);
                        y = y.wrapping_add(cs[1]);
                        z = z.wrapping_add(cs[2]);
                        cnt -= 1;
                        if cnt <= 0 {
                            break;
                        }
                    }
                }
                let mut skip = 4 - n;
                if skip == 0 {
                    continue;
                }
                if bits == 0 || skip > cols {
                    skip = cols;
                }
                hp = hp.wrapping_add((3 * skip) as u32);
                out = out.wrapping_add((24 * skip) as u32);
                cols -= skip;
                x = x.wrapping_add(skip.wrapping_mul(cs[0]));
                y = y.wrapping_add(skip.wrapping_mul(cs[1]));
                z = z.wrapping_add(skip.wrapping_mul(cs[2]));
                if cols <= 0 {
                    break;
                }
            }
        }
        for i in 0..3 {
            row[i] = row[i].wrapping_add(step[i]);
        }
    }
}

/// 0x453740(out, x, y, z), z >= 0x600: screen x, y in 2560x1920 units (1280 wide in split
/// screen), z >> 4 and the outcode. Widescreen scales x first (`wide::terrain_vertex`).
#[allow(clippy::too_many_arguments)]
fn project(m: &Memory, out: u32, x: i32, y: i32, z: i32, k: f32, split: bool, ao: bool) {
    let x = if k != 1.0 { (x as f32 * k) as i32 } else { x };
    let q = 0x1000_0000 / z;
    m.wr16(out + 4, (z >> 4) as u16);
    let cx = if split { 0x280 } else { 0x500 };
    let sx = ((x as i64 * q as i64) >> 16) as i32;
    let sx = sx.wrapping_add(cx);
    let sy = 0x3c0i32.wrapping_sub(((y as i64 * q as i64) >> 16) as i32);
    m.wr16(out, sx as u16);
    m.wr16(out + 2, sy as u16);
    let xc = if sx < 0 { 0x34 } else if sx < 2 * cx { 0x32 } else { 0x38 };
    let yc = if sy < 0 { 0x1c } else if sy < 0x780 { 0x0e } else { 0x2c };
    m.wr8(out + 7, xc & yc);
    if ao {
        super::ao::point(sx as i16, sy as i16, (z >> 4) as i16);
    }
}

pub const GROUP_TILES: Us = Us(0x4537f0);
/// The display list's end (records are appended there).
const LIST_END: Us = Us(0x511dc0);
/// Per shape: its 4 corners (vertex indices from the cell's first), and its kind (`& 7`).
const CORNERS: Us = Us(0x48c608);
const KIND: Us = Us(0x48c7c8);
/// The texture scroll of the block (u16), and where scrolled uv pairs go.
const UV_SCROLL: Us = Us(0x4a3ab8);
const UV_SCRATCH: Us = Us(0x4a3198);
/// Texture descriptor of each page; the 16x16 light grid.
const PAGE_DESC: Us = Us(0x50dc20);
const LIGHT: Us = Us(0x4a3078);

/// Hook of 0x4537f0(sect, vbuf, A, B, gofs) (cdecl): the tiles of one 4x4-cell group, walls
/// dropped by the side the camera is on, each tile left as a display list record (0x453930).
pub fn group_tiles(c: &mut Cpu) {
    let m = mem();
    let sp = c.r[ESP];
    let a = |k: u32| m.rd32(sp + 4 + 4 * k);
    let (sect, vbuf, ca, cb, gofs) = (a(0), a(1), a(2) as i32, a(3) as i32, a(4));
    match mode() {
        Mode::Game => call_original(c, GROUP_TILES.at()),
        Mode::Rust => {
            tiles(m, sect, vbuf, ca, cb, gofs);
            c.r[ESP] = sp + 4;
        }
        Mode::Check => {
            let end0 = m.rd32(LIST_END.at());
            let scratch: Vec<u8> = (0..8).map(|k| m.rd8(UV_SCRATCH.at() + k)).collect();
            // the records' memory as it was (the game leaves some bytes as they are)
            let span = 0x38 * 256;
            let before: Vec<u8> = (0..span).map(|k| m.rd8(end0 + k)).collect();
            tiles(m, sect, vbuf, ca, cb, gofs);
            let end1 = m.rd32(LIST_END.at());
            let ours: Vec<u8> = (0..end1 - end0).map(|k| m.rd8(end0 + k)).collect();
            for (k, &b) in before.iter().enumerate() {
                m.wr8(end0 + k as u32, b);
            }
            for (k, &b) in scratch.iter().enumerate() {
                m.wr8(UV_SCRATCH.at() + k as u32, b);
            }
            m.wr32(LIST_END.at(), end0);
            call_original(c, GROUP_TILES.at());
            let end2 = m.rd32(LIST_END.at());
            let game: Vec<u8> = (0..end2 - end0).map(|k| m.rd8(end0 + k)).collect();
            compare_records(&ours, &game);
        }
    }
}

fn compare_records(ours: &[u8], game: &[u8]) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SAME: AtomicU32 = AtomicU32::new(0);
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if ours == game {
        let n = SAME.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            log!("terrain check: {n} groups the same ({} records in the last)", game.len() / 0x38);
        }
    } else if LOGGED.fetch_add(1, Ordering::Relaxed) < 20 {
        let k = ours.iter().zip(game).position(|(a, b)| a != b).unwrap_or(ours.len().min(game.len()));
        let r = k / 0x38 * 0x38;
        let rec = |v: &[u8]| v.get(r..(r + 0x38).min(v.len())).map(|s| format!("{s:02x?}")).unwrap_or_default();
        log!("terrain check: records differ at byte {k} ({} / {} bytes)\n  ours: {}\n  game: {}", ours.len(), game.len(), rec(ours), rec(game));
    }
}

/// 0x4537f0: the group's tiles as one run from its top-left cell's first, cell ends marked by
/// bit 0, cells row by row. Walls (shape kinds 1..4) only when seen from a side they show
/// (bit 11 positive, bit 12 negative distance of the camera to the wall's plane).
fn tiles(m: &Memory, sect: u32, vbuf: u32, ca: i32, cb: i32, gofs: u32) {
    let mut tp = sect + 0x5d0 + (m.rd16(sect + 0x3cc + 2 * gofs) as u32 >> 4);
    let mut c = 0i32;
    let mut ci = gofs;
    let mut d = gofs as i32;
    loop {
        let mut t = m.rd32(tp);
        tp += 4;
        let kind = m.rd8(KIND.at() + ((t >> 15) & 0x7f)) & 7;
        let cell = ci;
        if t & 1 != 0 {
            t &= !1;
            c += 1;
            ci += 1;
            if c & 3 == 0 {
                ci += 0xc;
            }
        }
        let draw = if kind == 0 {
            // ask for the back-face test
            t |= (t >> 11) & 1;
            true
        } else {
            let (row, col) = ((cell & 0xf0) as i32, (cell & 0xf) as i32);
            match kind {
                1 => d = row.wrapping_sub(cb),
                2 => d = (col << 4).wrapping_sub(ca),
                3 => d = row.wrapping_add((col + 1) << 4).wrapping_sub(ca).wrapping_sub(cb),
                4 => d = row.wrapping_sub(col << 4).wrapping_sub(cb).wrapping_add(ca),
                _ => {}
            }
            let (pos, neg) = (t & 0x800 != 0, t & 0x1000 != 0);
            if d > 0 {
                pos
            } else if d < 0 {
                neg
            } else {
                pos && neg
            }
        };
        if draw {
            tile(m, vbuf + 24 * (cell + (cell >> 4)), t, sect, cell);
        }
        if c >= 16 {
            break;
        }
    }
}

/// 0x453930(vbase, t, sect, cell): one tile culled by its corners' outcodes (and, with t bit 0,
/// by facing), else appended to the display list.
fn tile(m: &Memory, vbase: u32, t: u32, sect: u32, cell: u32) {
    let shape = (t >> 15) & 0x7f;
    let k = CORNERS.at() + 4 * shape;
    let v: [u32; 4] = std::array::from_fn(|i| vbase + 8 * m.rd8(k + i as u32) as u32);
    let f: [u8; 4] = std::array::from_fn(|i| m.rd8(v[i] + 7));
    let or = f[0] | f[1] | f[2] | f[3];
    let and = f[0] & f[1] & f[2] & f[3];
    if or & 1 != 0 || (or & 2 == 0 && and != 0) {
        return;
    }
    if t & 1 != 0 && back_facing(m.rd32(v[0]), m.rd32(v[1]), m.rd32(v[2])) {
        return;
    }
    let z = |i: usize| m.rd16(v[i] + 4) as i16 as i32;
    let depth = if v[0] == v[3] {
        let s = z(0).wrapping_add(z(2)).wrapping_add(z(1));
        s.wrapping_add(z(0)).wrapping_add(s.wrapping_mul(4)) >> 4
    } else {
        z(3).wrapping_add(z(2)).wrapping_add(z(1)).wrapping_add(z(0)) >> 2
    };
    let r = m.rd32(LIST_END.at());
    m.wr32(LIST_END.at(), r.wrapping_add(0x38));
    m.wr32(r, depth as u32);
    for (i, &p) in v.iter().enumerate() {
        m.wr32(r + 0x28 + 4 * i as u32, m.rd32(p));
    }
    m.wr8(r + 6, 1);
    let tex = m.rd32(sect + 4).wrapping_add((t >> 21) & 0x7fe);
    let mut uv = m.rd32(sect + 8).wrapping_add(t & 0x7fe);
    let tw = m.rd16(tex) as u32;
    if tw & 0x800 != 0 {
        let s = m.rd16(UV_SCROLL.at());
        for i in 0..4 {
            m.wr16(UV_SCRATCH.at() + 2 * i, m.rd16(uv + 2 * i).wrapping_add(s));
        }
        uv = UV_SCRATCH.at();
    }
    let page = (tw >> 8) & 7;
    let desc = m.rd32(PAGE_DESC.at() + 4 * page);
    m.wr16(r + 8, m.rd16(uv));
    m.wr16(r + 0xa, m.rd16(uv + 2));
    m.wr16(r + 0xc, m.rd16(uv + 4));
    m.wr8(r + 5, ((m.rd32(desc + 0x24) >> 16) & 0xf) as u8);
    m.wr32(r + 0x14, m.rd32(desc + 0x28));
    m.wr8(r + 7, m.rd8(desc + 0x24));
    let mode = tw >> 13;
    let quad = mode & 1 != 0;
    if quad {
        m.wr16(r + 0xe, m.rd16(uv + 6));
    }
    // tw bit 12: semi-transparent (the record's type bit 3)
    let tb = ((tw >> 9) & 8) as u8;
    let shades = m.rd32(desc + 0x30);
    let light = |dr: u32, dc: u32| m.rd8(LIGHT.at() + (((((cell >> 4) + dr) & 15) << 4) | (((cell & 15) + dc) & 15)));
    match mode {
        0 | 1 => {
            m.wr8(r + 4, tb | if quad { 0xa4 } else { 0xa3 });
            m.wr32(r + 0x10, shades);
            m.wr8(r + 0x18, (tw & 0xfc) as u8);
        }
        2 | 3 => {
            m.wr8(r + 4, tb | if quad { 0xc4 } else { 0xc3 });
            m.wr32(r + 0x10, shades);
            let w = tw << 16 | m.rd16(tex + 2) as u32;
            m.wr8(r + 0x18, (((w >> 18) & 0x3f) << 2) as u8);
            m.wr8(r + 0x1c, (((w >> 12) & 0x3f) << 2) as u8);
            m.wr8(r + 0x20, (((w >> 6) & 0x3f) << 2) as u8);
            if quad {
                m.wr8(r + 0x24, ((w & 0x3f) << 2) as u8);
            }
        }
        4 | 5 => {
            m.wr8(r + 4, tb | if quad { 0xd4 } else { 0xd3 });
            m.wr32(r + 0x10, shades.wrapping_add(0x4000));
            let ct = m.rd32(sect);
            // an RGB555 colour, its components << 2, or << 3 with bit 15
            let colour = |at: u32, i: u32| {
                let col = m.rd16(ct.wrapping_add(2 * i)) as u32;
                let s = col >> 15;
                let sh = s + 2;
                let mask = (0x1f_u32 << sh) as u8;
                m.wr8(at, (col >> (8 - s)) as u8 & mask);
                m.wr8(at + 1, (col >> (3 - s)) as u8 & mask);
                m.wr8(at + 2, ((col as u8) << sh) & mask);
            };
            let t1 = m.rd16(tex + 2) as u32;
            colour(r + 0x19, tw & 0xff);
            colour(r + 0x1d, t1 >> 8);
            colour(r + 0x21, t1 & 0xff);
            if quad {
                colour(r + 0x25, m.rd16(tex + 4) as u32 >> 8);
            }
        }
        6 => {
            m.wr8(r + 4, tb | 0xc3);
            m.wr32(r + 0x10, shades);
            let v = tw & 7;
            m.wr8(r + 0x18, if v == 1 { light(0, 1) } else { light(0, 0) });
            m.wr8(r + 0x20, if v == 3 { light(1, 1) } else { light(1, 0) });
            m.wr8(r + 0x1c, if v == 3 || v == 0 { light(0, 1) } else { light(1, 1) });
        }
        _ => {
            m.wr8(r + 4, tb | 0xc4);
            m.wr32(r + 0x10, shades);
            m.wr8(r + 0x18, light(0, 0));
            m.wr8(r + 0x1c, light(0, 1));
            m.wr8(r + 0x20, light(1, 1));
            m.wr8(r + 0x24, light(1, 0));
        }
    }
}

/// 0x453e90: whether three screen points {x, y} (s16 pairs) turn the other way on screen (the
/// sign of their cross product).
fn back_facing(a: u32, b: u32, c: u32) -> bool {
    let (x, y) = (|p: u32| p as u16 as i16 as i32, |p: u32| (p >> 16) as u16 as i16 as i32);
    let cross = (x(b) - x(a)).wrapping_mul(y(c) - y(a)).wrapping_sub((y(b) - y(a)).wrapping_mul(x(c) - x(a)));
    cross < 0
}
