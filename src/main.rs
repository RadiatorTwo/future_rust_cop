//! Future Cop: L.A.P.D. recompiled: compiles the player's FCopLAPD.exe and runs it.
//!
//!     fcop [path to FCopLAPD.exe]
//!
//! Without a path: $FCOP_EXE, else the first FCopLAPD.exe in the current folder's `game_files`,
//! or in or below the folder fcop.exe is in (so it can sit in the game folder or next to it).

use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use fcop::jit::compile;
use fcop::smc::Smc;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The first FCopLAPD.exe in `d` or below it, at most `depth` folders down.
fn walk(d: &Path, depth: u32) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    for e in std::fs::read_dir(d).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            dirs.push(p);
        } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("FCopLAPD.exe")) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    dirs.sort();
    dirs.iter().find_map(|p| walk(p, depth - 1))
}

fn find_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::args().nth(1).or_else(|| std::env::var("FCOP_EXE").ok()) {
        return Some(PathBuf::from(p));
    }
    let here = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    walk(Path::new("game_files"), 4).or_else(|| walk(&here?, 3))
}

fn main() {
    // guest calls nest on the host stack like the original's: run on a thread with a large one
    let code = std::thread::Builder::new().stack_size(256 << 20).spawn(run).unwrap().join().unwrap();
    std::process::exit(code as i32);
}

fn run() -> u32 {
    let Some(exe) = find_exe() else {
        eprintln!(
            "FCopLAPD.exe not found. Put fcop.exe into the game's folder (or next to it), or pass the\n\
             path to FCopLAPD.exe: fcop path\\to\\FCopLAPD.exe\n\nPress Enter to close."
        );
        let _ = std::io::stdin().read_line(&mut String::new());
        std::process::exit(2);
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
