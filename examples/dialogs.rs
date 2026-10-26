//! Lists the EXE's dialog templates and their controls:  dialogs <FCopLAPD.exe>
use fcop::image::{Image, ResId, list_resources};
use fcop::platform::dialog::template::Template;
use std::path::Path;
fn main() {
    let img = Image::load(Path::new(&std::env::args().nth(1).expect("path to FCopLAPD.exe"))).unwrap();
    let rd = |a: u32| img.byte(a);
    for (name, at, size) in list_resources(&rd, img.base, ResId::Id(5)) {
        let bytes = img.read(at, size as usize).unwrap();
        let Some(t) = Template::parse(&bytes) else {
            println!("{name:?}: cannot parse");
            continue;
        };
        println!("{name:?} \"{}\" {:?} style {:#x} font {:?}", t.title, t.rect, t.style, t.font);
        for i in &t.items {
            println!("    {:>5} {:<12} {:?} style {:#010x} {:?}", i.id as i32, format!("{:?}", i.class), i.rect, i.style, i.text);
        }
    }
}
