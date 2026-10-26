//! Lists the functions the EXE imports, by DLL.
use fcop::image::Image;
use std::path::Path;
fn main() {
    let img = Image::load(Path::new(&std::env::args().nth(1).expect("path to FCopLAPD.exe"))).unwrap();
    let imports = img.imports();
    let mut dll = String::new();
    for i in &imports {
        if i.dll != dll {
            dll = i.dll.clone();
            println!("{dll}:");
        }
        println!("  {:#x}  {}", i.iat, i.name);
    }
    println!("{} imports", imports.len());
}
