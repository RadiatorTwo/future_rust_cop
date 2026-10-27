//! Future Cop: L.A.P.D. recompiled: compiles the player's FCopLAPD.exe and runs it.
//!
//!     fcop [path to FCopLAPD.exe]
//!
//! Without a path: the game files in `game_files` next to fcop.exe (see `setup`).
//!
//! A release build is a Windows program without a console window: its messages go to the
//! console it was started from, else to fcop.log next to fcop.exe.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use fcop::jit::compile_cached;
use fcop::setup;
use fcop::smc::Smc;
use fcop::splash;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Without a console window of our own: messages to the console we were started from (or
/// wherever they were redirected), else to fcop.log next to fcop.exe.
#[cfg(windows)]
fn output() {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::System::Console::*;
    unsafe {
        if !GetStdHandle(STD_ERROR_HANDLE).is_null() {
            return;
        }
        let out = if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            std::fs::OpenOptions::new().write(true).open("CONOUT$").ok()
        } else {
            let log = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("fcop.log")));
            log.and_then(|p| std::fs::File::create(p).ok())
        };
        if let Some(f) = out {
            let h = f.into_raw_handle();
            SetStdHandle(STD_OUTPUT_HANDLE, h as _);
            SetStdHandle(STD_ERROR_HANDLE, h as _);
        }
    }
}

#[cfg(not(windows))]
fn output() {}

fn main() {
    #[cfg(unix)]
    if std::env::args().nth(1).as_deref() == Some("--splash") {
        return splash::child();
    }
    output();
    // guest calls nest on the host stack like the original's: run on a thread with a large one
    let code = std::thread::Builder::new().stack_size(256 << 20).spawn(run).unwrap().join().unwrap();
    std::process::exit(code as i32);
}

fn run() -> u32 {
    splash::show();
    splash::status("Looking for the game...");
    let found = match std::env::args().nth(1) {
        Some(p) => setup::check(Path::new(&p)).map(|_| PathBuf::from(p)),
        None => setup::game_exe(&setup::root()),
    };
    let exe = match found {
        Ok(e) => e,
        Err(text) => {
            splash::fail(&text);
            std::process::exit(2);
        }
    };
    let img = Image::load(&exe).unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
    let ranges = load_functions();
    fcop::platform::dinput::start(); // gamepads are found while we compile
    splash::status("Compiling the game...");
    let t = Instant::now();
    let mut code = Code::new(&img);
    let prog = Program::build(&mut code, &ranges);
    let smc = Smc::find(&prog, &mut code);
    let found = t.elapsed().as_millis();
    // the machine code is kept beside the EXE, for this program and this EXE
    let cache = exe.with_file_name(setup::COMPILED);
    let key = cache_key(&img);
    let compiled = compile_cached(&prog, &smc, &mut code, fcop::platform::HOOKS, key.map(|k| (cache.as_path(), k)));
    let st = &compiled.stats;
    if st.cached {
        eprintln!("compiled code of {} functions loaded in {} ms", st.functions, t.elapsed().as_millis());
    } else {
        eprintln!(
            "compiled {} functions in {} ms (finding them {found}, lifting {}, compiling {})",
            st.functions,
            t.elapsed().as_millis(),
            st.lift_ms,
            st.compile_ms
        );
    }
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let code = fcop::platform::run_game(&img, dir);
    drop(compiled);
    code
}

/// What the compiled code depends on: this program (any change to the recompiler) and the
/// game's EXE. None when our own file cannot be read.
fn cache_key(img: &Image) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let own = std::fs::read(std::env::current_exe().ok()?).ok()?;
    let mut h = Sha256::new();
    h.update(&own);
    h.update(&img.data);
    Some(h.finalize().into())
}
