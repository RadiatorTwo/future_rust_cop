use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use std::path::Path;
fn main() {
    let img = Image::load(Path::new(&std::env::args().nth(1).unwrap())).unwrap();
    let ranges = load_functions(Path::new("data/functions.json")).unwrap();
    let mut code = Code::new(&img);
    let prog = Program::build(&mut code, &ranges);
    let mut seen = std::collections::BTreeSet::new();
    for f in prog.fns.values() {
        for &a in &f.ins {
            let i = code.at(a).unwrap();
            if let Some(why) = fcop::lift::unsupported(&i) {
                if seen.insert(a) { println!("{a:#x}  {:<40} [{why}]  in {}", i.to_string(), f.name); }
            }
        }
    }
}
