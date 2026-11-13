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
