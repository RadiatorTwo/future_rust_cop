//! PlayStation lighting for Glide: light brighter than the texture itself.
//!
//! Like the PlayStation, the game lights a textured polygon with a byte per colour where 0x80
//! leaves the texture as it is and more brightens it up to twice (lamps, muzzle flashes,
//! explosions). The software renderer and the PlayStation show that; the Glide walker
//! (0x4393e0) cuts it off at 1.0:
//! - Gouraud records (kind 0x40 grey, 0x50 RGB: a colour per vertex at +0x18 + 4 * i, grey
//!   in its first byte, RGB in the other three) double the byte and clamp at 255
//!   (`cmp eax, 0xff` at `CLAMPS`). Above 0x80 it sets a flag that draws some blended
//!   records a second time with the excess, opaque ones keep the clamp.
//! - Flat grey textured records (kinds 0x00, 0x10, 0x20) take their brightness from the
//!   record's shade-table pointer +0x10 (the software renderer's row, kind 0x20 adds
//!   (byte +0x18 >> 2) rows of 512 bytes) less the texture's base [table + 0x30], >> 6,
//!   clamped at 255 into the constant colour.
//!
//! On, the clamps' immediates (code fields, `smc.rs`) are 0x7fffffff while the 3D picture is
//! drawn: a vertex colour of up to 2.0 reaches the renderer, whose combine multiplies the
//! texture by it and clamps only the result, as the PlayStation does. The flag is never set
//! then (that pass is the same light). Flat grey textured records become Gouraud records
//! (kind 0x40) with the game's brightness, unclamped, at every vertex; put back afterwards.
//! Flat RGB records (0x30) use their bytes directly (0xff = 1.0) and untextured ones are
//! colours of their own: left as they are, as is the HUD.
//!
//! REG_DWORD `Lighting` in the game's key: 1 PlayStation (default), 0 original;
//! `FCOP_LIGHT=0/1` overrides it.

use crate::ver::Us;
use super::{advapi32, mem};
use std::sync::atomic::{AtomicU8, Ordering};

/// The walker's four `cmp eax, 0xff` (imm32 at +1) of the Gouraud colour bytes.
pub const CLAMPS: [Us; 4] = [Us(0x439826), Us(0x439871), Us(0x4398a1), Us(0x4398d2)];
const CLAMP: u32 = 0xff;
const NO_CLAMP: u32 = 0x7fff_ffff;

/// The textures by number (record +7): +0x30 the base its shade-table pointers count from.
const TEXTURES: Us = Us(0x50dc20);
/// Record +7 up to this is a texture page (beyond: the walker's special cases).
const LAST_PAGE: u8 = 0xc;

/// 0 original, 1 PlayStation, 2 not read yet.
static MODE: AtomicU8 = AtomicU8::new(2);

pub fn enabled() -> bool {
    let e = MODE.load(Ordering::Relaxed);
    if e != 2 {
        return e == 1;
    }
    let on = match std::env::var("FCOP_LIGHT") {
        Ok(v) => v != "0",
        Err(_) => advapi32::value("", "Lighting").is_none_or(|(_, d)| d.first().is_some_and(|&b| b != 0)),
    };
    MODE.store(on as u8, Ordering::Relaxed);
    on
}

/// Preferences' OK: saved, and used from the next picture on.
pub fn set_enabled(on: bool) {
    MODE.store(on as u8, Ordering::Relaxed);
    advapi32::set_value("", "Lighting", 4, &(on as u32).to_le_bytes());
}

/// What `begin` changed in the records.
pub struct Changed {
    recs: Vec<(u32, u8, [u8; 16])>,
}

/// Before the walker draws the 3D picture's records (`(record, vertices)`).
pub fn begin(recs: impl Iterator<Item = (u32, u32)>) -> Option<Changed> {
    if !enabled() {
        return None;
    }
    let m = mem();
    for c in CLAMPS {
        m.wr32(c.at() + 1, NO_CLAMP);
    }
    let mut changed = Vec::new();
    for (r, n) in recs {
        let t = m.rd8(r + 4);
        let kind = t & 0x70;
        let page = m.rd8(r + 7);
        if t & 0x80 == 0 || kind > 0x20 || page > LAST_PAGE {
            continue;
        }
        // the walker's brightness (0x4399e0), before its clamp
        let mut p = m.rd32(r + 0x10);
        if kind == 0x20 {
            p = p.wrapping_add((m.rd8(r + 0x18) as u32 >> 2) << 9);
        }
        let base = m.rd32(m.rd32(TEXTURES.at() + 4 * page as u32) + 0x30);
        let i = (p.wrapping_sub(base) >> 6) & 0x3ff_fff8;
        // a Gouraud byte is doubled
        let g = (i / 2).min(0xff) as u8;
        let mut old = [0; 16];
        for (k, b) in old.iter_mut().enumerate() {
            *b = m.rd8(r + 0x18 + k as u32);
        }
        changed.push((r, t & 0x70, old));
        m.wr8(r + 4, (t & !0x70) | 0x40);
        for k in 0..n {
            m.wr8(r + 0x18 + 4 * k, g);
        }
    }
    Some(Changed { recs: changed })
}

/// After it: the clamps and the records as they were (the walker's own changes kept).
pub fn end(c: Changed) {
    let m = mem();
    for a in CLAMPS {
        m.wr32(a.at() + 1, CLAMP);
    }
    for (r, kind, old) in c.recs {
        let t = m.rd8(r + 4);
        m.wr8(r + 4, (t & !0x70) | kind);
        for (k, b) in old.iter().enumerate() {
            m.wr8(r + 0x18 + k as u32, *b);
        }
    }
}
