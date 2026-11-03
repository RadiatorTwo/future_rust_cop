//! `FCOP_RECORD=file.txt`: the player's input is written there as `FCOP_INPUT` items, one a
//! line, to be played again with `FCOP_INPUT=@file.txt` (as rustschweine's HOGS_RECORD and
//! rust_metal's GUNMETAL_RECORD).
//!
//! Items are at ms since the game's start, as FCOP_INPUT counts them. Recorded: keys with how
//! long they were held (`Up*1500`), mouse buttons with where and how long (`click@x,y*n`,
//! `rclick`, `mclick`, `x1click`, `x2click`), the mouse's moves at most every 50 ms
//! (`move@x,y`), the wheel (`wheel@n`), typed characters (`text@c`, `char@hex`), the window's
//! focus and its close button, and the first real gamepad (`pad@A*n`, `pad@LX=0.5*n`: its
//! buttons and its sticks in steps of 0.05; a replay plays it on the scripted pad, which player 1
//! uses). Repeats of a held key are not written (the hold is).
//!
//! F11 marks a moment: a comment line with the time, and the picture shown is written to
//! `file.txt.<ms>.png`. F11 does not reach the game while recording.

use super::log;
use super::window::Input;
use std::io::Write;
use std::sync::Mutex;

/// The pad's controls (`dinput::CONTROLS`).
const PAD_CONTROLS: usize = 20;

/// The items for the input events, in order (pure: what the file gets).
#[derive(Default)]
pub struct Recorder {
    /// keys and mouse buttons held: their item (without its hold) and since when
    held: Vec<(String, u64)>,
    /// the mouse in frame pixels, and the last move written: when, where
    mouse: (i32, i32),
    moved: Option<(u64, i32, i32)>,
    /// the pad's controls as last seen, and since when (a value of 0 is not held)
    pad: [(f32, u64); PAD_CONTROLS],
}

impl Recorder {
    /// The lines for one input event at `t` (ms since start).
    pub fn event(&mut self, i: &Input, t: u64) -> Vec<String> {
        let mut out = Vec::new();
        match *i {
            Input::Move(x, y) => {
                self.mouse = (x, y);
                if self.moved.is_none_or(|(m, mx, my)| t >= m + 50 && (mx, my) != (x, y)) {
                    self.moved = Some((t, x, y));
                    out.push(format!("{t}:move@{x},{y}"));
                }
            }
            Input::Key { vk, scan, down, repeat: false } => {
                if let Some(name) = super::window::key_name(vk, scan) {
                    self.press(name, down, t, &mut out);
                }
            }
            Input::Button(b, down) => {
                let Some(kind) = ["click", "rclick", "mclick", "x1click", "x2click"].get(b as usize) else { return out };
                let (x, y) = self.mouse;
                // the item's own position: a scripted click moves there 30 ms before it presses
                let name = format!("{kind}@{x},{y}");
                if down {
                    self.release(kind, t, &mut out);
                    self.held.push((name, t.saturating_sub(30)));
                } else {
                    self.release(kind, t, &mut out);
                }
            }
            Input::Char(ch) => out.push(match ch {
                b'!'..=b'~' if ch != b'#' && ch != b'_' => format!("{t}:text@{}", ch as char),
                b' ' => format!("{t}:text@_"),
                _ => format!("{t}:char@{ch:02x}"),
            }),
            Input::Wheel(n) => out.push(format!("{t}:wheel@{n}")),
            Input::Focus(f) => out.push(format!("{t}:{}", if f { "focus" } else { "blur" })),
            Input::Close => {
                // what is still held ends here
                for (name, since) in std::mem::take(&mut self.held) {
                    out.push(format!("{since}:{name}*{}", t.saturating_sub(since).max(1)));
                }
                out.extend(self.pad(&[0.0; PAD_CONTROLS], t));
                out.push(format!("{t}:close"));
            }
            _ => {}
        }
        out
    }

    fn press(&mut self, name: String, down: bool, t: u64, out: &mut Vec<String>) {
        let k = self.held.iter().position(|h| h.0 == name);
        match (down, k) {
            (true, None) => self.held.push((name, t)),
            (false, Some(k)) => {
                let (name, since) = self.held.remove(k);
                out.push(format!("{since}:{name}*{}", t.saturating_sub(since).max(1)));
            }
            _ => {}
        }
    }

    /// A mouse button of this kind lets go.
    fn release(&mut self, kind: &str, t: u64, out: &mut Vec<String>) {
        let at = format!("{kind}@");
        if let Some(k) = self.held.iter().position(|h| h.0.starts_with(&at)) {
            let (name, since) = self.held.remove(k);
            // held from 30 ms after the item's time
            out.push(format!("{since}:{name}*{}", t.saturating_sub(since + 30).max(1)));
        }
    }

    /// The pad's controls now (`dinput::CONTROLS` order; buttons 0 or 1, axes -1...1, y up).
    pub fn pad(&mut self, v: &[f32; PAD_CONTROLS], t: u64) -> Vec<String> {
        let mut out = Vec::new();
        for (k, (&new, (old, since))) in v.iter().zip(self.pad.iter_mut()).enumerate() {
            // steps of 0.05, so a stick held still is one item; a resting stick's noise is 0
            let new = if new.abs() < 0.1 { 0.0 } else { (new.clamp(-1.0, 1.0) * 20.0).round() / 20.0 };
            if new == *old {
                continue;
            }
            if *old != 0.0 {
                let name = super::dinput::CONTROLS[k];
                let hold = t.saturating_sub(*since).max(1);
                out.push(if k < 16 { format!("{since}:pad@{name}*{hold}") } else { format!("{since}:pad@{name}={old}*{hold}") });
            }
            (*old, *since) = (new, t);
        }
        out
    }
}

struct File {
    file: std::fs::File,
    path: String,
    rec: Recorder,
}

static RECORD: std::sync::LazyLock<Mutex<Option<File>>> = std::sync::LazyLock::new(|| {
    Mutex::new((|| {
        let path = std::env::var("FCOP_RECORD").ok().filter(|p| !p.is_empty())?;
        let file = std::fs::File::create(&path).map_err(|e| log!("FCOP_RECORD: cannot write {path}: {e}")).ok()?;
        log!("FCOP_RECORD: the input goes to {path} (F11: a mark with a picture; play it with FCOP_INPUT=@{path})");
        Some(File { file, path, rec: Recorder::default() })
    })())
});

/// Recording is on.
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| RECORD.lock().unwrap().is_some())
}

fn now() -> u64 {
    super::kernel32::start_time().elapsed().as_millis() as u64
}

fn write(f: &mut File, lines: Vec<String>) {
    for l in lines {
        let _ = writeln!(f.file, "{l}");
    }
}

/// A real input event (before it reaches the game).
pub fn input(i: &Input) {
    if !on() {
        return;
    }
    let mut r = RECORD.lock().unwrap();
    let Some(f) = r.as_mut() else { return };
    let lines = f.rec.event(i, now());
    write(f, lines);
    if matches!(i, Input::Close) {
        let _ = f.file.flush();
    }
}

/// The first real pad's controls, at every read of the pads.
pub fn pad(v: &[f32; PAD_CONTROLS]) {
    if !on() {
        return;
    }
    let mut r = RECORD.lock().unwrap();
    let Some(f) = r.as_mut() else { return };
    let lines = f.rec.pad(v, now());
    write(f, lines);
}

/// F11: a mark (a comment) and the path for a picture of what is shown.
pub fn mark() -> Option<String> {
    let mut r = RECORD.lock().unwrap();
    let f = r.as_mut()?;
    let ms = now();
    let _ = writeln!(f.file, "# mark {ms} ms");
    let _ = f.file.flush();
    let shot = format!("{}.{ms:06}.png", f.path);
    log!("FCOP_RECORD: mark at {ms} ms: {shot}");
    Some(shot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_clicks() {
        let mut r = Recorder::default();
        let up = Input::Key { vk: 0x26, scan: 0x48, down: true, repeat: false };
        assert!(r.event(&up, 1000).is_empty());
        assert!(r.event(&Input::Key { vk: 0x26, scan: 0x48, down: true, repeat: true }, 1100).is_empty());
        assert_eq!(r.event(&Input::Key { vk: 0x26, scan: 0x48, down: false, repeat: false }, 2500), ["1000:Up*1500"]);
        assert_eq!(r.event(&Input::Move(484, 303), 3000), ["3000:move@484,303"]);
        assert!(r.event(&Input::Button(0, true), 3100).is_empty());
        assert_eq!(r.event(&Input::Button(0, false), 3200), ["3070:click@484,303*100"]);
        assert_eq!(r.event(&Input::Char(b'A'), 4000), ["4000:text@A"]);
        assert_eq!(r.event(&Input::Char(b' '), 4050), ["4050:text@_"]);
    }

    #[test]
    fn pad() {
        let mut r = Recorder::default();
        let mut v = [0.0; PAD_CONTROLS];
        v[3] = 1.0;
        v[16] = 0.51;
        assert!(r.pad(&v, 100).is_empty());
        v[16] = 0.49;
        assert!(r.pad(&v, 150).is_empty());
        v[3] = 0.0;
        v[16] = 0.0;
        assert_eq!(r.pad(&v, 600), ["100:pad@A*500", "100:pad@LX=0.5*500"]);
    }
}
