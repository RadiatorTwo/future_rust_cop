//! Our own settings inside the game's dialogs. For each dialog (by template id):
//! - `adjust` changes the template before the dialog is built: add our controls (ids from
//!   `FIRST_OURS`), move the game's, make the dialog larger;
//! - `init` runs after the game's WM_INITDIALOG: fill our controls, hide game controls;
//! - `command` handles clicks on our controls (the game's procedure never sees them);
//! - `ok` runs when the dialog's OK button is pressed: save our settings.
//!
//! Settings are REG_DWORD/REG_SZ values under the game's own key (`advapi32::FCOP_KEY`).

use super::template::{Class, Item, Template};
use super::{Dialog, FIRST_OURS, Kind};
use crate::platform::advapi32;

/// The Preferences dialog.
const PREFERENCES: u32 = 101;
/// Its "Processor Select" label and combo box (the game hides them): our renderer choice goes there.
const PROCESSOR_LABEL: u32 = 1111;
const PROCESSOR_COMBO: u32 = 1023;

/// Its Display Mode list (the game's resolutions up to 1280x960): with Glide, our list of the
/// display's modes takes its place (`display.rs`).
const DISPLAY_MODE: u32 = 1021;
/// Its FullScreen box (the game hides it with Glide): ours goes there.
const GAME_FULLSCREEN: u32 = 1012;
/// Its "Filtering" box in the 3DFX Options group: our "Widescreen" box goes next to it.
const FILTERING: u32 = 1013;
/// Its 3DFX Options group: our frame rate, texture filtering and anti-aliasing go below it.
const OPTIONS_GROUP: u32 = 1125;
/// Its OK and Cancel buttons: they move down a row for ours.
const OK: u32 = 1001;
const CANCEL: u32 = 1002;

const RENDERER_LABEL: u32 = FIRST_OURS;
const RENDERER: u32 = FIRST_OURS + 1;
const WIDESCREEN: u32 = FIRST_OURS + 2;
const RESOLUTION: u32 = FIRST_OURS + 3;
const FULLSCREEN: u32 = FIRST_OURS + 4;
const FRAME_RATE_LABEL: u32 = FIRST_OURS + 5;
const FRAME_RATE: u32 = FIRST_OURS + 6;
const TEXTURES_LABEL: u32 = FIRST_OURS + 7;
const TEXTURES: u32 = FIRST_OURS + 8;
const MSAA_LABEL: u32 = FIRST_OURS + 9;
const MSAA: u32 = FIRST_OURS + 10;

/// The Join Network Game dialog: our host address goes below its Connection Status group,
/// its JOIN and CANCEL buttons move down.
const JOIN: u32 = 167;
const JOIN_STATUS: u32 = 1144;
const JOIN_BUTTON: u32 = 1139;
const JOIN_CANCEL: u32 = 1140;
const ADDRESS_GROUP: u32 = FIRST_OURS + 11;
const ADDRESS: u32 = FIRST_OURS + 12;

const VISIBLE: u32 = 0x1000_0000;
const TABSTOP: u32 = 0x0001_0000;

/// The renderers a player can choose (the registry's `Group` value of the game's key).
const RENDERERS: &[&str] = &["Software", "3Dfx"];

fn renderer() -> String {
    advapi32::value("", "Group")
        .map(|(_, d)| d.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect())
        .unwrap_or_else(|| "3Dfx".into())
}

pub fn adjust(template: u32, t: &mut Template) {
    if template == JOIN {
        let Some(g) = t.items.iter().find(|i| i.id == JOIN_STATUS).map(|i| i.rect) else { return };
        let y = g[1] + g[3] + 4;
        t.items.push(Item { class: Class::Button, id: ADDRESS_GROUP, text: "Host Address (empty: local network)".into(), style: VISIBLE | 7, ex_style: 0, rect: [g[0], y, g[2], g[3]] });
        // like the game's edit boxes: a border, scrolling sideways
        t.items.push(Item { class: Class::Edit, id: ADDRESS, text: String::new(), style: VISIBLE | TABSTOP | 0x0080_0080, ex_style: 0, rect: [g[0] + 7, y + 12, g[2] - 14, 14] });
        let down = g[3] + 4;
        for i in t.items.iter_mut().filter(|i| i.id == JOIN_BUTTON || i.id == JOIN_CANCEL) {
            i.rect[1] += down;
        }
        t.rect[3] += down;
    }
    if template == PREFERENCES {
        let place = |id: u32| t.items.iter().find(|i| i.id == id).map(|i| i.rect);
        let (Some(label), Some(combo)) = (place(PROCESSOR_LABEL), place(PROCESSOR_COMBO)) else { return };
        let filtering = place(FILTERING);
        let display_mode = place(DISPLAY_MODE);
        let game_fullscreen = place(GAME_FULLSCREEN);
        let group = place(OPTIONS_GROUP);
        // wider than the game's label: up to the 3DFX Options group (x 132)
        t.items.push(Item { class: Class::Static, id: RENDERER_LABEL, text: "Renderer (needs restart)".into(), style: VISIBLE, ex_style: 0, rect: [label[0], label[1], 122, label[3]] });
        t.items.push(Item { class: Class::ComboBox, id: RENDERER, text: String::new(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect: combo });
        if let Some(r) = display_mode {
            t.items.push(Item { class: Class::ComboBox, id: RESOLUTION, text: String::new(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect: r });
        }
        if let Some(r) = game_fullscreen {
            // centred on the Display Mode list's row (one unit lower than the game's box: its
            // square sits centred in it), left-aligned with Filtering below it
            let x = filtering.map_or(r[0], |f| f[0]);
            let y = display_mode.map_or(r[1], |m| m[1]);
            t.items.push(Item { class: Class::Button, id: FULLSCREEN, text: "Fullscreen".into(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect: [x, y, r[2], r[3] - 1] });
        }
        if let Some(f) = filtering {
            // an auto check box (BS_AUTOCHECKBOX) right of Filtering
            let rect = [f[0] + 58, f[1], 70, f[3]];
            t.items.push(Item { class: Class::Button, id: WIDESCREEN, text: "Widescreen".into(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect });
        }
        if let Some(g) = group {
            // between the group and the OK button's row, the combo up to the group's right edge
            let y = g[1] + g[3] + 5;
            t.items.push(Item { class: Class::Static, id: FRAME_RATE_LABEL, text: "Frame Rate".into(), style: VISIBLE, ex_style: 0, rect: [g[0], y + 2, 52, 8] });
            t.items.push(Item { class: Class::ComboBox, id: FRAME_RATE, text: String::new(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect: [g[0] + 54, y, g[2] - 54, 100] });
            // one more row: OK and Cancel move down, the dialog grows
            let y = y + 15;
            for i in t.items.iter_mut().filter(|i| i.id == OK || i.id == CANCEL) {
                i.rect[1] += 15;
            }
            t.rect[3] += 15;
            t.items.push(Item { class: Class::Static, id: TEXTURES_LABEL, text: "Textures".into(), style: VISIBLE, ex_style: 0, rect: [g[0], y + 2, 52, 8] });
            t.items.push(Item { class: Class::ComboBox, id: TEXTURES, text: String::new(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect: [g[0] + 54, y, g[2] - 54, 100] });
            let y = y + 15;
            for i in t.items.iter_mut().filter(|i| i.id == OK || i.id == CANCEL) {
                i.rect[1] += 15;
            }
            t.rect[3] += 15;
            t.items.push(Item { class: Class::Static, id: MSAA_LABEL, text: "Anti-alias".into(), style: VISIBLE, ex_style: 0, rect: [g[0], y + 2, 52, 8] });
            t.items.push(Item { class: Class::ComboBox, id: MSAA, text: String::new(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect: [g[0] + 54, y, g[2] - 54, 100] });
        }
    }
}

pub fn init(d: &mut Dialog) {
    if d.template == JOIN {
        if let Some(c) = d.control(ADDRESS) {
            c.text = saved_address();
        }
    }
    if d.template == PREFERENCES {
        for id in [PROCESSOR_LABEL, PROCESSOR_COMBO] {
            if let Some(c) = d.control(id) {
                c.visible = false;
            }
        }
        let current = renderer();
        if let Some(c) = d.control(RENDERER) {
            if let Kind::Combo { items, sel, .. } = &mut c.kind {
                *items = RENDERERS.iter().map(|r| (r.to_string(), 0)).collect();
                *sel = RENDERERS.iter().position(|r| r.eq_ignore_ascii_case(&current)).map_or(0, |k| k as i32);
            }
        }
        // only the Glide renderer draws wide; the box sits in its options group
        let glide = d.control(FILTERING).is_some_and(|c| c.visible);
        if let Some(c) = d.control(WIDESCREEN) {
            c.check = crate::platform::wide::enabled() as u32;
            c.visible = glide;
        }
        // with Glide the game's resolution does not matter (we draw at ours): its list makes way
        if let Some(c) = d.control(DISPLAY_MODE) {
            c.visible &= !glide;
        }
        let full = crate::platform::window::is_fullscreen();
        if let Some(c) = d.control(FULLSCREEN) {
            c.check = full as u32;
            c.visible = glide;
        }
        let fps = crate::platform::fps::chosen();
        if let Some(c) = d.control(FRAME_RATE) {
            if let Kind::Combo { items, sel, .. } = &mut c.kind {
                *items = crate::platform::fps::CHOICES
                    .iter()
                    .map(|&f| {
                        let name = match f {
                            0 => "Original (37.5)".into(),
                            crate::platform::fps::UNLIMITED => "Unlimited".into(),
                            _ => format!("{f} fps"),
                        };
                        (name, f)
                    })
                    .collect();
                *sel = items.iter().position(|i| i.1 == fps).unwrap_or(0) as i32;
            }
        }
        // texture filtering: only Glide filters
        let filter = crate::platform::glide::filtering();
        for id in [TEXTURES_LABEL, TEXTURES] {
            if let Some(c) = d.control(id) {
                c.visible = glide;
            }
        }
        if let Some(c) = d.control(TEXTURES) {
            if let Kind::Combo { items, sel, .. } = &mut c.kind {
                *items = crate::platform::glide::FILTER_CHOICES
                    .iter()
                    .map(|&f| {
                        let name = match f {
                            0 => "Original".into(),
                            1 => "Trilinear".into(),
                            _ => format!("Anisotropic {f}x"),
                        };
                        (name, f)
                    })
                    .collect();
                *sel = items.iter().position(|i| i.1 == filter).unwrap_or(0) as i32;
            }
        }
        // anti-aliasing: Glide only, too
        let samples = crate::platform::glide::msaa();
        for id in [MSAA_LABEL, MSAA] {
            if let Some(c) = d.control(id) {
                c.visible = glide;
            }
        }
        if let Some(c) = d.control(MSAA) {
            if let Kind::Combo { items, sel, .. } = &mut c.kind {
                *items = crate::platform::glide::MSAA_CHOICES
                    .iter()
                    .map(|&n| (if n == 0 { "Off".into() } else { format!("MSAA {n}x") }, n))
                    .collect();
                *sel = items.iter().position(|i| i.1 == samples).unwrap_or(0) as i32;
            }
        }
        let (desktop, modes) = crate::platform::window::display_modes();
        let current = crate::platform::display::chosen().map_or(0, |(w, h)| w << 16 | h);
        if let Some(c) = d.control(RESOLUTION) {
            c.visible = glide;
            if let Kind::Combo { items, sel, .. } = &mut c.kind {
                items.push((format!("Desktop ({}x{})", desktop.0, desktop.1), 0));
                items.extend(modes.iter().map(|&(w, h)| (format!("{w}x{h}"), w << 16 | h)));
                if current != 0 && !items.iter().any(|i| i.1 == current) {
                    items.push((format!("{}x{}", current >> 16, current & 0xffff), current));
                }
                *sel = items.iter().position(|i| i.1 == current).unwrap_or(0) as i32;
            }
        }
    }
}

pub fn command(_d: &mut Dialog, _id: u32) {}

fn saved_address() -> String {
    advapi32::value("", "JoinAddress").map(|(_, d)| d.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect()).unwrap_or_default()
}

/// The host address typed into the Join dialog (`dplay.rs` looks for sessions there; empty:
/// on the local network), kept for the next time.
pub fn join_address() -> String {
    let Some(a) = super::text_in(JOIN, ADDRESS) else { return saved_address() };
    if a != saved_address() {
        let mut v = a.clone().into_bytes();
        v.push(0);
        advapi32::set_value("", "JoinAddress", 1, &v);
    }
    a
}

pub fn ok(d: &mut Dialog) {
    if d.template == PREFERENCES {
        let chosen = d.control(RENDERER).and_then(|c| match &c.kind {
            Kind::Combo { items, sel, .. } => items.get(*sel as usize).map(|i| i.0.clone()),
            _ => None,
        });
        if let Some(r) = chosen {
            let mut v = r.into_bytes();
            v.push(0);
            advapi32::set_value("", "Group", 1, &v);
        }
        let size = d.control(RESOLUTION).filter(|c| c.visible).and_then(|c| match &c.kind {
            Kind::Combo { items, sel, .. } => items.get(*sel as usize).map(|i| i.1),
            _ => None,
        });
        if let Some(on) = d.control(FULLSCREEN).filter(|c| c.visible).map(|c| c.check != 0) {
            crate::platform::display::set_fullscreen(on);
        }
        if let Some(v) = size {
            crate::platform::display::set_chosen((v != 0).then_some((v >> 16, v & 0xffff)));
        }
        let fps = d.control(FRAME_RATE).and_then(|c| match &c.kind {
            Kind::Combo { items, sel, .. } => items.get(*sel as usize).map(|i| i.1),
            _ => None,
        });
        if let Some(f) = fps {
            crate::platform::fps::set_chosen(f);
        }
        let filter = d.control(TEXTURES).filter(|c| c.visible).and_then(|c| match &c.kind {
            Kind::Combo { items, sel, .. } => items.get(*sel as usize).map(|i| i.1),
            _ => None,
        });
        if let Some(f) = filter {
            crate::platform::glide::set_filtering(f);
        }
        let samples = d.control(MSAA).filter(|c| c.visible).and_then(|c| match &c.kind {
            Kind::Combo { items, sel, .. } => items.get(*sel as usize).map(|i| i.1),
            _ => None,
        });
        if let Some(n) = samples {
            crate::platform::glide::set_msaa(n);
        }
        if let Some(on) = d.control(WIDESCREEN).map(|c| c.check != 0) {
            crate::platform::wide::set_enabled(on);
        }
    }
}
