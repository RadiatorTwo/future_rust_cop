//! Replacement textures made picture by picture. A texture page (256x256) is an atlas; where
//! a picture ends is not in the page, but in what uses it: every model face and terrain tile
//! names the part of the page it shows.
//!
//! - A model is mapped freely onto a painted sheet (the robot): its faces' parts that lie
//!   close together (4 texels apart at most) are one picture, the model's sheet.
//! - The terrain is the same: its tiles take windows at any place of large painted areas
//!   (tried: joining only windows that share 30% of the smaller still chained over whole
//!   pages, 90% gave thousands of overlapping windows), so its close parts are one picture
//!   too, mostly most of a page.
//!
//! `fcop export`: every page of every mission, cut into its pictures, to
//! `game_files/Textures/pieces/<mission>_page<n>_<hash>/<what>_x<x>_y<y>_<w>x<h>.png` (what:
//! `<mission>_model<n>` or `terrain`; alpha 0 where the game leaves texels out), with
//! `layout.png` (the page, twice the size, models' pictures outlined magenta, terrain's cyan)
//! and `page.png` (the page). Pieces already there are kept: they may be someone's work. A page
//! several missions share is cut once, by what all of them use. Pictures may overlap.
//!
//! `fcop import`: the changed pieces back into pages,
//! `game_files/Textures/<mission>_page<n>_<hash>.png` (the replacement, `platform::hd`):
//! pieces may be larger than the originals (up to 4x; all scaled to the largest's scale),
//! the rest of the page is the original scaled up. A page none of whose pieces changed is left
//! out (an earlier one removed).

use crate::mission;
use crate::platform::hd::{self, Image};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A page and what uses it: the rectangles around the parts faces and tiles show (x0 y0 x1
/// y1, the ends excluded).
struct Page {
    /// "<mission>_page<n>", from the first mission that has it
    name: String,
    texels: Vec<u16>,
    /// by model ("<mission>_model<n>")
    models: BTreeMap<String, Vec<[u16; 4]>>,
    terrain: Vec<[u16; 4]>,
}

/// One picture: what it is from ("<mission>_model<n>", "terrain") and its rectangle.
struct Piece {
    what: String,
    rect: [u16; 4],
}

fn bounds(uv: &[[u8; 2]]) -> Option<[u16; 4]> {
    let x0 = uv.iter().map(|c| c[0] as u16).min()?;
    let y0 = uv.iter().map(|c| c[1] as u16).min()?;
    let x1 = uv.iter().map(|c| c[0] as u16).max()?;
    let y1 = uv.iter().map(|c| c[1] as u16).max()?;
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
}

/// Every mission's pages, by hash.
fn pages(missions: &Path) -> Result<BTreeMap<u64, Page>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(missions)
        .map_err(|e| format!("{}: {e}", missions.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_none())
        .collect();
    files.sort();
    let mut out: BTreeMap<u64, Page> = BTreeMap::new();
    for file in files {
        let mission = file.file_name().unwrap().to_string_lossy().to_string();
        let m = mission::load(&std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?);
        // page number on faces and tiles (Cbmp - 1) -> hash
        let mut hashes = BTreeMap::new();
        for (&id, texels) in &m.pages {
            let bytes: Vec<u8> = texels.iter().flat_map(|t| t.to_le_bytes()).collect();
            let hash = hd::hash(0xb, 256, 256, &bytes);
            let new = || Page { name: format!("{mission}_page{id:02}"), texels: texels.clone(), models: BTreeMap::new(), terrain: Vec::new() };
            out.entry(hash).or_insert_with(new);
            hashes.insert(id - 1, hash);
        }
        for model in &m.models {
            for f in &model.faces {
                let uv: Vec<[u8; 2]> = f.corners.iter().map(|c| c.uv).collect();
                if let (Some(page), Some(r)) = (hashes.get(&f.page).and_then(|h| out.get_mut(h)), bounds(&uv)) {
                    page.models.entry(format!("{mission}_model{:03}", model.id)).or_default().push(r);
                }
            }
        }
        for t in m.tiles.iter().filter(|t| square(&t.uv)) {
            if let (Some(page), Some(r)) = (hashes.get(&t.page).and_then(|h| out.get_mut(h)), bounds(&t.uv)) {
                page.terrain.push(r);
            }
        }
    }
    Ok(out)
}

/// Texture coordinates that map a rectangle of the page (or half of one): two values of u
/// and two of v (2 texels apart at most count as one). Terrain tiles map such; the few others
/// are garbage the game does not draw (tiles drawn in a colour keep some): they would join
/// unrelated pictures.
fn square(uv: &[[u8; 2]]) -> bool {
    let distinct = |i: usize| {
        let mut v: Vec<u8> = uv.iter().map(|c| c[i]).collect();
        v.sort();
        1 + v.windows(2).filter(|w| w[1] - w[0] > 2).count()
    };
    distinct(0) <= 2 && distinct(1) <= 2
}

/// Whether two parts of a sheet are close: 4 texels apart at most.
fn close(a: &[u16; 4], b: &[u16; 4]) -> bool {
    a[0] <= b[2] + 4 && b[0] <= a[2] + 4 && a[1] <= b[3] + 4 && b[1] <= a[3] + 4
}

/// Rectangles joined by `joined` (directly or through others) as the rectangles around each
/// group, taking the column and row at their ends too (texture coordinates often name the
/// last texel).
fn groups(rects: &[[u16; 4]], joined: fn(&[u16; 4], &[u16; 4]) -> bool) -> Vec<[u16; 4]> {
    let mut rects = rects.to_vec();
    rects.sort();
    rects.dedup();
    let mut parent: Vec<usize> = (0..rects.len()).collect();
    fn root(p: &mut [usize], mut k: usize) -> usize {
        while p[k] != k {
            p[k] = p[p[k]];
            k = p[k];
        }
        k
    }
    for i in 0..rects.len() {
        for j in i + 1..rects.len() {
            if joined(&rects[i], &rects[j]) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut out: BTreeMap<usize, [u16; 4]> = BTreeMap::new();
    for (k, r) in rects.iter().enumerate() {
        let g = out.entry(root(&mut parent, k)).or_insert(*r);
        *g = [g[0].min(r[0]), g[1].min(r[1]), g[2].max(r[2]), g[3].max(r[3])];
    }
    out.into_values().map(|r| [r[0], r[1], (r[2] + 1).min(256), (r[3] + 1).min(256)]).collect()
}

/// A page's pictures: each model's sheet, then the terrain's; the same rectangle once.
fn pictures(page: &Page) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let all = page.models.iter().map(|(what, r)| (what.as_str(), groups(r, close))).chain([("terrain", groups(&page.terrain, close))]);
    for (what, rects) in all {
        for rect in rects {
            if !out.iter().any(|p| p.rect == rect) {
                out.push(Piece { what: what.to_string(), rect });
            }
        }
    }
    out
}

/// A page as RGBA: the colour, alpha 0 for black (the game's chroma key).
fn rgba(texels: &[u16]) -> Image {
    let e5 = |v: u16| ((v << 3) | (v >> 2)) as u8;
    let rgba = texels.iter().flat_map(|&t| [e5(t >> 10 & 31), e5(t >> 5 & 31), e5(t & 31), if t & 0x7fff != 0 { 255 } else { 0 }]).collect();
    Image { w: 256, h: 256, rgba }
}

fn cut(img: &Image, r: &[u16; 4]) -> Image {
    let (w, h) = ((r[2] - r[0]) as u32, (r[3] - r[1]) as u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in r[1] as u32..r[3] as u32 {
        let at = (4 * (y * img.w + r[0] as u32)) as usize;
        rgba.extend_from_slice(&img.rgba[at..at + 4 * w as usize]);
    }
    Image { w, h, rgba }
}

fn write_png(path: &Path, img: &Image) -> Result<(), String> {
    let f = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?);
    let mut e = png::Encoder::new(f, img.w, img.h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().and_then(|mut w| w.write_image_data(&img.rgba)).map_err(|e| format!("{}: {e}", path.display()))
}

fn piece_name(p: &Piece) -> String {
    let r = p.rect;
    format!("{}_x{}_y{}_{}x{}.png", p.what, r[0], r[1], r[2] - r[0], r[3] - r[1])
}

/// A piece's rectangle from its file name (its last three parts).
fn piece_rect(name: &str) -> Option<[u16; 4]> {
    let stem = name.strip_suffix(".png")?;
    let mut parts = stem.rsplit('_');
    let (w, h) = parts.next()?.split_once('x')?;
    let (w, h): (u16, u16) = (w.parse().ok()?, h.parse().ok()?);
    let y: u16 = parts.next()?.strip_prefix('y')?.parse().ok()?;
    let x: u16 = parts.next()?.strip_prefix('x')?.parse().ok()?;
    (w > 0 && h > 0 && x + w <= 256 && y + h <= 256).then_some([x, y, x + w, y + h])
}

/// The page twice the size with the pictures outlined: models' magenta, terrain's cyan.
fn layout(page: &Image, pictures: &[Piece]) -> Image {
    let mut img = hd::resize(page, 512, 512);
    // keyed texels dark grey, so the outlines show on them
    for p in img.rgba.chunks_mut(4) {
        if p[3] == 0 {
            p.copy_from_slice(&[40, 40, 40, 255]);
        }
    }
    for p in pictures {
        let colour = if p.what == "terrain" { [0, 255, 255, 255] } else { [255, 0, 255, 255] };
        let [x0, y0, x1, y1] = p.rect.map(|v| (2 * v as u32).min(511));
        for x in x0..=x1 {
            for y in [y0, y1] {
                let at = (4 * (y * 512 + x)) as usize;
                img.rgba[at..at + 4].copy_from_slice(&colour);
            }
        }
        for y in y0..=y1 {
            for x in [x0, x1] {
                let at = (4 * (y * 512 + x)) as usize;
                img.rgba[at..at + 4].copy_from_slice(&colour);
            }
        }
    }
    img
}

/// `fcop export`: the pieces of every page into `out/pieces`.
pub fn export(missions: &Path, out: &Path) -> Result<(), String> {
    let pages = pages(missions)?;
    let (mut written, mut kept) = (0, 0);
    for (hash, page) in &pages {
        let dir = out.join("pieces").join(format!("{}_{hash:016x}", page.name));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let img = rgba(&page.texels);
        let pictures = pictures(page);
        for p in &pictures {
            let path = dir.join(piece_name(p));
            if path.exists() {
                kept += 1;
            } else {
                write_png(&path, &cut(&img, &p.rect))?;
                written += 1;
            }
        }
        write_png(&dir.join("page.png"), &img)?;
        write_png(&dir.join("layout.png"), &layout(&img, &pictures))?;
    }
    eprintln!("{} pages, {written} pieces written, {kept} kept", pages.len());
    Ok(())
}

/// `fcop import`: changed pieces back into pages in `out`.
pub fn import(missions: &Path, out: &Path) -> Result<(), String> {
    let pages = pages(missions)?;
    let mut made = 0;
    for (hash, page) in &pages {
        let name = format!("{}_{hash:016x}", page.name);
        let dir = out.join("pieces").join(&name);
        let target = out.join(format!("{name}.png"));
        let original = rgba(&page.texels);
        // only the changed pieces: an unchanged one is the original, and pieces may overlap
        let mut pieces: Vec<([u16; 4], Image)> = Vec::new();
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            let Some(r) = piece_rect(&file) else { continue };
            let img = hd::load(&e.path()).map_err(|err| format!("{}: {err}", e.path().display()))?;
            let before = cut(&original, &r);
            if (img.w, img.h) != (before.w, before.h) || img.rgba != before.rgba {
                pieces.push((r, img));
            }
        }
        if pieces.is_empty() {
            if target.exists() {
                std::fs::remove_file(&target).map_err(|e| format!("{}: {e}", target.display()))?;
            }
            continue;
        }
        // the largest piece's scale, at most 4
        let k = pieces.iter().map(|(r, img)| img.w.div_ceil((r[2] - r[0]) as u32).max(img.h.div_ceil((r[3] - r[1]) as u32))).max().unwrap_or(1).clamp(1, hd::MAX_SCALE);
        let size = 256 * k;
        let mut result = hd::resize(&original, size, size);
        // larger pieces first: a smaller one inside is drawn over it
        pieces.sort_by_key(|(r, _)| std::cmp::Reverse((r[2] - r[0]) as u32 * (r[3] - r[1]) as u32));
        for (r, img) in &pieces {
            let (w, h) = ((r[2] - r[0]) as u32 * k, (r[3] - r[1]) as u32 * k);
            let img = hd::resize(img, w, h);
            for y in 0..h {
                let to = (4 * ((r[1] as u32 * k + y) * size + r[0] as u32 * k)) as usize;
                let from = (4 * y * w) as usize;
                result.rgba[to..to + 4 * w as usize].copy_from_slice(&img.rgba[from..from + 4 * w as usize]);
            }
        }
        write_png(&target, &result)?;
        made += 1;
    }
    eprintln!("{made} pages made from changed pieces");
    Ok(())
}
