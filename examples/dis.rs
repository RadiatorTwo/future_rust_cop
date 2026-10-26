//! Disassembles one function as discovery sees it:  dis <FCopLAPD.exe> <addr>
//! (`all`: every function, each headed by its address). Names from data/symbols.txt are shown
//! in the heading and after the targets of calls and jumps.
use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions, named, symbols};
use std::path::Path;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let img = Image::load(Path::new(&args[1])).unwrap();
    let ranges = load_functions(Path::new("data/functions.json")).unwrap();
    let mut code = Code::new(&img);
    let prog = Program::build(&mut code, &ranges);
    let line = |a: u32, i: &iced_x86::Instruction| {
        let target = (i.near_branch_target() as u32 != 0).then(|| symbols().get(&(i.near_branch_target() as u32))).flatten();
        match target {
            Some(n) => println!("{a:08x}  {i}  ; {n}"),
            None => println!("{a:08x}  {i}"),
        }
    };
    if args[2] == "all" {
        for (addr, f) in &prog.fns {
            println!("== {addr:08x} {}", symbols().get(addr).map_or("", |n| n.as_str()));
            for &a in &f.ins {
                line(a, &code.at(a).unwrap());
            }
        }
        return;
    }
    let addr = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap();
    println!("== {}", named(addr));
    for &a in &prog.fns[&addr].ins {
        line(a, &code.at(a).unwrap());
    }
}
