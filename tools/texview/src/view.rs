//! The selected piece: zoomable view (UV grid, the original beside it), where it lies on its
//! page, its JSON.

use crate::scan::{self, Faces};
use crate::{App, MARK, MODEL, Piece, TERRAIN};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2, pos2, vec2};
use std::time::SystemTime;

pub struct View {
    /// screen pixels per texel of the original; 0 = fit
    zoom: f32,
    offset: Vec2,
    pub grid: bool,
    pub original: bool,
    faces: Option<Faces>,
    json_mtime: Option<SystemTime>,
    /// the face under the mouse (image or JSON) and the one clicked
    hover: Option<usize>,
    picked: Option<usize>,
    scroll_json: bool,
    /// the last frame's fitting scale (for zooming from fit)
    fit_scale: f32,
}

impl Default for View {
    fn default() -> View {
        View { zoom: 0.0, offset: Vec2::ZERO, grid: true, original: true, faces: None, json_mtime: None, hover: None, picked: None, scroll_json: false, fit_scale: 1.0 }
    }
}

impl View {
    /// Another piece selected (or the selected one reloaded): its JSON read again.
    pub fn piece_changed(&mut self, piece: Option<&Piece>) {
        let json = piece.map(|p| p.path.with_extension("json"));
        self.faces = json.as_deref().map(scan::load_faces);
        self.json_mtime = json.as_deref().and_then(scan::mtime);
        self.hover = None;
        if self.picked.is_some_and(|f| self.faces.as_ref().is_none_or(|faces| f >= faces.faces.len())) {
            self.picked = None;
        }
    }

    /// A new piece selected: view fitted, nothing picked.
    pub fn reset(&mut self) {
        self.fit();
        self.picked = None;
    }

    pub fn poll(&mut self, piece: Option<&Piece>) {
        let json = piece.map(|p| p.path.with_extension("json"));
        if json.as_deref().and_then(scan::mtime) != self.json_mtime {
            self.piece_changed(piece);
        }
    }

    pub fn fit(&mut self) {
        self.zoom = 0.0;
        self.offset = Vec2::ZERO;
    }

    pub fn zoom_by(&mut self, k: f32) {
        let s = if self.zoom == 0.0 { self.fit_scale } else { self.zoom };
        self.zoom = (s * k).clamp(0.25, 64.0);
        self.offset *= self.zoom / s;
    }
}

/// Grey squares behind `rect` (transparent texels).
pub fn checker(painter: &egui::Painter, tex: &TextureHandle, rect: Rect) {
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(rect.width() / 16.0, rect.height() / 16.0));
    painter.image(tex.id(), rect, uv, Color32::WHITE);
}

const FULL: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

pub fn detail(app: &mut App, ui: &mut egui::Ui) {
    let App { loaded, sel_piece, view, checker, .. } = app;
    let Some((l, sel)) = loaded.as_ref().zip(*sel_piece) else {
        ui.label("no piece selected");
        return;
    };
    let p = &l.pieces[sel];
    let (w, h) = (p.rect[2] - p.rect[0], p.rect[3] - p.rect[1]);
    let mut open = false;
    ui.horizontal(|ui| {
        ui.strong(&p.name);
        let mut info = format!("{w}x{h} texels at {},{}", p.rect[0], p.rect[1]);
        if let Some(img) = &p.img
            && (img.w, img.h) != (w as u32, h as u32) {
                info += &format!(", file {}x{}", img.w, img.h);
            }
        ui.label(info);
        if p.changed {
            ui.colored_label(MARK, "changed");
        } else {
            ui.weak("unchanged");
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut view.grid, "UV grid (G)");
        ui.checkbox(&mut view.original, "original beside it (O)");
        if ui.button("fit (0)").clicked() {
            view.fit();
        }
        if ui.button("open in editor (E)").clicked() {
            open = true;
        }
        if ui.button("open folder").clicked() {
            let _ = open::that_detached(p.path.parent().unwrap());
        }
        if ui.button("copy path").clicked() {
            ui.ctx().copy_text(p.path.display().to_string());
        }
    });
    ui.separator();

    let mut hover = None;
    let mut clicked_page = None;
    let mut clicked_face = None;
    egui::Panel::bottom("info").resizable(true).default_size(320.0).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            clicked_page = page_map(ui, checker, l, sel);
            ui.separator();
            ui.vertical(|ui| hover = json(ui, view));
        });
    });
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
        let (h_image, c) = image_view(ui, view, checker, l, p, hover);
        hover = hover.or(h_image);
        clicked_face = c;
    });
    let v = view;
    v.hover = hover;
    if let Some(c) = clicked_face {
        v.picked = c;
        v.scroll_json = c.is_some();
    }
    if open {
        app.open_selected();
    }
    if let Some(i) = clicked_page {
        app.sel_piece = Some(i);
        app.view.piece_changed(app.loaded.as_ref().map(|l| &l.pieces[i]));
        app.view.reset();
    }
}

/// The piece (and the original beside it), zoomed and panned; returns the face under the
/// mouse and, when clicked, the face clicked (None: none there).
fn image_view(ui: &mut egui::Ui, v: &mut View, checker_tex: &TextureHandle, l: &crate::Loaded, p: &Piece, json_hover: Option<usize>) -> (Option<usize>, Option<Option<usize>>) {
    let (rect, r) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let (w, h) = ((p.rect[2] - p.rect[0]) as f32, (p.rect[3] - p.rect[1]) as f32);
    let original = v.original && p.changed;
    let n = if original { 2.0 } else { 1.0 };
    let gap = 16.0;
    v.fit_scale = ((rect.width() - gap * (n + 1.0)) / (w * n)).min((rect.height() - 2.0 * gap) / h).max(0.01);
    let mut s = if v.zoom == 0.0 { v.fit_scale } else { v.zoom };
    // the centres of the views: one, or two side by side
    let centre = |k: f32| pos2(rect.left() + rect.width() * (2.0 * k + 1.0) / (2.0 * n), rect.center().y);
    let origin = |k: f32, s: f32, offset: Vec2| centre(k) + offset - vec2(w, h) * s / 2.0;

    if r.dragged() {
        v.offset += r.drag_delta();
    }
    if let Some(m) = r.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let k = if original && m.x > rect.center().x { 1.0 } else { 0.0 };
            let s1 = (s * (scroll / 200.0).exp()).clamp(0.25, 64.0);
            // the point under the mouse stays
            let o = origin(k, s, v.offset);
            let o1 = m - (m - o) * (s1 / s);
            v.offset += o1 - origin(k, s1, v.offset);
            v.zoom = s1;
            s = s1;
        }
    }

    let mut hover = None;
    let faces = v.faces.as_ref();
    for k in 0..n as usize {
        let o = origin(k as f32, s, v.offset);
        let shown = Rect::from_min_size(o, vec2(w, h) * s);
        checker(&painter, checker_tex, shown.intersect(rect));
        let tex = if k == 0 { p.tex.as_ref() } else { None };
        match (k, tex) {
            (0, Some(tex)) => {
                painter.image(tex.id(), shown, FULL, Color32::WHITE);
            }
            (1, _) => {
                // the original: the piece's part of page.png
                if let Some(page) = &l.page_tex {
                    let uv = Rect::from_min_max(pos2(p.rect[0] as f32, p.rect[1] as f32) / 256.0, pos2(p.rect[2] as f32, p.rect[3] as f32) / 256.0);
                    painter.image(page.id(), shown, uv, Color32::WHITE);
                }
                painter.text(shown.center_top() - vec2(0.0, 4.0), egui::Align2::CENTER_BOTTOM, "original", egui::FontId::proportional(13.0), Color32::GRAY);
            }
            _ => {}
        }
        if k == 0 && original {
            painter.text(shown.center_top() - vec2(0.0, 4.0), egui::Align2::CENTER_BOTTOM, "now", egui::FontId::proportional(13.0), MARK);
        }
        let Some(faces) = faces else { continue };
        let to_screen = |c: &[f32; 2]| o + vec2(c[0] * w, c[1] * h) * s;
        if let Some(m) = r.hover_pos().filter(|m| shown.contains(*m)) {
            let at = [(m.x - o.x) / (w * s), (m.y - o.y) / (h * s)];
            // the smallest face there
            hover = faces
                .faces
                .iter()
                .enumerate()
                .filter(|(_, (_, uv))| inside(uv, at))
                .min_by(|a, b| area(&a.1.1).total_cmp(&area(&b.1.1)))
                .map(|(i, _)| i);
        }
        let lit = json_hover.or(hover).or(v.picked);
        if v.grid {
            for (i, (_, uv)) in faces.faces.iter().enumerate() {
                let points: Vec<Pos2> = uv.iter().map(to_screen).collect();
                if Some(i) != lit {
                    painter.add(egui::Shape::closed_line(points, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 230, 0, 150))));
                }
            }
        }
        if let Some((_, uv)) = lit.and_then(|i| faces.faces.get(i)) {
            let points: Vec<Pos2> = uv.iter().map(to_screen).collect();
            painter.add(egui::Shape::convex_polygon(points.clone(), Color32::from_rgba_unmultiplied(255, 80, 0, 60), Stroke::NONE));
            painter.add(egui::Shape::closed_line(points, Stroke::new(2.0, Color32::from_rgb(255, 80, 0))));
        }
    }
    let clicked = r.clicked().then_some(hover);
    (hover, clicked)
}

/// Whether point `at` is inside the polygon.
fn inside(poly: &[[f32; 2]], at: [f32; 2]) -> bool {
    let mut odd = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > at[1]) != (b[1] > at[1]) && at[0] < (b[0] - a[0]) * (at[1] - a[1]) / (b[1] - a[1]) + a[0] {
            odd = !odd;
        }
        j = i;
    }
    odd
}

fn area(poly: &[[f32; 2]]) -> f32 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a.abs() / 2.0
}

/// The page with every piece outlined (models magenta, terrain cyan) and the selected one
/// bright; a click selects the smallest piece there.
fn page_map(ui: &mut egui::Ui, checker_tex: &TextureHandle, l: &crate::Loaded, sel: usize) -> Option<usize> {
    let size = (ui.available_height() - 4.0).clamp(128.0, 512.0);
    let (rect, r) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let painter = ui.painter_at(rect);
    checker(&painter, checker_tex, rect);
    if let Some(tex) = &l.page_tex {
        painter.image(tex.id(), rect, FULL, Color32::WHITE);
    }
    let k = size / 256.0;
    let to_rect = |r: [u16; 4]| Rect::from_min_max(rect.min + vec2(r[0] as f32, r[1] as f32) * k, rect.min + vec2(r[2] as f32, r[3] as f32) * k);
    for p in &l.pieces {
        let c = if p.terrain() { TERRAIN } else { MODEL };
        painter.rect_stroke(to_rect(p.rect), 0.0, Stroke::new(1.0, c.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    }
    let s = to_rect(l.pieces[sel].rect);
    painter.rect_stroke(s.expand(1.0), 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
    painter.rect_stroke(s, 0.0, Stroke::new(2.0, Color32::YELLOW), egui::StrokeKind::Outside);
    let m = r.interact_pointer_pos().filter(|_| r.clicked())?;
    let at = (m - rect.min) / k;
    l.pieces
        .iter()
        .enumerate()
        .filter(|(_, p)| (p.rect[0] as f32..p.rect[2] as f32).contains(&at.x) && (p.rect[1] as f32..p.rect[3] as f32).contains(&at.y))
        .min_by_key(|(_, p)| (p.rect[2] - p.rect[0]) as u32 * (p.rect[3] - p.rect[1]) as u32)
        .map(|(i, _)| i)
}

/// The JSON, a face a line; returns the face whose line is under the mouse.
fn json(ui: &mut egui::Ui, v: &mut View) -> Option<usize> {
    let Some(faces) = &v.faces else {
        ui.label("no JSON");
        return None;
    };
    ui.label(format!("{} faces", faces.faces.len()));
    let line_face: std::collections::HashMap<usize, usize> = faces.faces.iter().enumerate().map(|(i, (line, _))| (*line, i)).collect();
    let lit = v.hover.or(v.picked);
    let row = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
    let mut area = egui::ScrollArea::both().auto_shrink(false);
    if v.scroll_json {
        if let Some((line, _)) = v.picked.and_then(|f| faces.faces.get(f)) {
            area = area.vertical_scroll_offset((*line as f32 * row - ui.available_height() / 2.0).max(0.0));
        }
        v.scroll_json = false;
    }
    let mut hover = None;
    area.show_rows(ui, row - ui.spacing().item_spacing.y, faces.lines.len(), |ui, range| {
        for i in range {
            let face = line_face.get(&i).copied();
            let mut text = egui::RichText::new(&faces.lines[i]).monospace();
            if face.is_some() && face == lit {
                text = text.background_color(Color32::from_rgba_unmultiplied(255, 80, 0, 90));
            }
            let r = ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend).sense(Sense::hover()));
            if r.hovered() && face.is_some() {
                hover = face;
            }
        }
    });
    hover
}
