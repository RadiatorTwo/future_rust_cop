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

const RENDERER_LABEL: u32 = FIRST_OURS;
const RENDERER: u32 = FIRST_OURS + 1;

const VISIBLE: u32 = 0x1000_0000;
const TABSTOP: u32 = 0x0001_0000;

/// The renderers a player can choose (the registry's `Group` value of the game's key).
const RENDERERS: &[&str] = &["Software"];

fn renderer() -> String {
    advapi32::value("", "Group")
        .map(|(_, d)| d.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect())
        .unwrap_or_else(|| "Software".into())
}

pub fn adjust(template: u32, t: &mut Template) {
    if template == PREFERENCES {
        let place = |id: u32| t.items.iter().find(|i| i.id == id).map(|i| i.rect);
        let (Some(label), Some(combo)) = (place(PROCESSOR_LABEL), place(PROCESSOR_COMBO)) else { return };
        t.items.push(Item { class: Class::Static, id: RENDERER_LABEL, text: "Renderer (next start)".into(), style: VISIBLE, ex_style: 0, rect: label });
        t.items.push(Item { class: Class::ComboBox, id: RENDERER, text: String::new(), style: VISIBLE | TABSTOP | 3, ex_style: 0, rect: combo });
    }
}

pub fn init(d: &mut Dialog) {
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
    }
}

pub fn command(_d: &mut Dialog, _id: u32) {}

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
    }
}
