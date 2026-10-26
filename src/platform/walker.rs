//! The game's display list walker for Glide (0x4393e0), drawn more precisely.
//!
//! The game draws into a fixed 4:3 screen of 2560x1920 units. Its display list is one linked
//! list of 0x38-byte records in depth buckets, farthest first; bucket 0 (the HUD) comes last,
//! behind the node at [0x499ad0]. Record: +0 next, +4 type (bits 0-2: 1..4 a polygon with that
//! many vertices in 2560x1920 units, 5 a sprite in 640x480 units, 6 a sprite in pixels of the
//! game's resolution, 0 a control record; a type byte of 0 is a bucket node), +0x28.. x/y
//! pairs (s16).
//!
//! The walker converts each vertex to pixels through a jump table indexed by the resolution
//! (0x511e58): at 1280x960, x / 2 rounded down, and y stretched to the 1280x1024 Glide screen
//! the game opens for it, except for type-6 text, which stays in 1280x960 pixels and so sits
//! about 60 pixels above its icons (the game's own bug). Precise mode points the current
//! resolution's table entry at the walker's plain path, which passes the units unchanged
//! (type-6 records stay in game pixels), and draws text and everything else in separate
//! calls, each with its own coordinate space (`glide::set_space`). `FCOP_PRECISE=0` turns it
//! off.
//!
//! In split screen the game draws each player's list in its own call, which starts with
//! control records: type bit 0x10 sets the view's x origin (+0x28, in 640x480 units: 0 left,
//! 0x140 right; the plain path adds a quarter of it), 0x08 its extent. Every call gets them,
//! with the origin rescaled for its records.
//!
//! Widescreen (`wide.rs`): the 3D picture fills the wide frame, the HUD (bucket 0) is drawn in
//! further calls with 4:3 views (`glide::View`): in single player its left, centre and right
//! groups at the frame's left edge, centre and right edge; in split screen one per player's
//! half. Full-height HUD records (the split-screen divider) are moved like target markers.

use super::glide::{self, View};
use super::{mem, wide};
use crate::rt::{Cpu, ESP, call_original};
use std::sync::OnceLock;

pub const GLIDE_WALKER: u32 = 0x4393e0;
/// The walker's jump table of resolution cases (indices 4..10) and its unscaled path.
const TABLE: u32 = 0x43a168;
const TABLE_FIRST: u32 = 4;
const TABLE_LEN: u32 = 7;
const PLAIN: u32 = 0x4397cd;
/// The node in front of bucket 0 (the HUD).
const HUD_NODE: u32 = 0x499ad0;
/// The game's resolution index.
const RES_INDEX: u32 = 0x511e58;
/// Nonzero in split screen.
const SPLIT: u32 = 0x511f54;

pub fn precise() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("FCOP_PRECISE").map_or(true, |v| v != "0"))
}

struct Record {
    at: u32,
    next: u32,
    /// 0 the 3D picture; HUD: 1 left, 2 centre, 3 right
    pass: u8,
    text: bool,
}

struct Control {
    at: u32,
    next: u32,
    x: i16,
}

/// Hook of 0x4393e0(list, ..).
pub fn draw_glide(c: &mut Cpu) {
    if !precise() {
        call_original(c, GLIDE_WALKER);
        return;
    }
    let m = mem();
    let sp = c.r[ESP];
    let head = m.rd32(sp + 4);
    let hud_node = m.rd32(HUD_NODE);
    let mut recs = Vec::new();
    let mut ctls = Vec::new();
    let mut hud = false;
    // which view the HUD gets: in split screen the control records say which player's half
    let mut split = None;
    let mut r = head;
    while r != 0 {
        let t = m.rd8(r + 4);
        hud |= r == hud_node;
        if t & 7 != 0 {
            recs.push(Record { at: r, next: m.rd32(r), pass: if hud { 2 } else { 0 }, text: t & 7 == 6 });
        } else if t != 0 && !hud && ctls.len() < 4 {
            let x = m.rd16(r + 0x28) as i16;
            if t & 0x10 != 0 {
                split = Some(x != 0);
            }
            if t & 0x08 != 0 && x >= 0x280 {
                split = None;
            }
            ctls.push(Control { at: r, next: m.rd32(r), x });
        }
        r = m.rd32(r);
    }
    if m.rd8(SPLIT) == 0 {
        split = None;
    }
    let wide = wide::k() != 1.0;
    if wide {
        let first_hud = recs.iter().position(|r| r.pass != 0).unwrap_or(recs.len());
        for r in &recs[first_hud..] {
            if wide::extent(r.at).is_some_and(|e| e[1] <= 8 && e[3] >= 1911) {
                wide::to_screen(r.at);
            }
        }
        if split.is_none() {
            hud_sides(&mut recs[first_hud..]);
        }
    }
    let hud_view = |pass: u8| match (split, pass) {
        (_, 0) => View::Full,
        (Some(right), _) => View::Half(right),
        (None, 1) => View::Hud(0.0),
        (None, 3) => View::Hud(1.0),
        (None, _) => View::Hud(0.5),
    };

    let idx = m.rd32(RES_INDEX).wrapping_sub(TABLE_FIRST);
    let entry = (idx < TABLE_LEN).then(|| TABLE + 4 * idx);
    let saved = entry.map(|e| m.rd32(e));
    if let Some(e) = entry {
        m.wr32(e, PLAIN);
    }
    let (gw, gh) = wide::game_px();
    'passes: for pass in 0..4 {
        glide::set_view(if wide { hud_view(pass) } else { View::Full });
        for text in [false, true] {
            let mut sel = recs.iter().filter(|r| r.pass == pass && r.text == text);
            let Some(first) = sel.next() else { continue };
            let mut last = first.at;
            for r in sel {
                m.wr32(last, r.at);
                last = r.at;
            }
            m.wr32(last, 0);
            let mut first = first.at;
            for k in ctls.iter().rev() {
                m.wr32(k.at, first);
                first = k.at;
                if m.rd8(k.at + 4) & 0x10 != 0 {
                    let x = if text { k.x as i32 * gw as i32 / 640 } else { k.x as i32 * 4 };
                    m.wr16(k.at + 0x28, x as u16);
                }
            }
            glide::set_space(Some(if text { (gw, gh) } else { (2560, 1920) }));
            // each call pushes the return address back (the walker's ret popped it)
            c.r[ESP] = sp;
            m.wr32(sp + 4, first);
            call_original(c, GLIDE_WALKER);
            if c.stop != 0 {
                break 'passes;
            }
        }
    }
    glide::set_space(None);
    glide::set_view(View::Full);
    if let (Some(e), Some(v)) = (entry, saved) {
        m.wr32(e, v);
    }
    for r in &recs {
        m.wr32(r.at, r.next);
    }
    for k in &ctls {
        m.wr16(k.at + 0x28, k.x as u16);
        m.wr32(k.at, k.next);
    }
    m.wr32(sp + 4, head);
    c.r[ESP] = sp + 4; // as the walker's ret leaves it
}

/// Single-player HUD records -> passes 1 left, 2 centre, 3 right, by the centre of their
/// extent (centre band 1280 +- 160 units). Font glyphs in a row (texture page 0xd, type 6)
/// count as one, so a word or number stays whole; records placed by `wide::to_screen` stay
/// centred.
fn hud_sides(recs: &mut [Record]) {
    let m = mem();
    let glyph = |r: &Record| r.text && m.rd8(r.at + 7) == 0xd;
    let mut i = 0;
    while i < recs.len() {
        let mut j = i + 1;
        let mut pass = 2;
        if let Some(mut e) = wide::extent(recs[i].at) {
            while glyph(&recs[i]) && j < recs.len() && glyph(&recs[j]) {
                let Some(f) = wide::extent(recs[j].at) else { break };
                let gap = if f[0] > e[2] { f[0] - e[2] } else { e[0] - f[2] };
                if (f[1] - e[1]).abs() > 8 || gap > 48 {
                    break;
                }
                e = [e[0].min(f[0]), e[1], e[2].max(f[2]), e[3]];
                j += 1;
            }
            let cx = (e[0] + e[2]) / 2;
            pass = if cx < 1280 - 160 { 1 } else if cx > 1280 + 160 { 3 } else { 2 };
            if recs[i..j].iter().any(|r| wide::placed(r.at)) {
                pass = 2;
            }
        }
        for r in &mut recs[i..j] {
            r.pass = pass;
        }
        i = j;
    }
}
