//! The folders `fcop export` writes and the pieces in them (see `src/export.rs` of fcop):
//! `pieces/<mission>_page<n>_<hash>/<what>_x<x>_y<y>_<w>x<h>.png` with `<...>.json` beside it,
//! `page.png` (the original page) and `layout.png`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// An RGBA picture.
#[derive(Clone, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// The rectangle x0 y0 x1 y1 (ends excluded) cut out.
    pub fn cut(&self, r: [u16; 4]) -> Image {
        let (w, h) = ((r[2] - r[0]) as u32, (r[3] - r[1]) as u32);
        let mut rgba = Vec::with_capacity((4 * w * h) as usize);
        for y in r[1] as u32..r[3] as u32 {
            let at = (4 * (y * self.w + r[0] as u32)) as usize;
            rgba.extend_from_slice(&self.rgba[at..at + 4 * w as usize]);
        }
        Image { w, h, rgba }
    }
}

/// A PNG of any colour type as RGBA.
pub fn load_png(path: &Path) -> Result<Image, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let buf = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf.to_vec(),
        png::ColorType::Rgb => buf.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(format!("{}: palette not expanded", path.display())),
    };
    Ok(Image { w: info.width, h: info.height, rgba })
}

/// A folder of pieces: one page.
pub struct Folder {
    pub path: PathBuf,
    /// "<mission>_page<n>_<hash>"
    pub name: String,
    pub mission: String,
    /// "page<n>"
    pub page: String,
}

/// All folders in `root`, by name.
pub fn folders(root: &Path) -> Vec<Folder> {
    let mut list: Vec<Folder> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let (mission, page) = match name.find("_page") {
                Some(at) => (name[..at].to_string(), name[at + 1..].split('_').next().unwrap_or("").to_string()),
                None => (String::new(), name.clone()),
            };
            Folder { path: e.path(), name, mission, page }
        })
        .collect();
    list.sort_by_key(|a| a.name.to_lowercase());
    list
}

/// A piece's rectangle from its file name (its last three parts), as fcop reads it.
pub fn piece_rect(name: &str) -> Option<[u16; 4]> {
    let stem = name.strip_suffix(".png")?;
    let mut parts = stem.rsplit('_');
    let (w, h) = parts.next()?.split_once('x')?;
    let (w, h): (u16, u16) = (w.parse().ok()?, h.parse().ok()?);
    let y: u16 = parts.next()?.strip_prefix('y')?.parse().ok()?;
    let x: u16 = parts.next()?.strip_prefix('x')?.parse().ok()?;
    (w > 0 && h > 0 && x + w <= 256 && y + h <= 256).then_some([x, y, x + w, y + h])
}

/// The piece files in a folder, by name.
pub fn piece_files(dir: &Path) -> Vec<(String, [u16; 4])> {
    let mut list: Vec<(String, [u16; 4])> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            piece_rect(&name).map(|r| (name, r))
        })
        .collect();
    list.sort();
    list
}

/// Whether a piece differs from its part of the original page, as `fcop import` decides.
pub fn changed(img: &Image, page: &Image, r: [u16; 4]) -> bool {
    *img != page.cut(r)
}

/// A file's modification time (for reloading what an editor saved).
pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A JSON file: its lines and the faces' corners (0...1 in the piece) with the line each
/// face is on.
pub struct Faces {
    pub lines: Vec<String>,
    pub faces: Vec<(usize, Vec<[f32; 2]>)>,
}

pub fn load_faces(path: &Path) -> Faces {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut faces = Vec::new();
    // fcop writes a face a line, after `"faces": [`
    if let Some(start) = lines.iter().position(|l| l.trim_start().starts_with("\"faces\"")) {
        for (i, line) in lines.iter().enumerate().skip(start + 1) {
            let Ok(face) = serde_json::from_str::<serde_json::Value>(line.trim().trim_end_matches(',')) else { continue };
            let uv: Vec<[f32; 2]> = face["uv"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| Some([c[0].as_f64()? as f32, c[1].as_f64()? as f32]))
                .collect();
            if uv.len() >= 2 {
                faces.push((i, uv));
            }
        }
    }
    Faces { lines, faces }
}

/// Finds `game_files/Textures/pieces` beside the program or the current folder, or above them.
pub fn default_root() -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        starts.push(dir);
    }
    if let Ok(dir) = std::env::current_dir() {
        starts.push(dir);
    }
    for start in starts {
        for dir in start.ancestors().take(6) {
            for sub in ["game_files/Textures/pieces", "Textures/pieces", "pieces", "target/release/game_files/Textures/pieces"] {
                let p = dir.join(sub);
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
    }
    None
}
