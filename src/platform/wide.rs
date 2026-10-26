//! Widescreen (Hor+) in Glide missions. The game projects into a fixed 4:3 screen of 2560x1920
//! units (`sx = 1280 + x * 4096 / z`). Narrowing camera-space x by k = (4/3) / aspect before
//! its projections and view tests puts the wider view into that screen; the renderer
//! stretches the 3D picture over the whole wide frame and draws the HUD in 4:3 parts
//! (`walker.rs`). Target markers are drawn at HUD level but belong to the 3D picture, so they
//! are moved back to their place (`to_screen`).
//!
//! The aspect follows the window (set at every frame shown, `set_aspect`); `FCOP_WIDE=0`
//! turns it off, `FCOP_WIDE=<aspect>` forces one.

use super::mem;
use crate::rt::{Cpu, ESP, call_original};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

pub const TERRAIN_VERTEX: u32 = 0x453740;
pub const SPRITE: u32 = 0x457140;
pub const POINTS: u32 = 0x457340;
pub const TERRAIN_VISIBLE: u32 = 0x452fd0;
pub const OBJECT_VISIBLE: u32 = 0x4572a0;
pub const MARKERS: u32 = 0x435330;

/// The display list's bump pointer (next free 0x38-byte record).
const LIST_TOP: u32 = 0x511dc0;
/// Nonzero in split screen (each view is 1280 units wide).
const SPLIT: u32 = 0x511f54;
/// The game's resolution index, and its table of modes (20 bytes each: width, height, ...).
const RES_INDEX: u32 = 0x511e58;
const RES_TABLE: u32 = 0x487080;

/// k as f32 bits; 1.0 is off.
static K: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// Records moved by `to_screen` this frame (drawn with the centred HUD).
static PLACED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

pub fn k() -> f32 {
    f32::from_bits(K.load(Ordering::Relaxed))
}

fn setting() -> Option<f32> {
    static S: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *S.get_or_init(|| std::env::var("FCOP_WIDE").ok().and_then(|v| v.parse().ok()))
}

/// The aspect of the frame to fill (None: no 3D frame, 4:3). Returns the aspect used, which
/// the renderer gives its picture. Called between frames.
pub fn set_aspect(a: Option<f32>) -> f32 {
    let a = match (setting(), a) {
        (_, None) => 4.0 / 3.0,
        (Some(f), _) if f == 0.0 => 4.0 / 3.0,
        (Some(f), _) => f,
        (None, Some(a)) => a,
    };
    let a = if super::walker::precise() { a.max(4.0 / 3.0) } else { 4.0 / 3.0 };
    let k = if a > 4.0 / 3.0 + 0.01 { 4.0 / 3.0 / a } else { 1.0 };
    K.store(k.to_bits(), Ordering::Relaxed);
    PLACED.lock().unwrap().clear();
    if k == 1.0 { 4.0 / 3.0 } else { a }
}

fn sx(v: i32) -> i32 {
    (v as f32 * k()) as i32
}

/// 0x453740(out, x, y, z): one terrain vertex (x s32).
pub fn terrain_vertex(c: &mut Cpu) {
    if k() != 1.0 {
        let m = mem();
        let a = c.r[ESP] + 8;
        m.wr32(a, sx(m.rd32(a) as i32) as u32);
    }
    call_original(c, TERRAIN_VERTEX);
}

/// 0x457140(p): a sprite {x, y, z, _, w, h} (s16).
pub fn sprite(c: &mut Cpu) {
    if k() != 1.0 {
        let m = mem();
        let p = m.rd32(c.r[ESP] + 4);
        for a in [p, p + 8] {
            m.wr16(a, sx(m.rd16(a) as i16 as i32) as u16);
        }
    }
    call_original(c, SPRITE);
}

/// 0x457340(points, flags, n): n points {x, y, z, _} (s16, 8 bytes each), projected in place.
pub fn points(c: &mut Cpu) {
    if k() != 1.0 {
        let m = mem();
        let p = m.rd32(c.r[ESP] + 4);
        let n = m.rd32(c.r[ESP] + 12) as i32;
        for i in 0..n.max(0) as u32 {
            m.wr16(p + 8 * i, sx(m.rd16(p + 8 * i) as i16 as i32) as u16);
        }
    }
    call_original(c, POINTS);
}

/// 0x452fd0(p, r): is the sphere at camera-space p {x, y, z} (s32) with radius r inside the
/// 4:3 view? Terrain blocks and cells are culled with it (0x452a20).
pub fn terrain_visible(c: &mut Cpu) {
    if k() == 1.0 {
        return call_original(c, TERRAIN_VISIBLE);
    }
    let m = mem();
    let p = m.rd32(c.r[ESP] + 4);
    let x = m.rd32(p);
    m.wr32(p, sx(x as i32) as u32);
    call_original(c, TERRAIN_VISIBLE);
    m.wr32(p, x);
}

/// 0x4572a0(p, r): the same test for objects, p {x, y, z} s16.
pub fn object_visible(c: &mut Cpu) {
    if k() == 1.0 {
        return call_original(c, OBJECT_VISIBLE);
    }
    let m = mem();
    let p = m.rd32(c.r[ESP] + 4);
    let x = m.rd16(p);
    m.wr16(p, sx(x as i16 as i32) as u16);
    call_original(c, OBJECT_VISIBLE);
    m.wr16(p, x);
}

/// 0x435330 draws the target markers at the projected position of their target, on the HUD
/// level (depth key 0).
pub fn markers(c: &mut Cpu) {
    let m = mem();
    let from = m.rd32(LIST_TOP);
    call_original(c, MARKERS);
    let to = m.rd32(LIST_TOP);
    if k() == 1.0 || to < from || to - from > 0x38 * 256 {
        return;
    }
    // +0 holds the depth key until the list is sorted
    for r in (from..to).step_by(0x38) {
        if m.rd8(r + 4) != 0 && m.rd32(r) == 0 {
            to_screen(r);
        }
    }
}

pub fn placed(r: u32) -> bool {
    PLACED.lock().unwrap().contains(&r)
}

/// The centre of the view in 2560x1920 units (split screen: each view's own coordinates).
fn centre() -> i32 {
    if mem().rd8(SPLIT) != 0 { 640 } else { 1280 }
}

/// The game's resolution in pixels (the space of type-6 records).
pub fn game_px() -> (u32, u32) {
    let m = mem();
    let r = m.rd32(RES_INDEX);
    let (w, h) = if r <= 10 { (m.rd32(RES_TABLE + 20 * r), m.rd32(RES_TABLE + 4 + 20 * r)) } else { (1280, 960) };
    if w < 320 || h < 200 { (1280, 960) } else { (w, h) }
}

/// 2560x1920 units per unit of a record: type 5 sprites are in 640x480 units, type 6 in
/// pixels of the game's resolution.
fn units(n: u8) -> i32 {
    if n == 5 { 4 } else { (2560 / game_px().0 as i32).max(1) }
}

/// A record's extent in 2560x1920 units (x0, y0, x1, y1), if it has one.
pub fn extent(r: u32) -> Option<[i32; 4]> {
    let m = mem();
    let t = m.rd8(r + 4);
    let n = t & 7;
    let xy = |i: u32| (m.rd16(r + 0x28 + 4 * i) as i16 as i32, m.rd16(r + 0x2a + 4 * i) as i16 as i32);
    match n {
        1..=4 => {
            let mut e = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for i in 0..n as u32 {
                let (x, y) = xy(i);
                e = [e[0].min(x), e[1].min(y), e[2].max(x), e[3].max(y)];
            }
            Some(e)
        }
        5 | 6 => {
            let (u, scale) = (units(n), if n == 5 { 2 } else { 1 });
            let (w, h) = if t & 0x80 != 0 {
                (m.rd8(r + 10) as i32 * scale, m.rd8(r + 11) as i32 * scale)
            } else {
                xy(1)
            };
            let (x, y) = xy(0);
            Some([x * u, y * u, (x + w) * u, (y + h) * u])
        }
        _ => None,
    }
}

/// A HUD-level record whose place belongs to the 3D picture (a target marker, the
/// split-screen divider): the HUD is drawn 4:3, so move it from x to cx + (x - cx) / k.
/// Polygons: every vertex; sprites {x, y, w, h}: the centre (the size stays).
pub fn to_screen(r: u32) {
    PLACED.lock().unwrap().push(r);
    let m = mem();
    let t = m.rd8(r + 4);
    let n = t & 7;
    let (k, cx) = (k(), centre());
    let move_x = |x: i32, cx: i32| (cx as f32 + (x - cx) as f32 / k) as i32;
    match n {
        1..=4 => {
            for i in 0..n as u32 {
                let a = r + 0x28 + 4 * i;
                m.wr16(a, move_x(m.rd16(a) as i16 as i32, cx) as u16);
            }
        }
        5 | 6 => {
            let w = if t & 0x80 != 0 { m.rd8(r + 10) as i32 * if n == 5 { 2 } else { 1 } } else { m.rd16(r + 0x2c) as i16 as i32 };
            let x = m.rd16(r + 0x28) as i16 as i32 + w / 2;
            m.wr16(r + 0x28, (move_x(x, cx / units(n)) - w / 2) as u16);
        }
        _ => {}
    }
}
