//! The game window (winit) and the frames shown in it (softbuffer). Everything here lives on
//! the game's main thread: the game creates its window, pumps its messages and presents its
//! frames there.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::Duration;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::{Window, WindowId};

/// Input for the game, in game terms: Windows virtual-key codes and frame pixels.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    Key { vk: u8, scan: u8, down: bool, repeat: bool },
    Char(u8),
    Move(i32, i32),
    /// 0 left, 1 right, 2 middle
    Button(u8, bool),
    Close,
    Focus(bool),
}

struct App {
    want: Option<String>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    input: VecDeque<Input>,
    /// the last frame shown: 0RGB pixels, width, height
    frame: (Vec<u32>, u32, u32),
    cursor: PhysicalPosition<f64>,
}

struct Host {
    event_loop: EventLoop<()>,
    app: App,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// The thread that owns the window (the game's main thread), and a frame another thread
/// presented, shown at the main thread's next pump.
static MAIN: OnceLock<ThreadId> = OnceLock::new();
static PENDING: Mutex<Option<(Vec<u32>, u32, u32)>> = Mutex::new(None);

fn on_main() -> bool {
    *MAIN.get_or_init(|| std::thread::current().id()) == std::thread::current().id()
}

fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> R {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let host = h.get_or_insert_with(|| {
            #[allow(unused_mut)]
            let mut b = EventLoop::builder();
            #[cfg(windows)]
            winit::platform::windows::EventLoopBuilderExtWindows::with_any_thread(&mut b, true);
            #[cfg(all(unix, not(target_os = "android"), not(target_vendor = "apple")))]
            winit::platform::x11::EventLoopBuilderExtX11::with_any_thread(&mut b, true);
            Host {
                event_loop: b.build().expect("cannot open the window system"),
                app: App {
                    want: None,
                    window: None,
                    surface: None,
                    input: VecDeque::new(),
                    frame: (vec![0; 640 * 480], 640, 480),
                    cursor: PhysicalPosition::new(0.0, 0.0),
                },
            }
        });
        f(host)
    })
}

/// Open the window (shown at the next pump).
pub fn open(title: &str) {
    if !on_main() {
        return;
    }
    with_host(|h| {
        if h.app.window.is_none() {
            h.app.want = Some(title.to_string());
        }
    });
    pump(Duration::ZERO);
}

/// Handle the window system's events, waiting up to `wait` for one.
pub fn pump(wait: Duration) {
    if !on_main() {
        return;
    }
    with_host(|h| {
        if let Some(f) = PENDING.lock().unwrap().take() {
            h.app.frame = f;
            h.app.draw();
        }
        let _ = h.event_loop.pump_app_events(Some(wait), &mut h.app);
    });
}

/// The next input event, if any (call `pump` first).
pub fn next_input() -> Option<Input> {
    if !on_main() {
        return None;
    }
    with_host(|h| h.app.input.pop_front())
}

/// Show a frame of 0RGB pixels, scaled to the window with its aspect kept.
pub fn present(pixels: &[u32], w: u32, h: u32) {
    shot(pixels, w, h);
    if !on_main() {
        *PENDING.lock().unwrap() = Some((pixels.to_vec(), w, h));
        return;
    }
    with_host(|host| {
        let app = &mut host.app;
        app.frame = (pixels.to_vec(), w, h);
        app.draw();
    });
}

/// FCOP_SHOT=file.png: write the frame shown there, at most every half second (for checks
/// without looking at the window).
fn shot(pixels: &[u32], w: u32, h: u32) {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let Some(path) = PATH.get_or_init(|| std::env::var("FCOP_SHOT").ok()) else { return };
    let mut last = LAST.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
        return;
    }
    *last = Some(std::time::Instant::now());
    let _ = std::fs::write(path, png(pixels, w, h));
}

/// An uncompressed PNG of 0RGB pixels.
fn png(pixels: &[u32], w: u32, h: u32) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        let c = crc(&out[start..]);
        out.extend(c.to_be_bytes());
    }
    let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
    for row in pixels.chunks(w as usize).take(h as usize) {
        raw.push(0);
        for p in row {
            raw.extend([(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
    }
    // zlib stream of stored deflate blocks
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(0xffff).collect();
    for (k, b) in blocks.iter().enumerate() {
        z.push((k + 1 == blocks.len()) as u8);
        z.extend((b.len() as u16).to_le_bytes());
        z.extend((!(b.len() as u16)).to_le_bytes());
        z.extend(*b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend((b << 16 | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 13, 10, 0x1a, 10];
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Where a frame of w x h lands in a window of ww x wh: offset and scale.
fn fit(w: u32, h: u32, ww: u32, wh: u32) -> (f64, f64, f64) {
    let s = (ww as f64 / w as f64).min(wh as f64 / h as f64);
    ((ww as f64 - w as f64 * s) / 2.0, (wh as f64 - h as f64 * s) / 2.0, s)
}

impl App {
    fn draw(&mut self) {
        let (Some(win), Some(surface)) = (&self.window, &mut self.surface) else { return };
        let size = win.inner_size();
        let (Some(ww), Some(wh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(ww, wh).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        let (ref px, w, h) = self.frame;
        let (ww, wh) = (ww.get(), wh.get());
        let (ox, oy, s) = fit(w, h, ww, wh);
        // nearest neighbour; columns once per frame
        let cols: Vec<Option<u32>> = (0..ww)
            .map(|x| {
                let fx = ((x as f64 + 0.5 - ox) / s).floor();
                (fx >= 0.0 && fx < w as f64).then_some(fx as u32)
            })
            .collect();
        for y in 0..wh {
            let fy = ((y as f64 + 0.5 - oy) / s).floor();
            let row = &mut buf[(y * ww) as usize..((y + 1) * ww) as usize];
            if fy < 0.0 || fy >= h as f64 {
                row.fill(0);
                continue;
            }
            let src = &px[(fy as u32 * w) as usize..];
            for (d, c) in row.iter_mut().zip(&cols) {
                *d = c.map_or(0, |c| src[c as usize]);
            }
        }
        let _ = buf.present();
    }

    fn create(&mut self, el: &ActiveEventLoop) {
        let Some(title) = self.want.take() else { return };
        let attrs = Window::default_attributes().with_title(title).with_inner_size(LogicalSize::new(1280.0, 960.0));
        let win = Rc::new(el.create_window(attrs).expect("cannot create the window"));
        let ctx = softbuffer::Context::new(win.clone()).expect("softbuffer context");
        self.surface = Some(softbuffer::Surface::new(&ctx, win.clone()).expect("softbuffer surface"));
        self.window = Some(win);
    }

    /// The cursor in frame pixels.
    fn frame_pos(&self) -> (i32, i32) {
        let Some(win) = &self.window else { return (0, 0) };
        let size = win.inner_size();
        let (_, w, h) = self.frame;
        let (ox, oy, s) = fit(w, h, size.width.max(1), size.height.max(1));
        let x = ((self.cursor.x - ox) / s).clamp(0.0, w as f64 - 1.0);
        let y = ((self.cursor.y - oy) / s).clamp(0.0, h as f64 - 1.0);
        (x as i32, y as i32)
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        self.create(el);
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.create(el);
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, e: WindowEvent) {
        match e {
            WindowEvent::CloseRequested => self.input.push_back(Input::Close),
            WindowEvent::Focused(f) => self.input.push_back(Input::Focus(f)),
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => self.draw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                let (x, y) = self.frame_pos();
                self.input.push_back(Input::Move(x, y));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                self.input.push_back(Input::Button(b, state == ElementState::Pressed));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some((vk, scan)) = vk_of(code) {
                        self.input.push_back(Input::Key { vk, scan, down, repeat: event.repeat });
                    }
                }
                if down {
                    if let Some(t) = &event.text {
                        for ch in t.chars().filter(|&c| (c as u32) < 256) {
                            self.input.push_back(Input::Char(ch as u8));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Windows virtual-key code and set-1 scan code for a key.
fn vk_of(k: KeyCode) -> Option<(u8, u8)> {
    use KeyCode::*;
    let letters = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU,
        KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const LETTER_SCAN: [u8; 26] =
        [0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c];
    if let Some(i) = letters.iter().position(|&l| l == k) {
        return Some((b'A' + i as u8, LETTER_SCAN[i]));
    }
    let digits = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    if let Some(i) = digits.iter().position(|&d| d == k) {
        return Some((b'0' + i as u8, if i == 0 { 0x0b } else { 1 + i as u8 }));
    }
    let pad = [Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9];
    const PAD_SCAN: [u8; 10] = [0x52, 0x4f, 0x50, 0x51, 0x4b, 0x4c, 0x4d, 0x47, 0x48, 0x49];
    if let Some(i) = pad.iter().position(|&d| d == k) {
        return Some((0x60 + i as u8, PAD_SCAN[i]));
    }
    let f = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    if let Some(i) = f.iter().position(|&d| d == k) {
        return Some((0x70 + i as u8, if i < 10 { 0x3b + i as u8 } else { 0x57 + (i as u8 - 10) }));
    }
    Some(match k {
        Escape => (0x1b, 0x01),
        Enter => (0x0d, 0x1c),
        NumpadEnter => (0x0d, 0x1c),
        Space => (0x20, 0x39),
        Tab => (0x09, 0x0f),
        Backspace => (0x08, 0x0e),
        ShiftLeft => (0x10, 0x2a),
        ShiftRight => (0x10, 0x36),
        ControlLeft | ControlRight => (0x11, 0x1d),
        AltLeft | AltRight => (0x12, 0x38),
        ArrowLeft => (0x25, 0x4b),
        ArrowUp => (0x26, 0x48),
        ArrowRight => (0x27, 0x4d),
        ArrowDown => (0x28, 0x50),
        Insert => (0x2d, 0x52),
        Delete => (0x2e, 0x53),
        Home => (0x24, 0x47),
        End => (0x23, 0x4f),
        PageUp => (0x21, 0x49),
        PageDown => (0x22, 0x51),
        NumpadAdd => (0x6b, 0x4e),
        NumpadSubtract => (0x6d, 0x4a),
        NumpadMultiply => (0x6a, 0x37),
        NumpadDivide => (0x6f, 0x35),
        NumpadDecimal => (0x6e, 0x53),
        Minus => (0xbd, 0x0c),
        Equal => (0xbb, 0x0d),
        Comma => (0xbc, 0x33),
        Period => (0xbe, 0x34),
        Slash => (0xbf, 0x35),
        Semicolon => (0xba, 0x27),
        Quote => (0xde, 0x28),
        BracketLeft => (0xdb, 0x1a),
        BracketRight => (0xdd, 0x1b),
        Backslash => (0xdc, 0x2b),
        Backquote => (0xc0, 0x29),
        Pause => (0x13, 0x45),
        _ => return None,
    })
}
