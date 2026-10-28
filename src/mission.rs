//! The game's mission files (`missions/M2C`...): a container of resources, each with a kind
//! (`Cbmp` texture page, `Cobj` model, `Ctil` terrain, `Cwav` sound...) and a number. Chunks
//! are a tag (stored reversed: "COHS" for SHOC), a size (with the header) and the contents;
//! a SHOC holds either a resource's header (SHDR: kind, number) or a piece of its data (SDAT),
//! the pieces following their header. Inside a resource the same chunks again: a `Cbmp` holds
//! `PX16` (256x256 texels, 16 bits), a `Cobj` its vertices, faces and texture coordinates, a
//! `Ctil` a section of the terrain (`Sect`).

use std::collections::BTreeMap;

pub struct Resource {
    /// e.g. "Cobj"
    pub kind: String,
    pub id: u32,
    pub data: Vec<u8>,
}

fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([d[at], d[at + 1]])
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}

/// A chunk's tag as written in the documentation (the file stores it reversed).
fn tag(d: &[u8]) -> String {
    d.iter().rev().map(|&b| b as char).collect()
}

/// The resources of a mission file, in their order.
pub fn resources(file: &[u8]) -> Vec<Resource> {
    let mut out: Vec<Resource> = Vec::new();
    let mut i = 0;
    while i + 8 <= file.len() {
        let size = u32_at(file, i + 4) as usize;
        if size < 8 || i + size > file.len() {
            break;
        }
        if tag(&file[i..i + 4]) == "SHOC" && size >= 20 {
            match tag(&file[i + 16..i + 20]).as_str() {
                "SHDR" if size >= 32 => out.push(Resource { kind: tag(&file[i + 24..i + 28]), id: u32_at(file, i + 28), data: Vec::new() }),
                "SDAT" => {
                    if let Some(r) = out.last_mut() {
                        r.data.extend_from_slice(&file[i + 20..i + size]);
                    }
                }
                _ => {}
            }
        }
        i += size;
    }
    out
}

/// The chunks inside a resource: tag and contents.
pub fn chunks(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= data.len() {
        let size = u32_at(data, i + 4) as usize;
        if size < 8 || i + size > data.len() {
            break;
        }
        out.push((tag(&data[i..i + 4]), &data[i + 8..i + size]));
        i += size;
    }
    out
}

fn chunk<'a>(data: &'a [u8], name: &str) -> Option<&'a [u8]> {
    chunks(data).into_iter().find(|(t, _)| t == name).map(|(_, c)| c)
}

/// A texture page's texels as the game downloads them (ARGB1555): `PX16` has the colour in
/// the same place; the alpha bit is set where a texel is not black and its own top bit (the
/// PlayStation's semi-transparency bit) is clear.
pub fn page_texels(cbmp: &[u8]) -> Option<Vec<u16>> {
    let px = chunk(cbmp, "PX16").filter(|p| p.len() == 256 * 256 * 2)?;
    Some(
        px.chunks(2)
            .map(|p| {
                let v = u16::from_le_bytes([p[0], p[1]]);
                let colour = v & 0x7fff;
                colour | if colour != 0 && v & 0x8000 == 0 { 0x8000 } else { 0 }
            })
            .collect(),
    )
}

/// One corner of a face: its vertex and its texel coordinates on the page.
#[derive(Clone, Copy)]
pub struct Corner {
    pub vertex: u16,
    pub uv: [u8; 2],
}

pub struct Face {
    pub corners: Vec<Corner>,
    /// the texture page: the mission's `Cbmp` number - 1
    pub page: u32,
}

pub struct Model {
    pub id: u32,
    /// x, y, z (the first vertex list: models with several are animated or in parts)
    pub vertices: Vec<[i16; 3]>,
    /// textured triangles and quads (corners in order around the face)
    pub faces: Vec<Face>,
}

/// A `Cobj`: `4DVL` vertices (a word, the count, then x y z and a pad word each), `3DQL` faces
/// (a word, the count, then 12 bytes each: flags, corners (low 3 bits: 3 or 4; others are
/// points and lines), the byte offset of its `3DTL` entry, 4 vertex and 4 normal indices) and
/// `3DTL` texture entries (a word, then 16 bytes each: 4 bytes of kind and colour, 4 texel
/// coordinates u v, the page).
pub fn model(id: u32, data: &[u8]) -> Option<Model> {
    let vl = chunk(data, "4DVL")?;
    let n = u32_at(vl, 4) as usize;
    let vertices: Vec<[i16; 3]> = (0..n)
        .take_while(|k| 8 + 8 * k + 6 <= vl.len())
        .map(|k| {
            let at = 8 + 8 * k;
            [u16_at(vl, at) as i16, u16_at(vl, at + 2) as i16, u16_at(vl, at + 4) as i16]
        })
        .collect();
    let ql = chunk(data, "3DQL")?;
    let tl = chunk(data, "3DTL").unwrap_or(&[]);
    let mut faces = Vec::new();
    for k in 0..u32_at(ql, 4) as usize {
        let Some(f) = ql.get(8 + 12 * k..8 + 12 * k + 12) else { break };
        let count = (f[1] & 7) as usize;
        let at = 4 + u16_at(f, 2) as usize;
        let (true, Some(t)) = (count == 3 || count == 4, tl.get(at..at + 16)) else { continue };
        if f[4..4 + count].iter().any(|&v| v as usize >= vertices.len()) {
            continue;
        }
        let corners = (0..count).map(|c| Corner { vertex: f[4 + c] as u16, uv: [t[4 + 2 * c], t[5 + 2 * c]] }).collect();
        faces.push(Face { corners, page: u32_at(t, 12) });
    }
    Some(Model { id, vertices, faces })
}

/// A terrain tile's texture: its corners' texel coordinates (3 or 4) and its page.
pub struct Tile {
    pub uv: Vec<[u8; 2]>,
    /// as on model faces: the `Cbmp` number - 1
    pub page: u32,
}

/// The textured tiles of a `Ctil` section (`Sect`, as the game reads it at 0x4537f0 and
/// 0x453930; offsets in the chunk's contents): a word, the number of texel coordinate pairs
/// (u16 at 2), heights, 16x16 cells at 0x3c4 (u16: first tile << 6 | count), at 0x5c4 the
/// number of tiles << 6, from 0x5c8 the tiles (u32: bits 1-10 the byte offset of its
/// coordinates, 15-21 its shape, 22-31 its texture), after them the coordinate pairs (u v
/// bytes) and the textures (u16: bits 8-10 the page, 0x2000 four coordinates, else three;
/// bits 13-15 how it is drawn). A triangle shape uses only three.
pub fn tiles(data: &[u8]) -> Vec<Tile> {
    let Some(s) = chunk(data, "Sect").filter(|s| s.len() >= 0x5c8) else { return Vec::new() };
    let pairs = u16_at(s, 2) as usize;
    let count = (u16_at(s, 0x5c4) >> 6) as usize;
    let coords = 0x5c8 + 4 * count;
    let textures = coords + 2 * pairs;
    let mut out = Vec::new();
    for k in 0..count {
        let Some(t) = s.get(0x5c8 + 4 * k..0x5cc + 4 * k) else { break };
        let t = u32::from_le_bytes(t.try_into().unwrap());
        let Some(texture) = s.get(textures + 2 * (t >> 22) as usize..).filter(|r| r.len() >= 2).map(|r| u16_at(r, 0)) else { continue };
        let n = if texture & 0x2000 != 0 && !triangle((t >> 15 & 0x7f) as u8) { 4 } else { 3 };
        let at = coords + (t & 0x7fe) as usize;
        let Some(c) = s.get(at..at + 2 * n) else { continue };
        out.push(Tile { uv: c.chunks(2).map(|p| [p[0], p[1]]).collect(), page: (texture >> 8 & 7) as u32 });
    }
    out
}

/// The tile shapes that are triangles (the game's table of corners at 0x48c608 repeats the
/// first corner as the fourth for them).
fn triangle(shape: u8) -> bool {
    shape < 60 || (71..=102).contains(&shape)
}

/// A mission's texture pages (by `Cbmp` number), models and terrain tiles.
pub struct Mission {
    pub pages: BTreeMap<u32, Vec<u16>>,
    pub models: Vec<Model>,
    pub tiles: Vec<Tile>,
}

pub fn load(file: &[u8]) -> Mission {
    let mut m = Mission { pages: BTreeMap::new(), models: Vec::new(), tiles: Vec::new() };
    for r in resources(file) {
        match r.kind.as_str() {
            "Cbmp" => {
                if let Some(t) = page_texels(&r.data) {
                    m.pages.insert(r.id, t);
                }
            }
            "Cobj" => m.models.extend(model(r.id, &r.data)),
            "Ctil" => m.tiles.extend(tiles(&r.data)),
            _ => {}
        }
    }
    m
}
