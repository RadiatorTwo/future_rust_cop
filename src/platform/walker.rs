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

use super::{glide, mem};
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
/// The game's resolution index, and its table of modes (20 bytes each: width, height, ...).
const RES_INDEX: u32 = 0x511e58;
const RES_TABLE: u32 = 0x487080;

fn precise() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("FCOP_PRECISE").map_or(true, |v| v != "0"))
}

/// The game's resolution in pixels (the space of type-6 records).
fn game_px() -> (u32, u32) {
    let m = mem();
    let r = m.rd32(RES_INDEX);
    let (w, h) = if r <= 10 { (m.rd32(RES_TABLE + 20 * r), m.rd32(RES_TABLE + 4 + 20 * r)) } else { (1280, 960) };
    if w < 320 || h < 200 { (1280, 960) } else { (w, h) }
}

struct Record {
    at: u32,
    next: u32,
    hud: bool,
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
    let mut r = head;
    while r != 0 {
        let t = m.rd8(r + 4);
        hud |= r == hud_node;
        if t & 7 != 0 {
            recs.push(Record { at: r, next: m.rd32(r), hud, text: t & 7 == 6 });
        } else if t != 0 && !hud && ctls.len() < 4 {
            ctls.push(Control { at: r, next: m.rd32(r), x: m.rd16(r + 0x28) as i16 });
        }
        r = m.rd32(r);
    }

    let idx = m.rd32(RES_INDEX).wrapping_sub(TABLE_FIRST);
    let entry = (idx < TABLE_LEN).then(|| TABLE + 4 * idx);
    let saved = entry.map(|e| m.rd32(e));
    if let Some(e) = entry {
        m.wr32(e, PLAIN);
    }
    let (gw, gh) = game_px();
    'passes: for hud in [false, true] {
        for text in [false, true] {
            let mut sel = recs.iter().filter(|r| r.hud == hud && r.text == text);
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
