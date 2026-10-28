//! texview: a navigator for the texture pieces `fcop export` writes to
//! `game_files/Textures/pieces`. Left the folders (one a page), next the pieces in the selected
//! one, right the piece with its faces (UV grid), the original beside it when it was changed,
//! where it lies on its page and its JSON. Files saved by an image editor show at once.
//!
//! `texview [folder or piece]`; without one it looks for `game_files/Textures/pieces` beside
//! itself or the current folder (and above them). The pieces' folder, one page's folder or a
//! piece opens there (a piece: selected); they can also be dropped onto the window.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod scan;
mod view;

use eframe::egui::{self, Color32, Key, Modifiers, Sense, TextureHandle, TextureOptions, vec2};
use scan::{Folder, Image};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

fn main() -> eframe::Result {
    let root = std::env::args().nth(1).map(PathBuf::from).or_else(scan::default_root);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 1000.0]).with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("texview", options, Box::new(|cc| Ok(Box::new(App::new(cc, root)))))
}

pub const MODEL: Color32 = Color32::from_rgb(255, 0, 255);
pub const TERRAIN: Color32 = Color32::from_rgb(0, 255, 255);
pub const MARK: Color32 = Color32::from_rgb(255, 150, 0);

/// One piece of the loaded folder.
pub struct Piece {
    pub name: String,
    pub rect: [u16; 4],
    pub path: PathBuf,
    pub img: Option<Image>,
    pub tex: Option<TextureHandle>,
    pub changed: bool,
    mtime: Option<SystemTime>,
}

impl Piece {
    pub fn terrain(&self) -> bool {
        self.name.starts_with("terrain_")
    }

    /// "model019 103x70", "terrain 256x128"
    fn label(&self) -> String {
        let r = self.rect;
        let what = self.name.split('_').find(|p| p.starts_with("model")).unwrap_or("terrain");
        format!("{what} {}x{}", r[2] - r[0], r[3] - r[1])
    }
}

/// The selected folder, loaded.
pub struct Loaded {
    pub folder: usize,
    pub page: Option<Image>,
    pub page_tex: Option<TextureHandle>,
    page_mtime: Option<SystemTime>,
    pub pieces: Vec<Piece>,
}

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    All,
    Models,
    Terrain,
}

pub struct App {
    root: Option<PathBuf>,
    root_text: String,
    folders: Vec<Folder>,
    /// per folder: any piece changed (filled by a thread)
    folder_changed: Vec<Option<bool>>,
    changed_rx: Option<Receiver<(usize, bool)>>,
    sel_folder: Option<usize>,
    pub loaded: Option<Loaded>,
    pub sel_piece: Option<usize>,
    folder_filter: String,
    piece_filter: String,
    kind: Kind,
    only_changed: bool,
    focus_filter: bool,
    scroll_folder: bool,
    scroll_piece: bool,
    last_poll: Instant,
    pub view: view::View,
    pub checker: TextureHandle,
}

impl App {
    fn new(cc: &eframe::CreationContext, root: Option<PathBuf>) -> App {
        // grey squares behind transparent texels, repeated
        let checker = egui::ColorImage::from_rgba_unmultiplied(
            [2, 2],
            &[90, 90, 90, 255, 60, 60, 60, 255, 60, 60, 60, 255, 90, 90, 90, 255],
        );
        let options = TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..TextureOptions::NEAREST };
        let checker = cc.egui_ctx.load_texture("checker", checker, options);
        let mut app = App {
            root: None,
            root_text: String::new(),
            folders: Vec::new(),
            folder_changed: Vec::new(),
            changed_rx: None,
            sel_folder: None,
            loaded: None,
            sel_piece: None,
            folder_filter: String::new(),
            piece_filter: String::new(),
            kind: Kind::All,
            only_changed: false,
            focus_filter: false,
            scroll_folder: false,
            scroll_piece: false,
            last_poll: Instant::now(),
            view: view::View::default(),
            checker,
        };
        if let Some(path) = root {
            app.open_path(&cc.egui_ctx, path);
        }
        app
    }

    /// The pieces' folder, a page's folder (it has `page.png`) or a piece in one.
    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        let (dir, piece) = if path.is_file() {
            (path.parent().map(Path::to_path_buf).unwrap_or_default(), path.file_name().map(|n| n.to_string_lossy().replace(".json", ".png")))
        } else {
            (path, None)
        };
        if !dir.join("page.png").exists() {
            self.open_root(ctx, dir);
            return;
        }
        let Some(root) = dir.parent() else { return };
        self.open_root(ctx, root.to_path_buf());
        if let Some(f) = self.folders.iter().position(|f| f.path.file_name() == dir.file_name()) {
            self.select_folder(ctx, f);
            if let Some(i) = piece.and_then(|name| self.loaded.as_ref()?.pieces.iter().position(|p| p.name == name)) {
                self.sel_piece = Some(i);
                self.select_piece_changed();
            }
        }
    }

    fn open_root(&mut self, ctx: &egui::Context, root: PathBuf) {
        self.root_text = root.display().to_string();
        self.folders = scan::folders(&root);
        self.folder_changed = vec![None; self.folders.len()];
        self.loaded = None;
        self.sel_folder = None;
        self.sel_piece = None;
        // which folders have changed pieces, in the background (thousands of PNGs)
        let (tx, rx) = channel();
        let dirs: Vec<PathBuf> = self.folders.iter().map(|f| f.path.clone()).collect();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            for (i, dir) in dirs.iter().enumerate() {
                let any = scan::load_png(&dir.join("page.png")).is_ok_and(|page| {
                    scan::piece_files(dir).iter().any(|(name, r)| scan::load_png(&dir.join(name)).is_ok_and(|img| scan::changed(&img, &page, *r)))
                });
                if tx.send((i, any)).is_err() {
                    return;
                }
                ctx2.request_repaint();
            }
        });
        self.changed_rx = Some(rx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("texview - {}", root.display())));
        self.root = Some(root);
        if let Some(first) = self.visible_folders().first().copied() {
            self.select_folder(ctx, first);
        }
    }

    fn select_folder(&mut self, ctx: &egui::Context, i: usize) {
        self.sel_folder = Some(i);
        self.scroll_folder = true;
        self.load_folder(ctx, None);
        self.sel_piece = self.visible_pieces().first().copied();
        self.select_piece_changed();
    }

    /// (Re)loads the selected folder; `keep`: the piece to select again, by name.
    fn load_folder(&mut self, ctx: &egui::Context, keep: Option<String>) {
        let Some(i) = self.sel_folder else { return };
        let dir = self.folders[i].path.clone();
        let page_path = dir.join("page.png");
        let page = scan::load_png(&page_path).ok();
        let page_tex = page.as_ref().map(|p| texture(ctx, "page", p));
        let mut pieces: Vec<Piece> = scan::piece_files(&dir)
            .into_iter()
            .map(|(name, rect)| Piece { path: dir.join(&name), name, rect, img: None, tex: None, changed: false, mtime: None })
            .collect();
        for p in &mut pieces {
            load_piece(ctx, p, page.as_ref());
        }
        self.folder_changed[i] = Some(pieces.iter().any(|p| p.changed));
        self.loaded = Some(Loaded { folder: i, page, page_tex, page_mtime: scan::mtime(&page_path), pieces });
        self.sel_piece = keep.and_then(|name| self.loaded.as_ref()?.pieces.iter().position(|p| p.name == name));
    }

    fn select_piece_changed(&mut self) {
        self.scroll_piece = true;
        self.view.reset();
        self.view.piece_changed(self.loaded.as_ref().zip(self.sel_piece).map(|(l, i)| &l.pieces[i]));
    }

    fn visible_folders(&self) -> Vec<usize> {
        let filter = self.folder_filter.to_lowercase();
        (0..self.folders.len())
            .filter(|&i| self.folders[i].name.to_lowercase().contains(&filter))
            .filter(|&i| !self.only_changed || self.folder_changed[i] == Some(true))
            .collect()
    }

    fn visible_pieces(&self) -> Vec<usize> {
        let Some(l) = &self.loaded else { return Vec::new() };
        let filter = self.piece_filter.to_lowercase();
        (0..l.pieces.len())
            .filter(|&i| {
                let p = &l.pieces[i];
                p.name.to_lowercase().contains(&filter)
                    && match self.kind {
                        Kind::All => true,
                        Kind::Models => !p.terrain(),
                        Kind::Terrain => p.terrain(),
                    }
                    && (!self.only_changed || p.changed)
            })
            .collect()
    }

    /// Reloads what changed on disk (an editor saved a piece), once a second.
    fn poll(&mut self, ctx: &egui::Context) {
        if self.last_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_poll = Instant::now();
        let Some(l) = &mut self.loaded else { return };
        let dir = self.folders[l.folder].path.clone();
        let names: Vec<String> = scan::piece_files(&dir).into_iter().map(|(n, _)| n).collect();
        let same = names.len() == l.pieces.len() && names.iter().zip(&l.pieces).all(|(n, p)| *n == p.name);
        if !same || scan::mtime(&dir.join("page.png")) != l.page_mtime {
            let keep = self.sel_piece.map(|i| l.pieces[i].name.clone());
            self.load_folder(ctx, keep);
            self.view.piece_changed(self.loaded.as_ref().zip(self.sel_piece).map(|(l, i)| &l.pieces[i]));
            return;
        }
        let mut any = false;
        for p in &mut l.pieces {
            if scan::mtime(&p.path) != p.mtime {
                load_piece(ctx, p, l.page.as_ref());
                any = true;
            }
        }
        if any {
            self.folder_changed[l.folder] = Some(l.pieces.iter().any(|p| p.changed));
            self.view.piece_changed(self.sel_piece.map(|i| &l.pieces[i]));
        }
        self.view.poll(self.sel_piece.map(|i| &l.pieces[i]));
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
            self.focus_filter = true;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let key = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        let step = |list: &[usize], at: Option<usize>, by: isize| -> Option<usize> {
            let pos = at.and_then(|a| list.iter().position(|&x| x == a));
            let next = match pos {
                Some(p) => (p as isize + by).clamp(0, list.len() as isize - 1) as usize,
                None => 0,
            };
            list.get(next).copied()
        };
        for (k, by) in [(Key::ArrowUp, -1), (Key::W, -1), (Key::ArrowDown, 1), (Key::S, 1)] {
            if key(k)
                && let Some(f) = step(&self.visible_folders(), self.sel_folder, by)
                    && Some(f) != self.sel_folder {
                        self.select_folder(ctx, f);
                    }
        }
        for (k, by) in [(Key::ArrowLeft, -1), (Key::A, -1), (Key::ArrowRight, 1), (Key::D, 1)] {
            if key(k)
                && let Some(p) = step(&self.visible_pieces(), self.sel_piece, by) {
                    self.sel_piece = Some(p);
                    self.select_piece_changed();
                }
        }
        if key(Key::G) {
            self.view.grid = !self.view.grid;
        }
        if key(Key::O) {
            self.view.original = !self.view.original;
        }
        if key(Key::E) {
            self.open_selected();
        }
        if key(Key::Plus) || key(Key::Equals) {
            self.view.zoom_by(1.25);
        }
        if key(Key::Minus) {
            self.view.zoom_by(0.8);
        }
        if key(Key::Num0) {
            self.view.fit();
        }
    }

    pub fn open_selected(&self) {
        if let Some((l, i)) = self.loaded.as_ref().zip(self.sel_piece) {
            let _ = open::that_detached(&l.pieces[i].path);
        }
    }

    fn folder_list(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.folder_filter).hint_text("filter folders"));
        ui.checkbox(&mut self.only_changed, "only changed");
        ui.separator();
        let visible = self.visible_folders();
        let mut pick = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut mission = None;
            for i in visible {
                let f = &self.folders[i];
                if mission != Some(&f.mission) {
                    mission = Some(&f.mission);
                    ui.add_space(4.0);
                    ui.strong(&f.mission);
                }
                let mut text = egui::RichText::new(format!("  {}", f.page));
                if self.folder_changed[i] == Some(true) {
                    text = text.color(MARK);
                }
                let r = ui.selectable_label(self.sel_folder == Some(i), text).on_hover_text(&f.name);
                if r.clicked() {
                    pick = Some(i);
                }
                if self.sel_folder == Some(i) && self.scroll_folder {
                    r.scroll_to_me(None);
                }
            }
        });
        self.scroll_folder = false;
        if let Some(i) = pick {
            self.select_folder(ui.ctx(), i);
            self.scroll_folder = false;
        }
    }

    fn piece_list(&mut self, ui: &mut egui::Ui) {
        let filter = ui.add(egui::TextEdit::singleline(&mut self.piece_filter).hint_text("filter pieces (Ctrl+F)"));
        if self.focus_filter {
            filter.request_focus();
            self.focus_filter = false;
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.kind, Kind::All, "all");
            ui.selectable_value(&mut self.kind, Kind::Models, "models");
            ui.selectable_value(&mut self.kind, Kind::Terrain, "terrain");
        });
        ui.separator();
        let visible = self.visible_pieces();
        let Some(l) = &self.loaded else { return };
        const CELL: egui::Vec2 = vec2(100.0, 118.0);
        let gap = ui.spacing().item_spacing;
        let cols = ((ui.available_width() + gap.x) / (CELL.x + gap.x)).floor().max(1.0) as usize;
        let rows = visible.len().div_ceil(cols);
        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if self.scroll_piece {
            if let Some(pos) = self.sel_piece.and_then(|s| visible.iter().position(|&v| v == s)) {
                let row = (pos / cols) as f32;
                let target = row * (CELL.y + gap.y) - (ui.available_height() - CELL.y) / 2.0;
                area = area.vertical_scroll_offset(target.max(0.0));
            }
            self.scroll_piece = false;
        }
        let mut pick = None;
        area.show_rows(ui, CELL.y, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for &i in visible.iter().skip(row * cols).take(cols) {
                        let p = &l.pieces[i];
                        let (rect, r) = ui.allocate_exact_size(CELL, Sense::click());
                        let painter = ui.painter_at(rect);
                        if self.sel_piece == Some(i) {
                            painter.rect_filled(rect, 4.0, ui.visuals().selection.bg_fill);
                        } else if r.hovered() {
                            painter.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        let image = egui::Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(92.0, 92.0));
                        if let Some(tex) = &p.tex {
                            let (w, h) = ((p.rect[2] - p.rect[0]) as f32, (p.rect[3] - p.rect[1]) as f32);
                            let k = (92.0 / w).min(92.0 / h);
                            let shown = egui::Rect::from_center_size(image.center(), vec2(w * k, h * k));
                            view::checker(&painter, &self.checker, shown);
                            painter.image(tex.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        } else {
                            painter.text(image.center(), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(20.0), Color32::RED);
                        }
                        if p.changed {
                            painter.circle_filled(rect.right_top() + vec2(-9.0, 9.0), 5.0, MARK);
                        }
                        let colour = if p.terrain() { TERRAIN } else { ui.visuals().text_color() };
                        painter.text(egui::pos2(rect.center().x, rect.bottom() - 11.0), egui::Align2::CENTER_CENTER, p.label(), egui::FontId::proportional(11.0), colour);
                        if r.on_hover_text(&p.name).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            }
        });
        if let Some(i) = pick {
            self.sel_piece = Some(i);
            self.select_piece_changed();
            self.scroll_piece = false;
        }
    }

    fn no_root(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(100.0);
            ui.heading("Where are the pieces?");
            ui.label("The folder `fcop export` writes: game_files/Textures/pieces (or drop it onto this window)");
            ui.add_space(8.0);
            ui.add(egui::TextEdit::singleline(&mut self.root_text).desired_width(600.0));
            if ui.button("Open").clicked() {
                let p = PathBuf::from(self.root_text.trim());
                if p.exists() {
                    self.open_path(ui.ctx(), p);
                }
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(rx) = &self.changed_rx {
            for (i, any) in rx.try_iter() {
                // the loaded folder knows better (it may have been saved since)
                if self.loaded.as_ref().is_none_or(|l| l.folder != i) {
                    self.folder_changed[i] = Some(any);
                }
            }
        }
        let dropped: Option<PathBuf> = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(p) = dropped {
            self.open_path(&ctx, p);
        }
        if self.root.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.no_root(ui));
            return;
        }
        self.poll(&ctx);
        self.keys(&ctx);
        egui::Panel::left("folders").resizable(true).default_size(200.0).show(ui, |ui| self.folder_list(ui));
        egui::Panel::left("pieces").resizable(true).default_size(330.0).show(ui, |ui| self.piece_list(ui));
        egui::CentralPanel::default().show(ui, |ui| view::detail(self, ui));
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

pub fn texture(ctx: &egui::Context, name: &str, img: &Image) -> TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.rgba);
    ctx.load_texture(name, image, TextureOptions::NEAREST)
}

fn load_piece(ctx: &egui::Context, p: &mut Piece, page: Option<&Image>) {
    p.mtime = scan::mtime(&p.path);
    p.img = scan::load_png(&p.path).ok();
    p.tex = p.img.as_ref().map(|img| texture(ctx, &p.name, img));
    p.changed = match (&p.img, page) {
        (Some(img), Some(page)) => scan::changed(img, page, p.rect),
        _ => false,
    };
}
