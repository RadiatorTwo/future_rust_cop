//! Decodes the instruction at each address:  at <FCopLAPD.exe> <addr>...
use fcop::discover::Code;
use fcop::image::Image;
use std::path::Path;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let img = Image::load(Path::new(&args[1])).unwrap();
    let mut code = Code::new(&img);
    for a in &args[2..] {
        let a = u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap();
        println!("{a:08x}  {}", code.at(a).map_or("?".into(), |i| i.to_string()));
    }
}
