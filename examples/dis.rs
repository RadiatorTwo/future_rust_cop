//! Disassembles one function as discovery sees it:  dis <FCopLAPD.exe> <addr>
//! (`all`: every function, each headed by its address)
use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use std::path::Path;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let img = Image::load(Path::new(&args[1])).unwrap();
    let ranges = load_functions(Path::new("data/functions.json")).unwrap();
    let mut code = Code::new(&img);
    let prog = Program::build(&mut code, &ranges);
    if args[2] == "all" {
        for (addr, f) in &prog.fns {
            println!("== {addr:08x}");
            for &a in &f.ins {
                println!("{a:08x}  {}", code.at(a).unwrap());
            }
        }
        return;
    }
    let addr = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap();
    for &a in &prog.fns[&addr].ins {
        println!("{a:08x}  {}", code.at(a).unwrap());
    }
}
