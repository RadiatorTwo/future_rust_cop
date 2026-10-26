//! Future Cop: L.A.P.D. recompiled: compiles the player's FCopLAPD.exe and runs it.
//!
//!     fcop [path to FCopLAPD.exe]     (default: $FCOP_EXE, or the first one under game_files/)

use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use fcop::jit::compile;
use fcop::smc::Smc;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn find_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::args().nth(1).or_else(|| std::env::var("FCOP_EXE").ok()) {
        return Some(PathBuf::from(p));
    }
    fn walk(d: &Path) -> Option<PathBuf> {
        for e in std::fs::read_dir(d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(x) = walk(&p) {
                    return Some(x);
                }
            } else if p.file_name()?.to_str()?.eq_ignore_ascii_case("FCopLAPD.exe") {
                return Some(p);
            }
        }
        None
    }
    walk(Path::new("game_files"))
}

fn main() {
    // guest calls nest on the host stack like the original's: run on a thread with a large one
    let code = std::thread::Builder::new().stack_size(256 << 20).spawn(run).unwrap().join().unwrap();
    std::process::exit(code as i32);
}

fn run() -> u32 {
    let Some(exe) = find_exe() else {
        eprintln!("FCopLAPD.exe not found: pass its path, set FCOP_EXE, or put the game under game_files/");
        std::process::exit(2);
    };
    let img = Image::load(&exe).unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
    let ranges = load_functions(Path::new("data/functions.json")).expect("data/functions.json");
    fcop::image::symbols(); // names for reports, read before the game changes the directory
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
