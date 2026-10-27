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
use fcop::jit::compile;
use fcop::setup;
use fcop::smc::Smc;
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
    output();
    // guest calls nest on the host stack like the original's: run on a thread with a large one
    let code = std::thread::Builder::new().stack_size(256 << 20).spawn(run).unwrap().join().unwrap();
    std::process::exit(code as i32);
}

fn run() -> u32 {
    let found = match std::env::args().nth(1) {
        Some(p) => setup::check(Path::new(&p)).map(|_| PathBuf::from(p)),
        None => setup::game_exe(&setup::root()),
    };
    let exe = match found {
        Ok(e) => e,
        Err(text) => {
            setup::tell(&text);
            std::process::exit(2);
        }
    };
    let img = Image::load(&exe).unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
    let ranges = load_functions();
    fcop::platform::dinput::start(); // gamepads are found while we compile
    let t = Instant::now();
    let mut code = Code::new(&img);
    let prog = Program::build(&mut code, &ranges);
    let smc = Smc::find(&prog, &mut code);
    let compiled = compile(&prog, &smc, &mut code, fcop::platform::HOOKS);
    eprintln!("compiled {} functions in {} ms", compiled.stats.functions, t.elapsed().as_millis());
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let code = fcop::platform::run_game(&img, dir);
    drop(compiled);
    code
}
