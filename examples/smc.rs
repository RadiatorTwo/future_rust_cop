//! Lists the self-modifying code fields and rewritten instructions found in the EXE.
use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use fcop::smc::Smc;
use std::path::Path;
fn main() {
    let img = Image::load(Path::new(&std::env::args().nth(1).expect("path to FCopLAPD.exe"))).unwrap();
    let ranges = load_functions(Path::new("data/functions.json")).unwrap();
    let mut code = Code::new(&img);
    let prog = Program::build(&mut code, &ranges);
    let smc = Smc::find(&prog, &mut code);
    let show = |what: &str, m: &std::collections::HashMap<u32, fcop::smc::Field>, code: &mut Code| {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by_key(|e| e.0);
        for (a, f) in v {
            println!("{what:<5} {a:#x}  {:<36} field {:#x} ({} bytes)", code.at(*a).unwrap().to_string(), f.at, f.size);
        }
    };
    show("imm", &smc.imm, &mut code);
    show("disp", &smc.disp, &mut code);
    let mut v: Vec<_> = smc.variants.iter().collect();
    v.sort_by_key(|e| e.0);
    for (a, vs) in v {
        let alts: Vec<String> = vs.iter().map(|(_, i)| i.to_string()).collect();
        println!("var   {a:#x}  {:<36} -> {}", code.at(*a).unwrap().to_string(), alts.join(" | "));
    }
    println!("{} immediates/offsets, {} displacements, {} rewritten instructions", smc.imm.len(), smc.disp.len(), smc.variants.len());
}
