//! The window shown at start: what happens until the game's own window opens (looking for the
//! game, copying it from a CD image, compiling), or why the game cannot start; the text goes to
//! stderr too. Windows: a plain Win32 window on its own thread, so it keeps answering while we
//! work. Linux: winit allows one event loop a process, and the game's window needs it, so the
//! window is a second fcop process (`fcop --splash`) that reads the texts from its stdin.
//! None with FCOP_BATCH set (tests).

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::Mutex;
    use std::thread::JoinHandle;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    static WINDOW: AtomicIsize = AtomicIsize::new(0);
    static TEXT: AtomicIsize = AtomicIsize::new(0);
    static BUTTON: AtomicIsize = AtomicIsize::new(0);
    /// we close it (the game starts, or the player read an error): not the player quitting
    static OURS: AtomicBool = AtomicBool::new(false);
    static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

    const CLOSE_ID: usize = 1;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub fn show() {
        let mut t = THREAD.lock().unwrap();
        if t.is_some() || std::env::var_os("FCOP_BATCH").is_some() {
            return;
        }
        let (ready, wait) = std::sync::mpsc::channel();
        *t = Some(std::thread::spawn(move || unsafe { run(ready) }));
        let _ = wait.recv();
    }

    pub fn status(text: &str) {
        let h = TEXT.load(Ordering::Acquire);
        if h != 0 {
            let w = wide(text);
            unsafe { SetWindowTextW(h as HWND, w.as_ptr()) };
        }
    }

    pub fn close() {
        OURS.store(true, Ordering::Release);
        let h = WINDOW.load(Ordering::Acquire);
        if h != 0 {
            unsafe { PostMessageW(h as HWND, WM_CLOSE, 0, 0) };
        }
    }

    /// Show an error with a Close button and wait until the player closes it; false without a
    /// window.
    pub fn fail(text: &str) -> bool {
        show();
        if WINDOW.load(Ordering::Acquire) == 0 {
            return false;
        }
        OURS.store(true, Ordering::Release);
        status(text);
        unsafe { ShowWindow(BUTTON.load(Ordering::Acquire) as HWND, SW_SHOW) };
        if let Some(t) = THREAD.lock().unwrap().take() {
            let _ = t.join();
        }
        true
    }

    unsafe fn run(ready: std::sync::mpsc::Sender<()>) {
        unsafe {
            // as winit does: the game's window must see the display's real pixels
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let px = |v: i32| v * GetDpiForSystem() as i32 / 96;
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide("FcopSplash");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: inst,
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: (COLOR_BTNFACE + 1) as usize as HBRUSH,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let (w, h) = (px(560), px(340));
            let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let title = wide("Future Cop: L.A.P.D.");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                (sw - w) / 2,
                (sh - h) / 2,
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                let _ = ready.send(());
                return;
            }
            let font = |size: i32, weight: i32| {
                let face = wide("Segoe UI");
                CreateFontW(-px(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr())
            };
            let (big, normal) = (font(22, 600), font(15, 400));
            let child = |class: &str, text: &str, style: u32, x: i32, y: i32, w: i32, h: i32, id: usize, f: HFONT| {
                let (c, t) = (wide(class), wide(text));
                let c = CreateWindowExW(
                    0,
                    c.as_ptr(),
                    t.as_ptr(),
                    WS_CHILD | style,
                    px(x),
                    px(y),
                    px(w),
                    px(h),
                    hwnd,
                    id as HMENU,
                    inst,
                    std::ptr::null(),
                );
                SendMessageW(c, WM_SETFONT, f as WPARAM, 1);
                c
            };
            // SS_NOPREFIX: an & in a path is not an underline; SS_EDITCONTROL: long paths wrap
            child("STATIC", "Future Cop: L.A.P.D.", WS_VISIBLE | 0x80, 20, 16, 510, 32, 0, big);
            let text = child("STATIC", "Starting...", WS_VISIBLE | 0x80 | 0x2000, 20, 58, 510, 190, 0, normal);
            let button = child("BUTTON", "Close", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP, 430, 255, 100, 28, CLOSE_ID, normal);
            TEXT.store(text as isize, Ordering::Release);
            BUTTON.store(button as isize, Ordering::Release);
            WINDOW.store(hwnd as isize, Ordering::Release);
            SetForegroundWindow(hwnd);
            let _ = ready.send(());
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            WINDOW.store(0, Ordering::Release);
            TEXT.store(0, Ordering::Release);
            DeleteObject(big as HGDIOBJ);
            DeleteObject(normal as HGDIOBJ);
        }
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_COMMAND if wp & 0xffff == CLOSE_ID => {
                    DestroyWindow(hwnd);
                    0
                }
                WM_CLOSE => {
                    DestroyWindow(hwnd);
                    0
                }
                WM_DESTROY => {
                    if !OURS.load(Ordering::Acquire) {
                        // the player closed it while we were starting
                        std::process::exit(0);
                    }
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }
}

#[cfg(unix)]
mod unix {
    use crate::platform::dialog::draw::{glyph, text_width};
    use crate::platform::gl::Gl;
    use std::io::Write;
    use std::process::{ChildStdin, Command, Stdio};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;
    use winit::application::ApplicationHandler;
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Window, WindowId};

    /// we close it (the game starts, or the player read an error): not the player quitting
    static OURS: AtomicBool = AtomicBool::new(false);
    static PIPE: Mutex<Option<ChildStdin>> = Mutex::new(None);
    static WAITER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

    pub fn show() {
        let mut w = WAITER.lock().unwrap();
        if w.is_some() || std::env::var_os("FCOP_BATCH").is_some() {
            return;
        }
        if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let Ok(mut child) = Command::new(exe).arg("--splash").stdin(Stdio::piped()).stdout(Stdio::null()).spawn() else { return };
        *PIPE.lock().unwrap() = child.stdin.take();
        *w = Some(std::thread::spawn(move || {
            let _ = child.wait();
            if !OURS.load(Ordering::Acquire) {
                // the player closed it while we were starting
                std::process::exit(0);
            }
        }));
    }

    /// One line to the window: its kind (s status, f failure), then the text with its line
    /// breaks as U+001F.
    fn send(kind: char, text: &str) {
        if let Some(p) = PIPE.lock().unwrap().as_mut() {
            let _ = writeln!(p, "{kind}{}", text.replace('\n', "\u{1f}"));
        }
    }

    pub fn status(text: &str) {
        send('s', text);
    }

    pub fn close() {
        OURS.store(true, Ordering::Release);
        // the end of its input closes it
        PIPE.lock().unwrap().take();
    }

    /// Show an error with a Close button and wait until the player closes it.
    pub fn fail(text: &str) {
        show();
        OURS.store(true, Ordering::Release);
        send('f', text);
        PIPE.lock().unwrap().take();
        if let Some(w) = WAITER.lock().unwrap().take() {
            let _ = w.join();
        }
    }

    /// The picture is drawn at half the window's size and shown twice as large (the dialogs'
    /// 8x8 font).
    const W: i32 = 280;
    const H: i32 = 170;
    const BACK: u32 = 0x0a_1440;
    const TITLE: u32 = 0x7f_d0ff;
    const TEXT: u32 = 0xe8_f0ff;
    const EDGE: u32 = 0x3c_64d2;
    const FACE: u32 = 0x0c_1e5a;
    const BUTTON: [i32; 4] = [W - 70, H - 26, W - 10, H - 10];

    enum Msg {
        Status(String),
        Fail(String),
        End,
    }

    struct App {
        text: String,
        failed: bool,
        cursor: (f64, f64),
        window: Option<(Window, Gl)>,
    }

    /// `fcop --splash`: the window, until its input ends or the player closes it.
    pub fn child() {
        let Ok(el) = EventLoop::<Msg>::with_user_event().build() else { return };
        let proxy = el.create_proxy();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                let text = line.get(1..).unwrap_or("").replace('\u{1f}', "\n");
                let _ = proxy.send_event(if line.starts_with('f') { Msg::Fail(text) } else { Msg::Status(text) });
            }
            let _ = proxy.send_event(Msg::End);
        });
        let mut app = App { text: "Starting...".into(), failed: false, cursor: (0.0, 0.0), window: None };
        let _ = el.run_app(&mut app);
    }

    /// Lines of at most `width` pixels, broken at spaces (and inside words longer than that).
    fn wrap(text: &str, width: i32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let joined = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if text_width(&joined) <= width {
                    line = joined;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                for ch in word.chars() {
                    line.push(ch);
                    if text_width(&line) > width {
                        line.pop();
                        lines.push(std::mem::replace(&mut line, ch.to_string()));
                    }
                }
            }
            lines.push(line);
        }
        lines
    }

    fn text(px: &mut [u32], x: i32, y: i32, s: &str, c: u32) {
        let mut x = x;
        for ch in s.chars() {
            let Some((g, l, w)) = glyph(ch) else {
                x += 3;
                continue;
            };
            for (row, bits) in g.iter().enumerate() {
                for col in l..l + w {
                    let (px_x, px_y) = (x + col - l, y + row as i32);
                    if bits >> col & 1 != 0 && (0..W).contains(&px_x) && (0..H).contains(&px_y) {
                        px[(px_y * W + px_x) as usize] = c;
                    }
                }
            }
            x += w + 1;
        }
    }

    impl App {
        fn draw(&mut self) {
            let Some((win, gl)) = &mut self.window else { return };
            let mut px = vec![BACK; (W * H) as usize];
            text(&mut px, 10, 10, "Future Cop: L.A.P.D.", TITLE);
            let bottom = if self.failed { BUTTON[1] - 4 } else { H - 10 };
            for (i, line) in wrap(&self.text, W - 20).iter().enumerate() {
                let y = 30 + 11 * i as i32;
                if y + 8 > bottom {
                    break;
                }
                text(&mut px, 10, y, line, TEXT);
            }
            if self.failed {
                let [x0, y0, x1, y1] = BUTTON;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let edge = x == x0 || x == x1 - 1 || y == y0 || y == y1 - 1;
                        px[(y * W + x) as usize] = if edge { EDGE } else { FACE };
                    }
                }
                text(&mut px, (x0 + x1 - text_width("Close")) / 2, (y0 + y1 - 8) / 2 + 1, "Close", TEXT);
            }
            let size = win.inner_size();
            gl.frame(&px, W as u32, H as u32);
            gl.draw(size.width, size.height, false);
        }

        /// The cursor in picture pixels.
        fn at(&self) -> (i32, i32) {
            let Some((win, _)) = &self.window else { return (-1, -1) };
            let size = win.inner_size();
            let (ox, oy, s) = crate::platform::window::fit(W as u32, H as u32, size.width.max(1), size.height.max(1));
            (((self.cursor.0 - ox) / s) as i32, ((self.cursor.1 - oy) / s) as i32)
        }
    }

    impl ApplicationHandler<Msg> for App {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attrs = Window::default_attributes()
                .with_title("Future Cop: L.A.P.D.")
                .with_inner_size(winit::dpi::LogicalSize::new(2 * W, 2 * H))
                .with_resizable(false);
            self.window = Some(Gl::new(el, attrs));
        }

        fn user_event(&mut self, el: &ActiveEventLoop, m: Msg) {
            match m {
                Msg::Status(t) => self.text = t,
                Msg::Fail(t) => {
                    self.text = t;
                    self.failed = true;
                }
                // with an error it stays until the player closes it
                Msg::End if self.failed => return,
                Msg::End => return el.exit(),
            }
            if let Some((win, _)) = &self.window {
                win.request_redraw();
            }
        }

        fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, e: WindowEvent) {
            match e {
                WindowEvent::CloseRequested => el.exit(),
                WindowEvent::Resized(s) => {
                    if let Some((_, gl)) = &self.window {
                        gl.resize(s.width, s.height);
                    }
                    self.draw();
                }
                WindowEvent::RedrawRequested => self.draw(),
                WindowEvent::CursorMoved { position, .. } => self.cursor = (position.x, position.y),
                WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } if self.failed => {
                    let (x, y) = self.at();
                    let [x0, y0, x1, y1] = BUTTON;
                    if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
                        el.exit();
                    }
                }
                _ => {}
            }
        }
    }
}

/// `fcop --splash`: the start window's own process (Linux).
#[cfg(unix)]
pub use unix::child;

/// Open the window (does nothing when it is open, or with FCOP_BATCH).
pub fn show() {
    #[cfg(windows)]
    win::show();
    #[cfg(unix)]
    unix::show();
}

/// What happens now.
pub fn status(text: &str) {
    eprintln!("{text}");
    #[cfg(windows)]
    win::status(text);
    #[cfg(unix)]
    unix::status(text);
}

/// The game's window is there: close ours.
pub fn close() {
    #[cfg(windows)]
    win::close();
    #[cfg(unix)]
    unix::close();
}

/// Why the game cannot start: in the window (until the player closes it), else on stderr.
pub fn fail(text: &str) {
    eprintln!("{text}");
    #[cfg(windows)]
    win::fail(text);
    #[cfg(unix)]
    unix::fail(text);
}
