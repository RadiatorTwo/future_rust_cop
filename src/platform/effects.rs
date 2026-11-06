//! Knowing the effects (PLAN.md 2a step 4): which primitive of the 3D picture is smoke, fire,
//! an explosion, a muzzle flash... The later effects (soft particles, lit smoke, heat shimmer,
//! bloom) need it.
//!
//! Every primitive comes from a display list record (`walker.rs`): +4 type (bits 0-2 vertex
//! count, 0x70 how it is coloured, 0x80 textured, 0x08 ?), +5, +6, +7 texture number, +0x28 the
//! vertices' x/y (s16). In precise mode those x/y are exactly the vertices `glide::add` gets:
//! the walker hands the 3D picture's records over in drawing order (`begin`), and each
//! primitive finds its record by its first vertex, looking ahead from the last one found (a
//! quad is two triangles with the same first vertex; records drawn as nothing are skipped).
//!
//! Byte +6 is how a record is blended: 0 opaque, 1 alpha (the world's translucent things), 2
//! additive, 3 darkening. Effects are the additive and darkening ones (`glide::effect`): a
//! line is the laser sight, untextured ones are glows, textured ones by the mean colour of
//! their texture rectangle smoke (grey, or darkening), fire (warm) or energy (cool). Seen in
//! the first mission: fireballs, the blue shock wave ring, grey and dark smoke puffs (type
//! 0xbc), the rocket's trail (0xdc), glows (0x5b/0x5c, 3 or 4 vertices), the laser (0x3a). A
//! primitive whose first vertex is not found (some of the opaque and alpha-blended geometry,
//! drawn split or clipped otherwise) is no effect.
//!
//! A model's glows (street lamps' lights: the glow primitive of its faces, 0x428690 calling
//! the glow 0x4271c0) are marked (`glow_draw`): untextured warm glows like an explosion's
//! (0x43cbc0, 0x418080, 0x418de0... call it too), but lights, not heat.
//!
//! `FCOP_EFFECT_LOG=1`: every blended primitive's record kind (type, +5, +6, texture number)
//! and blend not seen before is logged, its texture rectangle written to `Textures/effects/`.

use crate::rt::{Cpu, ESP, call_original};
use crate::ver::Us;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// (position, size, colour...): adds a glow's records to the display list.
pub const GLOW_DRAW: Us = Us(0x4271c0);
/// Where a model's face primitive (0x428690) returns to from it.
const MODEL_GLOW: Us = Us(0x428be7);
/// The display list's end.
const LIST_END: Us = Us(0x511dc0);

/// Hook of the glow: a model's are marked as lights.
pub fn glow_draw(c: &mut Cpu) {
    let m = super::mem();
    let (ret, from) = (m.rd32(c.r[ESP]), m.rd32(LIST_END.at()));
    call_original(c, GLOW_DRAW.at());
    if ret == MODEL_GLOW.at() && wanted() {
        let to = m.rd32(LIST_END.at());
        MODEL_LIGHTS.with_borrow_mut(|s| s.extend((from..to).step_by(0x38)));
    }
}

/// What a primitive of the 3D picture is (a vertex attribute; `FCOP_GBUFFER=effects` shows
/// each in a colour of its own: the shader's `kinds`).
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u32)]
pub enum Effect {
    /// not an effect: geometry, terrain, translucent things
    None = 0,
    /// textured, grey: smoke puffs, rocket trails, dark smoke (drawn darkening)
    Smoke = 1,
    /// textured, warm colours: fireballs, flames
    Fire = 2,
    /// untextured additive: glows (muzzle flashes, lights; vertex colours fading to black)
    Glow = 3,
    /// a line: the laser sight
    Laser = 4,
    /// textured, cool colours: muzzle flashes, shock waves
    Energy = 5,
}

/// `FCOP_EFFECT_LOG=1`.
pub fn logging() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FCOP_EFFECT_LOG").is_some_and(|v| v != "0"))
}

/// `FCOP_GBUFFER=effects`: the 3D picture's primitives coloured by their kind.
pub fn showing() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("FCOP_GBUFFER").is_ok_and(|v| v == "effects"))
}

/// Something needs the primitives' records.
pub fn wanted() -> bool {
    logging() || showing() || super::lights::smoke_enabled() || super::heat::enabled()
}

/// A record's bytes +4 ... +7: type, two more, texture number.
pub type Record = [u8; 4];

/// The 3D picture's records in drawing order (first vertex, bytes +4 ... +7), and the next
/// one expected. The walker and the renderer run on one thread.
struct List {
    recs: Vec<((i16, i16), Record)>,
    /// each record's: a model's light (`glow_draw`)
    lights: Vec<bool>,
    next: usize,
    /// `FCOP_EFFECT_LOG`: primitives with and without their record found, pictures
    found: [u32; 2],
    pictures: u32,
}

thread_local! {
    /// the records of models' lights added since the last picture (`glow_draw`)
    static MODEL_LIGHTS: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
    static LIST: RefCell<List> = const { RefCell::new(List { recs: Vec::new(), lights: Vec::new(), next: 0, found: [0; 2], pictures: 0 }) };
}

/// How far ahead a primitive's record is looked for.
const AHEAD: usize = 16;

/// The walker draws the 3D picture: its records (guest addresses) in order.
pub fn begin(recs: impl Iterator<Item = u32>) {
    let m = super::mem();
    LIST.with_borrow_mut(|l| {
        l.recs.clear();
        l.lights.clear();
        l.next = 0;
        let lights = MODEL_LIGHTS.take();
        for r in recs {
            l.recs.push(((m.rd16(r + 0x28) as i16, m.rd16(r + 0x2a) as i16), m.rd32(r + 4).to_le_bytes()));
            l.lights.push(lights.contains(&r));
        }
    });
}

/// The 3D picture is drawn.
pub fn end() {
    LIST.with_borrow_mut(|l| {
        l.recs.clear();
        l.pictures += 1;
        if logging() && l.pictures % 1000 == 0 {
            super::log!("effect: records found for {} primitives, not for {}", l.found[0], l.found[1]);
            l.found = [0; 2];
        }
    });
}

/// The record of a primitive whose first vertex is at (x, y) (2560x1920 units, the view's
/// origin taken off).
/// And whether it is a model's light.
pub fn record_of(x: f32, y: f32) -> Option<(Record, bool)> {
    let v = (x.round() as i16, y.round() as i16);
    LIST.with_borrow_mut(|l| {
        let end = (l.next + AHEAD).min(l.recs.len());
        // the last one found again (the second triangle of a quad), then ahead
        let from = l.next.saturating_sub(1);
        let k = (from..end).find(|&k| l.recs[k].0 == v);
        l.found[k.is_none() as usize] += 1;
        let k = k?;
        l.next = k + 1;
        Some((l.recs[k].1, l.lights[k]))
    })
}

/// A primitive seen: its record, texture rectangle `rect` (texels), blend, colour (rgba) and
/// picture (rgb, width, height). Blended ones are logged and written once per record kind,
/// texture rectangle and blend.
pub fn census(rec: Record, rect: [u32; 4], blend: (u32, u32), colour: [u8; 4], cut: Option<(Vec<u8>, u32, u32)>) {
    static SEEN: Mutex<Option<HashSet<(Record, [u32; 4], (u32, u32))>>> = Mutex::new(None);
    // opaque: drawn ONE, ZERO, or SRC_ALPHA, 1 - SRC_ALPHA with alpha 1
    if blend == (4, 0) || blend == (1, 5) && colour[3] == 0xff {
        return;
    }
    let rect = if cut.is_some() { rect } else { [0; 4] };
    if !SEEN.lock().unwrap().get_or_insert_with(HashSet::new).insert((rec, rect, blend)) {
        return;
    }
    let [x0, y0, x1, y1] = rect;
    let name = format!("{:02x}{:02x}{:02x}_{:02x}_b{}{}_{x0}_{y0}_{x1}_{y1}", rec[0], rec[1], rec[2], rec[3], blend.0, blend.1);
    super::log!("effect: {name} colour {colour:02x?}");
    let Some((px, w, h)) = cut else { return };
    let rgba: Vec<u8> = px.chunks(3).flat_map(|c| [c[0], c[1], c[2], if c == [0, 0, 0] { 0 } else { 255 }]).collect();
    let dir = super::hd::folder().join("effects");
    let write = || -> Result<(), String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let f = std::io::BufWriter::new(std::fs::File::create(dir.join(format!("{name}.png"))).map_err(|e| e.to_string())?);
        let mut e = png::Encoder::new(f, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().and_then(|mut w| w.write_image_data(&rgba)).map_err(|e| e.to_string())
    };
    if let Err(e) = write() {
        super::log!("effect: cannot write {name}: {e}");
    }
}
