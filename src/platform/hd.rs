//! Replacement textures ("HD textures"). The game downloads its texture pages (256x256
//! ARGB1555 atlases) whole, and some parts of them again later as bands of rows (the HUD's
//! message icon with its flashing lights, composed anew each time). Each download is known by a
//! hash of its texels: a PNG named by that hash in `game_files/Textures` (any folder below it,
//! anything before the hash in the name, e.g. `street_0123456789abcdef.png`) replaces it.
//!
//! Replacements are full colour with real alpha (straight, not premultiplied): alpha is the
//! coverage, 0 where the original is keyed out (black), soft edges allowed. The texel alpha the
//! game blends some surfaces with (ARGB1555's bit) stays the original's. A replacement can be
//! larger: every one is scaled to the size of the largest (twice, four times the original; at
//! most 4x), and whatever is not replaced on a page that has replacements is the original,
//! scaled up.
//!
//! `FCOP_DUMP_TEXTURES=1` writes every download not seen before to `Textures/dump/<hash>.png`
//! (alpha 0 where keyed out): the originals to make replacements from. `FCOP_HD=0` turns
//! replacements off.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The largest scale of a replacement: 1024x1024 for a page.
pub const MAX_SCALE: u32 = 4;
/// Decoded replacements kept (a mission's pages, with room to spare).
const KEPT: usize = 32;

/// An image: RGBA, 8 bits a channel, straight alpha.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

struct Index {
    files: HashMap<u64, PathBuf>,
    /// the size of replacements against the originals (1, 2 or 4)
    scale: u32,
}

/// The folder of replacements: `Textures` beside the game folder.
pub fn folder() -> PathBuf {
    let game = super::game_dir();
    game.parent().unwrap_or(game).join("Textures")
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut files = HashMap::new();
        if std::env::var_os("FCOP_HD").is_none_or(|v| v != "0") {
            scan(&folder(), &mut files);
        }
        // the widest replacement gives the scale (pages are 256 wide)
        let widest = files.values().filter_map(|p| png_size(p)).map(|(w, _)| w).max().unwrap_or(256);
        let scale = (widest / 256).clamp(1, MAX_SCALE).next_power_of_two().min(MAX_SCALE);
        if !files.is_empty() {
            super::log!("hd: {} replacement textures in {}, scale {scale}", files.len(), folder().display());
        }
        Index { files, scale }
    })
}

/// PNGs below `dir` whose names end in a 16-digit hex hash (not the dumps).
fn scan(dir: &Path, files: &mut HashMap<u64, PathBuf>) {
    let Ok(d) = std::fs::read_dir(dir) else { return };
    for e in d.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !e.file_name().eq_ignore_ascii_case("dump") {
                scan(&p, files);
            }
            continue;
        }
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
            continue;
        }
        let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let hex = stem.get(stem.len().saturating_sub(16)..).unwrap_or("");
        if let (16, Ok(h)) = (hex.len(), u64::from_str_radix(hex, 16)) {
            files.insert(h, p);
        }
    }
}

fn png_size(p: &Path) -> Option<(u32, u32)> {
    let f = std::io::BufReader::new(std::fs::File::open(p).ok()?);
    let r = png::Decoder::new(f).read_info().ok()?;
    let i = r.info();
    Some((i.width, i.height))
}

/// Replacements are drawn at this many times the original size; 0: there are none.
pub fn scale() -> u32 {
    let i = index();
    if i.files.is_empty() { 0 } else { i.scale }
}

/// The hash a download is known by: its format, size and texels (FNV-1a, 64 bits).
pub fn hash(format: u32, w: u32, h: u32, texels: &[u8]) -> u64 {
    let mut x: u64 = 0xcbf2_9ce4_8422_2325;
    for b in [format, w, h].iter().flat_map(|v| v.to_le_bytes()).chain(texels.iter().copied()) {
        x = (x ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    x
}

/// The replacement for a download, `w` x `h` texels of the original, at `scale()` times that.
pub fn find(key: u64, w: u32, h: u32) -> Option<Arc<Image>> {
    static CACHE: Mutex<VecDeque<(u64, Arc<Image>)>> = Mutex::new(VecDeque::new());
    let path = index().files.get(&key)?;
    let mut cache = CACHE.lock().unwrap();
    if let Some(k) = cache.iter().position(|(h, _)| *h == key) {
        let e = cache.remove(k).unwrap();
        cache.push_back(e.clone());
        return Some(e.1);
    }
    let img = match load(path) {
        Ok(img) => img,
        Err(e) => {
            super::log!("hd: cannot read {}: {e}", path.display());
            return None;
        }
    };
    let k = scale();
    let img = Arc::new(resize(&img, w * k, h * k));
    cache.push_back((key, img.clone()));
    if cache.len() > KEPT {
        cache.pop_front();
    }
    Some(img)
}

/// A PNG as RGBA8.
fn load(path: &Path) -> Result<Image, String> {
    let f = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let mut d = png::Decoder::new(f);
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = d.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        t => return Err(format!("colour type {t:?}")),
    };
    Ok(Image { w, h, rgba })
}

/// Scaled to `w` x `h` (bilinear, premultiplied so that transparent texels do not darken the
/// edges; an image of that size stays as it is).
fn resize(img: &Image, w: u32, h: u32) -> Image {
    if (img.w, img.h) == (w, h) {
        return Image { w, h, rgba: img.rgba.clone() };
    }
    let at = |x: u32, y: u32| {
        let p = &img.rgba[4 * (y * img.w + x) as usize..][..4];
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
    };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * img.h as f32 / h as f32 - 0.5).clamp(0.0, (img.h - 1) as f32);
        let (y0, ty) = (fy.floor() as u32, fy.fract());
        let y1 = (y0 + 1).min(img.h - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * img.w as f32 / w as f32 - 0.5).clamp(0.0, (img.w - 1) as f32);
            let (x0, tx) = (fx.floor() as u32, fx.fract());
            let x1 = (x0 + 1).min(img.w - 1);
            let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            let mut p = [0.0; 4];
            for i in 0..4 {
                p[i] = (a[i] * (1.0 - tx) + b[i] * tx) * (1.0 - ty) + (c[i] * (1.0 - tx) + d[i] * tx) * ty;
            }
            let k = if p[3] > 0.0 { 255.0 / p[3] } else { 0.0 };
            rgba.extend([(p[0] * k).round() as u8, (p[1] * k).round() as u8, (p[2] * k).round() as u8, p[3].round() as u8]);
        }
    }
    Image { w, h, rgba }
}

/// `FCOP_DUMP_TEXTURES=1`: a download (RGBA, straight) written to `Textures/dump` once.
pub fn dump(key: u64, w: u32, h: u32, rgba: &[u8]) {
    static ON: OnceLock<bool> = OnceLock::new();
    static DONE: Mutex<Option<HashSet<u64>>> = Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var_os("FCOP_DUMP_TEXTURES").is_some_and(|v| v != "0")) {
        return;
    }
    if !DONE.lock().unwrap().get_or_insert_with(HashSet::new).insert(key) {
        return;
    }
    let dir = folder().join("dump");
    let path = dir.join(format!("{key:016x}.png"));
    if path.exists() {
        return;
    }
    let write = || -> Result<(), String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let f = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        let mut e = png::Encoder::new(f, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().and_then(|mut w| w.write_image_data(rgba)).map_err(|e| e.to_string())
    };
    if let Err(e) = write() {
        super::log!("hd: cannot write {}: {e}", path.display());
    }
}
