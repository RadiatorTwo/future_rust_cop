//! The PlayStation's look for Glide: texture coloring and colours.
//!
//! Texture coloring: light brighter than the texture itself.
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
//! REG_DWORD `TextureColoring` in the game's key: 1 on (default), 0 the game's clamp;
//! `FCOP_COLORING=0/1` overrides it.
//!
//! PlayStation colors: the PC version recoloured one mission's textures, the first (`M2C`):
//! all 10 pages darker, yellower and more saturated than the PlayStation's (mean difference
//! 9...17 of 255 a page; every other mission's pages are the PlayStation's within 1...5, its
//! rounding; ConFt's pages 6 and 10 differ in content, not colour). A colour matrix fitted to
//! the two (least squares over all M2C texels) brings them within ~6 (the floor is ~3):
//! `M2C_TO_PS`. Its pages are known by their hashes, computed from the player's `missions/M2C`
//! when Glide first downloads; whole downloads of them are recoloured (black stays black: it
//! is keyed out). Seen from the next mission start on. REG_DWORD `PlayStationColors` (default
//! on), `FCOP_PS_COLORS=0/1`. With them the game's own gamma (slider * 0.01 + 1.25) matches the
//! PlayStation's brightness (measured at M2C's start: mean 63, 63, 48 against 62, 60, 47).

use crate::ver::Us;
use super::{advapi32, mem};
use std::collections::HashSet;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// The walker's four `cmp eax, 0xff` (imm32 at +1) of the Gouraud colour bytes.
pub const CLAMPS: [Us; 4] = [Us(0x439826), Us(0x439871), Us(0x4398a1), Us(0x4398d2)];
const CLAMP: u32 = 0xff;
const NO_CLAMP: u32 = 0x7fff_ffff;

/// The textures by number (record +7): +0x30 the base its shade-table pointers count from.
const TEXTURES: Us = Us(0x50dc20);
/// Record +7 up to this is a texture page (beyond: the walker's special cases).
const LAST_PAGE: u8 = 0xc;

/// PC M2C texel -> PlayStation colour: per output channel r, g, b the weights of r, g, b
/// (0...1) and an offset.
const M2C_TO_PS: [[f32; 4]; 3] = [
    [0.9362, 0.1643, 0.1087, -0.0011],
    [0.0857, 1.0441, 0.1013, -0.0091],
    [0.0899, 0.1432, 1.0376, -0.0023],
];
/// The mission whose pages are recoloured.
const RECOLOURED: &str = "M2C";

/// An on/off setting: a REG_DWORD in the game's key (default on), an environment variable
/// over it; 0 off, 1 on, 2 not read yet.
struct Setting {
    on: AtomicU8,
    value: &'static str,
    env: &'static str,
}

impl Setting {
    const fn new(value: &'static str, env: &'static str) -> Setting {
        Setting { on: AtomicU8::new(2), value, env }
    }

    fn get(&self) -> bool {
        let e = self.on.load(Ordering::Relaxed);
        if e != 2 {
            return e == 1;
        }
        let on = match std::env::var(self.env) {
            Ok(v) => v != "0",
            Err(_) => advapi32::value("", self.value).is_none_or(|(_, d)| d.first().is_some_and(|&b| b != 0)),
        };
        self.on.store(on as u8, Ordering::Relaxed);
        on
    }

    /// Preferences' OK: saved, and used from the next picture on.
    fn set(&self, on: bool) {
        self.on.store(on as u8, Ordering::Relaxed);
        advapi32::set_value("", self.value, 4, &(on as u32).to_le_bytes());
    }
}

static COLORING: Setting = Setting::new("TextureColoring", "FCOP_COLORING");
static PS_COLORS: Setting = Setting::new("PlayStationColors", "FCOP_PS_COLORS");

pub fn coloring() -> bool {
    COLORING.get()
}

pub fn set_coloring(on: bool) {
    COLORING.set(on);
}

pub fn ps_colors() -> bool {
    PS_COLORS.get()
}

pub fn set_ps_colors(on: bool) {
    PS_COLORS.set(on);
}

/// The hashes of the recoloured mission's pages (`hd::hash` of a whole download).
fn recoloured_pages() -> &'static HashSet<u64> {
    static PAGES: OnceLock<HashSet<u64>> = OnceLock::new();
    PAGES.get_or_init(|| {
        let file = std::fs::read_dir(super::game_dir())
            .into_iter()
            .flatten()
            .flatten()
            .find(|e| e.file_name().eq_ignore_ascii_case("missions"))
            .and_then(|d| std::fs::read_dir(d.path()).ok())
            .into_iter()
            .flatten()
            .flatten()
            .find(|e| e.file_name().eq_ignore_ascii_case(RECOLOURED));
        let Some(data) = file.and_then(|f| std::fs::read(f.path()).ok()) else { return HashSet::new() };
        crate::mission::load(&data)
            .pages
            .values()
            .map(|texels| super::hd::hash(0xb, 256, 256, &texels.iter().flat_map(|t| t.to_le_bytes()).collect::<Vec<u8>>()))
            .collect()
    })
}

/// A download's texels (RGBA8, `key` its hash) in the PlayStation's colours, where they differ.
pub fn recolour(key: u64, rgba: &mut [u8]) {
    if !ps_colors() || !recoloured_pages().contains(&key) {
        return;
    }
    for p in rgba.chunks_mut(4) {
        if p[..3] == [0, 0, 0] {
            continue;
        }
        let c = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let out = M2C_TO_PS.map(|m| ((m[0] * c[0] + m[1] * c[1] + m[2] * c[2] + m[3]).clamp(0.0, 1.0) * 255.0).round() as u8);
        // black would be keyed out
        if out != [0, 0, 0] {
            p[..3].copy_from_slice(&out);
        }
    }
}

/// What `begin` changed in the records.
pub struct Changed {
    recs: Vec<(u32, u8, [u8; 16])>,
}

/// Before the walker draws the 3D picture's records (`(record, vertices)`).
pub fn begin(recs: impl Iterator<Item = (u32, u32)>) -> Option<Changed> {
    if !coloring() {
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
