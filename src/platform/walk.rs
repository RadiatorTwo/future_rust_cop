//! The game's display list walker for Glide (0x4393e0) in Rust, for precise mode (`walker.rs`):
//! the walker's own code took about a quarter of the game thread's time (FCOP_SAMPLE). Made
//! after the game's code instruction by instruction, with its plain path only (precise mode
//! points every resolution at it); the same Glide calls with the same arguments in the same
//! order, the same writes into the records.
//!
//! Its locals live where the game's are, in the guest stack below the walker's return address
//! (S = esp - 0x188 after its prologue): the four Glide vertices handed to the draws (S+0x98,
//! 0x3c bytes each) and the state it keeps from record to record (S+0x20..0x2c). The game
//! leaves some of it unset (vertex fields, the state of the first record), taking what an
//! earlier call left there; so does this.
//!
//! `FCOP_WALK=0` runs the game's walker instead; `FCOP_WALK=check` runs both on every list (ours
//! only keeping its calls, the records and the stack put back after it) and logs where their
//! Glide calls differ.
use super::glide::{self, Call};
use super::{light, log, mem};
use crate::rt::{Cpu, ESP, Memory, call_original};
use crate::ver::Us;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Game,
    Rust,
    Check,
}

fn mode() -> Mode {
    static M: OnceLock<Mode> = OnceLock::new();
    *M.get_or_init(|| match std::env::var("FCOP_WALK").as_deref() {
        Ok("0") => Mode::Game,
        Ok("check") => Mode::Check,
        _ => Mode::Rust,
    })
}

/// The game's resolution index; the modes' widths and heights (20 bytes a mode).
const RES_INDEX: Us = Us(0x511e58);
const MODE_W: Us = Us(0x487080);
const MODE_H: Us = Us(0x487084);
/// Nonzero in split screen.
const SPLIT: Us = Us(0x511f54);
/// TMU start address of each texture page.
const PAGE_ADDR: Us = Us(0x511f00);
/// Texture descriptor of each page (+0x30: the base its shade pointers count from).
const PAGE_DESC: Us = Us(0x50dc20);
/// Texture data of pages 10 and 11 as records above page 13 name them.
const DATA_10: Us = Us(0x4cdc00);
const DATA_11: Us = Us(0x4edc20);

/// The walker's frame: locals from esp - 0x188 (esp: at its return address).
const FRAME: u32 = 0x188;
const COLOR: u32 = 0x20;
const AMODE: u32 = 0x24;
const CMODE: u32 = 0x28;
const BMODE: u32 = 0x2c;
const VERTS: u32 = 0x98;
const VERT: u32 = 0x3c;
// GrVertex fields
const X: u32 = 0;
const Y: u32 = 4;
const Z: u32 = 8;
const R: u32 = 0xc;
const G: u32 = 0x10;
const B: u32 = 0x14;
const OOZ: u32 = 0x18;
const A: u32 = 0x1c;
const OOW: u32 = 0x20;
const SOW: u32 = 0x24;
const TOW: u32 = 0x28;
const OOW0: u32 = 0x2c;

/// grColorCombine, grAlphaCombine and grAlphaBlendFunction by the walker's selectors (its
/// tables at 0x43a1f0, 0x43a204, 0x43a214).
const CC: [[u32; 5]; 5] = [[3, 1, 0, 1, 0], [3, 1, 1, 1, 0], [3, 8, 1, 1, 0], [1, 0, 1, 2, 0], [1, 0, 0, 2, 0]];
const AC: [[u32; 5]; 4] = [[1, 0, 1, 2, 0], [4, 1, 1, 1, 0], [3, 1, 1, 1, 0], [1, 0, 0, 2, 0]];
const BF: [(u32, u32); 4] = [(1, 5), (4, 4), (0, 5), (0, 6)];

/// The walker's call: list at [esp + 4], the whole-screen flag in [esp + 8]'s low byte. Runs
/// the game's code or ours by `FCOP_WALK`.
pub fn call(c: &mut Cpu, walker: u32) {
    let m = mem();
    let sp = c.r[ESP];
    let (list, full) = (m.rd32(sp + 4), m.rd8(sp + 8) != 0);
    match mode() {
        Mode::Game => call_original(c, walker),
        Mode::Rust => {
            walk(m, sp - FRAME, list, full);
            // as the walker's ret leaves it
            c.r[ESP] = sp + 4;
        }
        Mode::Check => {
            // ours first, only keeping its calls; then the records and the stack as they were
            let mut recs = Vec::new();
            let mut r = list;
            while r != 0 {
                recs.push((r, (0..0x38).map(|k| m.rd8(r + k)).collect::<Vec<u8>>()));
                r = m.rd32(r);
            }
            let stack: Vec<u8> = (0..FRAME).map(|k| m.rd8(sp - FRAME + k)).collect();
            glide::trace_begin(true);
            walk(m, sp - FRAME, list, full);
            let ours = glide::trace_end();
            let ours_recs: Vec<Vec<u8>> = recs.iter().map(|(r, _)| (0..0x38).map(|k| m.rd8(r + k)).collect()).collect();
            for (r, b) in &recs {
                for (k, &v) in b.iter().enumerate() {
                    m.wr8(r + k as u32, v);
                }
            }
            for (k, &v) in stack.iter().enumerate() {
                m.wr8(sp - FRAME + k as u32, v);
            }
            glide::trace_begin(false);
            call_original(c, walker);
            let game = glide::trace_end();
            compare(&ours, &game, recs.iter().zip(&ours_recs).any(|((r, _), o)| (0..0x38).any(|k| m.rd8(r + k) != o[k as usize])));
        }
    }
}

/// Logs the first difference of a list's calls (and counts the lists that agree).
fn compare(ours: &[Call], game: &[Call], recs_differ: bool) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SAME: AtomicU32 = AtomicU32::new(0);
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    match ours.iter().zip(game).position(|(a, b)| a != b) {
        None if ours.len() == game.len() && !recs_differ => {
            let n = SAME.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                log!("walk check: {n} lists the same ({} calls in the last)", game.len());
            }
        }
        k => {
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 20 {
                let k = k.unwrap_or(ours.len().min(game.len()));
                log!(
                    "walk check: lists differ at call {k} of {} (ours) / {} (game), records differ: {recs_differ}\n  ours: {:?}\n  game: {:?}",
                    ours.len(),
                    game.len(),
                    ours.get(k),
                    game.get(k)
                );
            }
        }
    }
}

fn wf(m: &Memory, a: u32, v: f32) {
    m.wr32(a, v.to_bits());
}

fn rf(m: &Memory, a: u32) -> f32 {
    f32::from_bits(m.rd32(a))
}

/// The screen's width and height in the game's current mode (1024 rows for 1280x960: the
/// Glide screen it opens for it).
fn screen(m: &Memory) -> (u32, u32) {
    let idx = m.rd32(RES_INDEX.at());
    let w = m.rd32(MODE_W.at() + 20 * idx);
    let h = if idx == 9 { 0x400 } else { m.rd32(MODE_H.at() + 20 * idx) };
    (w, h)
}

/// The walker on the list at `list`, its frame at `s`.
fn walk(m: &Memory, s: u32, list: u32, full: bool) {
    let v = |i: u32, f: u32| s + VERTS + VERT * i + f;
    let (mut w, h) = screen(m);
    if m.rd8(SPLIT.at()) != 0 && !full {
        w /= 2;
    }
    glide::set_clip([0, 0, w as i32, h as i32]);
    // (grTexCombine(0, LOCAL, ZERO, LOCAL, ZERO): what our Glide always does)
    for i in 0..4 {
        wf(m, v(i, Z), 1.0);
        wf(m, v(i, OOZ), 1.0);
        wf(m, v(i, OOW), 1.0);
        wf(m, v(i, OOW0), 1.0);
        wf(m, v(i, A), 0.0);
    }
    // the clamp of Gouraud colours (light.rs lifts it while the 3D picture is drawn)
    let clamp = m.rd32(light::CLAMPS[0].at() + 1);
    let mut over = false;
    let mut org = 0i32;
    let mut tex = u32::MAX;
    let (mut last_color, mut last_tex, mut last_c, mut last_a, mut last_b) = (0, u32::MAX, u32::MAX, u32::MAX, u32::MAX);
    let mut color = m.rd32(s + COLOR);
    let mut amode = m.rd32(s + AMODE);
    let mut cmode = m.rd32(s + CMODE);
    let mut bmode = m.rd32(s + BMODE);
    let mut r = list;
    loop {
        'record: {
            let t = m.rd8(r + 4);
            if t == 0 {
                break 'record;
            }
            let n = (t & 7) as u32;
            if n != 0 {
                if n == 5 || n == 6 {
                    sprite(m, r, t, n);
                } else {
                    // bounding box: culled (one crossing the screen's edge goes to
                    // guDrawTriangleWithClip, the same as grDrawTriangle in our Glide)
                    let (mut x0, mut y0) = (m.rd16(r + 0x28) as i16 as i32, m.rd16(r + 0x2a) as i16 as i32);
                    let (mut x1, mut y1) = (x0, y0);
                    for i in 1..n {
                        let x = m.rd16(r + 0x28 + 4 * i) as i16 as i32;
                        if x < x0 {
                            x0 = x;
                        } else if x > x1 {
                            x1 = x;
                        }
                        let y = m.rd16(r + 0x2a + 4 * i) as i16 as i32;
                        if y < y0 {
                            y0 = y;
                        } else if y > y1 {
                            y1 = y;
                        }
                    }
                    if y1 < 0 || y0 >= 0x780 || x1 < 0 || x0 >= 0xa00 {
                        break 'record;
                    }
                    if n > 2 && (y0 >= y1 || x0 >= x1) {
                        break 'record;
                    }
                }
                // vertices (the plain path: 2560x1920 units, the view's origin added)
                for i in 0..n.min(4) {
                    let p = r + 0x28 + 4 * i;
                    let (mut x, mut y) = (m.rd16(p) as i16 as i32, m.rd16(p + 2) as i16 as i32);
                    if n > 4 && n != 6 {
                        x <<= 2;
                        y <<= 2;
                    }
                    wf(m, v(i, X), (org / 4 + x) as f32);
                    wf(m, v(i, Y), y as f32);
                    let t = m.rd8(r + 4);
                    if t & 0x80 != 0 {
                        wf(m, v(i, SOW), m.rd8(r + 8 + 2 * i) as f32);
                        wf(m, v(i, TOW), m.rd8(r + 9 + 2 * i) as f32);
                    }
                    let mut shade = |b: u8| {
                        let mut c = 2 * b as u32;
                        if c > clamp {
                            over |= c > 0x100;
                            c = 0xff;
                        }
                        c as f32
                    };
                    match t & 0x70 {
                        0x40 => {
                            let c = shade(m.rd8(r + 0x18 + 4 * i));
                            for f in [R, G, B] {
                                wf(m, v(i, f), c);
                            }
                        }
                        0x50 => {
                            for (k, f) in [R, G, B].into_iter().enumerate() {
                                let c = shade(m.rd8(r + 0x19 + 4 * i + k as u32));
                                wf(m, v(i, f), c);
                            }
                        }
                        _ => {}
                    }
                }
                // state
                let t = m.rd8(r + 4);
                let (kind, semi, mode) = (t & 0x70, t & 8 != 0, m.rd8(r + 6));
                let rgb = |o: u32| m.rd8(r + o) as u32;
                // each byte doubled, its bit 6 into bit 0: the channels can spill into each other
                let rep = |b: u32| b << 1 | (b >> 6) & 1;
                let rep3 = || rep(rgb(0x19)) << 16 | rep(rgb(0x1a)) << 8 | rep(rgb(0x1b));
                if t & 0x80 != 0 {
                    let p = m.rd8(r + 7) as u32;
                    tex = p;
                    if p <= 12 {
                        match kind {
                            0x40 | 0x50 => {
                                color = 0;
                                cmode = 0;
                            }
                            0x30 => {
                                color = if semi && mode == 3 {
                                    (4 * rgb(0x19)).min(255) << 16 | (4 * rgb(0x1a)).min(255) << 8 | (4 * rgb(0x1b)).min(255)
                                } else {
                                    rgb(0x19) << 16 | rgb(0x1a) << 8 | rgb(0x1b)
                                };
                                cmode = 1;
                            }
                            _ => {
                                if kind == 0x20 {
                                    m.wr32(r + 0x10, m.rd32(r + 0x10).wrapping_add((rgb(0x18) >> 2) << 9));
                                }
                                let base = m.rd32(m.rd32(PAGE_DESC.at() + 4 * p) + 0x30);
                                let mut b = (m.rd32(r + 0x10).wrapping_sub(base) >> 6) & 0x3ff_fff8;
                                b = b.min(0xff);
                                b |= (b >> 5) & 7;
                                color = b | b << 8 | b << 16;
                                cmode = 1;
                            }
                        }
                        if semi && mode == 1 {
                            color |= 0x8000_0000;
                            amode = 1;
                        } else {
                            color |= 0xff00_0000;
                            amode = 0;
                        }
                    } else if p == 13 {
                        if n == 6 {
                            // a glyph: its 555 colour
                            let c = m.rd16(r + 0x1a) as u32;
                            let e = ((((c & 0x7c00) << 3 | (c & 0x3e0)) << 3) | (c & 0x1f)) << 3;
                            color = e | (e >> 5) & 0x07_0707 | 0xff00_0000;
                            cmode = 1;
                            amode = 2;
                        } else {
                            color = 0xff00_0001;
                            amode = 0;
                            cmode = 2;
                            for i in 0..4 {
                                wf(m, v(i, TOW), (rf(m, v(i, TOW)) as f64 + 208.0) as f32);
                            }
                        }
                    } else {
                        let q = m.rd32(r + 0x14);
                        tex = if q == DATA_10.at() {
                            10
                        } else if q == DATA_11.at() {
                            11
                        } else {
                            break 'record;
                        };
                        (color, amode) = if t & 8 != 0 { (0x80ff_ffff, 1) } else { (0xffff_ffff, 0) };
                        cmode = 1;
                    }
                    bmode = if semi && mode != 1 { if mode == 3 { 3 } else { 1 } } else { 0 };
                } else {
                    match kind {
                        0x00 | 0x20 | 0x30 => {
                            if semi && mode != 1 {
                                if mode == 3 {
                                    let a = 2 * (rgb(0x19) + 2 * rgb(0x1b)) / 3;
                                    for i in 0..4 {
                                        wf(m, v(i, A), a as f32);
                                    }
                                    (cmode, amode, bmode) = (3, 3, 2);
                                } else {
                                    color = rep3();
                                    (cmode, amode, bmode) = (3, 0, 1);
                                }
                            } else {
                                color = if kind == 0x20 {
                                    // the game's: only red, also in the alpha byte
                                    let e = rgb(0x18) << 17 | ((rgb(0x18) >> 6) & 1) << 16;
                                    e | e.wrapping_shl(8) | e.wrapping_shl(16)
                                } else {
                                    rep3()
                                };
                                color |= if semi { 0x8000_0000 } else { 0xff00_0000 };
                                (cmode, amode, bmode) = (3, 0, 0);
                            }
                        }
                        0x40 | 0x50 => {
                            if semi && mode != 1 {
                                color = 0xff00_0001;
                                if mode == 3 {
                                    for i in 0..n.min(4) {
                                        let a = (rgb(0x19 + 4 * i) + 2 * rgb(0x1b + 4 * i)).min(0xff);
                                        wf(m, v(i, A), a as f32);
                                    }
                                    (cmode, amode, bmode) = (3, 3, 2);
                                } else {
                                    (cmode, amode, bmode) = (4, 0, 1);
                                }
                            } else {
                                color = if semi { 0x8000_0001 } else { 0xff00_0001 };
                                (cmode, amode, bmode) = (4, 0, 0);
                            }
                        }
                        // 0x10, 0x60, 0x70: the state of the record before
                        _ => {}
                    }
                }
                if color != last_color {
                    last_color = color;
                    glide::set_constant(color);
                }
                if tex != last_tex {
                    last_tex = tex;
                    glide::set_tex(m.rd32(PAGE_ADDR.at() + 4 * tex));
                }
                if cmode != last_c {
                    last_c = cmode;
                    if let Some(&cc) = CC.get(cmode as usize) {
                        glide::set_color_combine(cc);
                    }
                }
                if amode != last_a {
                    last_a = amode;
                    if let Some(&ac) = AC.get(amode as usize) {
                        glide::set_alpha_combine(ac);
                    }
                }
                if bmode != last_b {
                    last_b = bmode;
                    if let Some(&bf) = BF.get(bmode as usize) {
                        glide::set_blend(bf);
                    }
                }
            }
            // draw (records with an excess of brightness twice)
            loop {
                let t = m.rd8(r + 4);
                match t & 7 {
                    0 => {
                        if t & 0x1f == 0x10 && !full {
                            let (w, h) = screen(m);
                            let x = m.rd16(r + 0x28) as i16;
                            glide::set_clip(if x > 0 { [(w / 2) as i32, 0, w as i32, h as i32] } else { [0, 0, (w / 2) as i32, h as i32] });
                            org = x as i32 * 4;
                        }
                    }
                    1 => glide::draw(glow::POINTS, &[v(0, 0)]),
                    2 => glide::draw(glow::TRIANGLES, &[v(0, 0), v(1, 0)]),
                    3 => glide::draw(glow::TRIANGLES, &[v(0, 0), v(1, 0), v(2, 0)]),
                    4 => {
                        glide::draw(glow::TRIANGLES, &[v(0, 0), v(1, 0), v(2, 0)]);
                        glide::draw(glow::TRIANGLES, &[v(0, 0), v(2, 0), v(3, 0)]);
                    }
                    _ => {}
                }
                if !over {
                    break;
                }
                over = false;
                if t & 8 != 0 && m.rd8(r + 6) == 2 {
                    // additive: the part above full brightness
                    let excess = |b: u8| (2 * b as i32 - 256).max(0) as f32;
                    for i in 0..n.min(4) {
                        match t & 0x70 {
                            0x40 => {
                                let e = excess(m.rd8(r + 0x18 + 4 * i));
                                for f in [R, G, B] {
                                    wf(m, v(i, f), e);
                                }
                            }
                            0x50 => {
                                for (k, f) in [R, G, B].into_iter().enumerate() {
                                    wf(m, v(i, f), excess(m.rd8(r + 0x19 + 4 * i + k as u32)));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        r = m.rd32(r);
        if r == 0 {
            break;
        }
    }
    m.wr32(s + COLOR, color);
    m.wr32(s + AMODE, amode);
    m.wr32(s + CMODE, cmode);
    m.wr32(s + BMODE, bmode);
}

/// A sprite (type 5: 640x480 units, 6: game pixels) made a quad of type 4 in the record, as
/// the game does: corners and, when textured, their texture coordinates.
fn sprite(m: &Memory, r: u32, t: u8, n: u32) {
    let (w, hh);
    if t & 0x80 != 0 {
        let (u0, v0) = (m.rd8(r + 8) as u32, m.rd8(r + 9) as u32);
        let (tw, th) = (m.rd8(r + 0xa) as u32, m.rd8(r + 0xb) as u32);
        m.wr8(r + 0xe, u0 as u8);
        m.wr8(r + 0xc, (u0 + tw) as u8);
        m.wr8(r + 0xa, (u0 + tw) as u8);
        m.wr8(r + 0xb, v0 as u8);
        m.wr8(r + 0xf, (v0 + th) as u8);
        m.wr8(r + 0xd, (v0 + th) as u8);
        (w, hh) = if n == 5 { (2 * tw, 2 * th) } else { (tw, th) };
    } else {
        (w, hh) = (m.rd16(r + 0x2c) as i16 as i32 as u32, m.rd16(r + 0x2e) as i16 as i32 as u32);
    }
    let (x0, y0) = (m.rd16(r + 0x28) as u32, m.rd16(r + 0x2a) as u32);
    m.wr16(r + 0x34, x0 as u16);
    m.wr16(r + 0x30, x0.wrapping_add(w) as u16);
    m.wr16(r + 0x2c, x0.wrapping_add(w) as u16);
    m.wr16(r + 0x2e, y0 as u16);
    m.wr16(r + 0x36, y0.wrapping_add(hh) as u16);
    m.wr16(r + 0x32, y0.wrapping_add(hh) as u16);
    m.wr8(r + 4, (t & 0xfc) | 4);
}
