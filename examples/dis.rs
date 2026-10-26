//! Disassembles one function as discovery sees it:  dis <FCopLAPD.exe> <addr>
use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use std::path::Path;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let img = Image::load(Path::new(&args[1])).unwrap();
    let addr = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap();
    let ranges = load_functions(Path::new("data/functions.json")).unwrap();
    let mut code = Code::new(&img);
    let prog = Program::build(&mut code, &ranges);
    let f = &prog.fns[&addr];
    for &a in &f.ins {
        println!("{a:08x}  {}", code.at(a).unwrap());
    }
}
