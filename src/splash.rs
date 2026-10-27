//! The window shown at start (Windows): what happens until the game's own window opens
//! (looking for the game, copying it from a CD image, compiling), or why the game cannot start.
//! A plain Win32 window on its own thread, so it keeps answering while we work. None with
//! FCOP_BATCH set (tests), and none on other systems yet: there the text goes to stderr only.

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

/// Open the window (does nothing when it is open, or with FCOP_BATCH).
pub fn show() {
    #[cfg(windows)]
    win::show();
}

/// What happens now.
pub fn status(text: &str) {
    eprintln!("{text}");
    #[cfg(windows)]
    win::status(text);
}

/// The game's window is there: close ours.
pub fn close() {
    #[cfg(windows)]
    win::close();
}

/// Why the game cannot start: in the window (until the player closes it), else on stderr.
pub fn fail(text: &str) {
    eprintln!("{text}");
    #[cfg(windows)]
    win::fail(text);
}
