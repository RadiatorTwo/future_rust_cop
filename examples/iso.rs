//! Lists the files of a CD image: `cargo run --release --example iso -- <file.iso|.bin>`.

use fcop::iso::{Entry, Iso};

fn show(iso: &mut Iso, dir: &Entry, indent: usize) {
    for e in iso.list(dir).expect("cannot read the image") {
        println!("{:indent$}{}{} {}", "", e.name, if e.dir { "/" } else { "" }, e.size);
        if e.dir {
            show(iso, &e, indent + 2);
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: iso <image>");
    let mut iso = Iso::open(path.as_ref()).expect("not a CD image");
    let root = iso.root();
    show(&mut iso, &root, 0);
}
