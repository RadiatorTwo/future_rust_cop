//! Builds the address map from the supported EXE to another version of it (the German one is
//! the same code linked again: functions and data moved by a few bytes):
//!   vermap <us FCopLAPD.exe> <other FCopLAPD.exe> > data/map-<name>.txt
//! Both .text sections are decoded from start to end and the instructions compared with the
//! addresses in them masked out; equal runs give the code's moves, the addresses that matched
//! instructions use give the data's. Only addresses are written, nothing of either EXE.
use fcop::image::Image;
use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction};
use std::collections::BTreeMap;
use std::path::Path;

struct Ins {
    ip: u32,
    len: u32,
    key: u64,
    /// absolute addresses in the instruction (displacement, immediate) and a branch target
    addrs: Vec<u32>,
}

fn in_image(v: u32) -> bool {
    (0x400000..0x530000).contains(&v)
}

fn decode(img: &Image) -> Vec<Ins> {
    let t = img.sections.iter().find(|s| s.name == ".text").unwrap();
    let bytes = img.read(t.va, t.vsize as usize).unwrap();
    let mut d = Decoder::with_ip(32, &bytes, t.va as u64, DecoderOptions::NONE);
    let mut out = Vec::new();
    let mut i = Instruction::default();
    while d.can_decode() {
        d.decode_out(&mut i);
        let off = (i.ip() as u32 - t.va) as usize;
        let len = i.len();
        let mut b = bytes[off..off + len].to_vec();
        let c = d.get_constant_offsets(&i);
        let mut addrs = Vec::new();
        let mask = |o: usize, n: usize, b: &mut Vec<u8>| b[o..o + n].iter_mut().for_each(|x| *x = 0);
        if c.has_displacement() && c.displacement_size() == 4 {
            let o = c.displacement_offset();
            let v = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            if in_image(v) {
                addrs.push(v);
                mask(o, 4, &mut b);
            }
        }
        if c.has_immediate() && c.immediate_size() == 4 {
            let o = c.immediate_offset();
            let v = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            let branch = matches!(i.flow_control(), FlowControl::UnconditionalBranch | FlowControl::ConditionalBranch | FlowControl::Call);
            if branch {
                addrs.push(i.near_branch32());
                mask(o, 4, &mut b);
            } else if in_image(v) {
                addrs.push(v);
                mask(o, 4, &mut b);
            }
        } else if c.has_immediate() && i.is_jcc_short_or_near() | i.is_jmp_short() | i.is_loop() | i.is_loopcc() | i.is_jcx_short() {
            addrs.push(i.near_branch32());
            mask(c.immediate_offset(), c.immediate_size(), &mut b);
        }
        // FNV-1a of the masked bytes
        let mut h = 0xcbf29ce484222325u64;
        for x in b {
            h = (h ^ x as u64).wrapping_mul(0x100000001b3);
        }
        out.push(Ins { ip: i.ip() as u32, len: len as u32, key: h, addrs });
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let us = Image::load(Path::new(&a[1])).unwrap();
    let ot = Image::load(Path::new(&a[2])).unwrap();
    let (u, g) = (decode(&us), decode(&ot));
    eprintln!("{} / {} instructions", u.len(), g.len());

    // align: equal runs, resynchronised on 6 equal instructions within a window
    const RUN: usize = 6;
    const WIN: usize = 400;
    let same = |i: usize, j: usize| (0..RUN).all(|k| i + k < u.len() && j + k < g.len() && u[i + k].key == g[j + k].key);
    let mut pairs = Vec::new(); // (us index, other index)
    let (mut i, mut j) = (0, 0);
    let mut skipped = Vec::new();
    let mut subst = Vec::new();
    let show = std::env::var("SHOW").is_ok();
    while i < u.len() && j < g.len() {
        if u[i].key == g[j].key {
            pairs.push((i, j));
            i += 1;
            j += 1;
            continue;
        }
        // smallest total skip that resynchronises
        let mut best = None;
        'o: for s in 1..WIN {
            for di in 0..=s {
                let dj = s - di;
                if same(i + di, j + dj) {
                    best = Some((di, dj));
                    break 'o;
                }
            }
        }
        let Some((di, dj)) = best else {
            eprintln!("lost at {:x} / {:x}", u[i].ip, g[j].ip);
            break;
        };
        if di == dj && (0..di).all(|k| u[i + k].len == g[j + k].len) {
            // the same instructions with other constants: same places
            for k in 0..di {
                subst.push((i + k, j + k));
            }
        } else {
            skipped.push((u[i].ip, di, g[j].ip, dj));
            if show {
                let f = |v: &Vec<Ins>, s: usize, n: usize, img: &Image| {
                    for x in &v[s..s + n] {
                        let b = img.read(x.ip, x.len as usize).unwrap();
                        let mut d = Decoder::with_ip(32, &b, x.ip as u64, DecoderOptions::NONE);
                        eprintln!("    {:x}  {}", x.ip, d.decode());
                    }
                };
                eprintln!("  us:");
                f(&u, i, di, &us);
                eprintln!("  other:");
                f(&g, j, dj, &ot);
            }
        }
        i += di;
        j += dj;
    }

    // code: delta per matched instruction, grouped into runs
    let mut code: BTreeMap<u32, i64> = BTreeMap::new();
    let mut data: BTreeMap<u32, BTreeMap<i64, u32>> = BTreeMap::new();
    let mut bad = 0;
    for &(i, j) in &subst {
        code.insert(u[i].ip, g[j].ip as i64 - u[i].ip as i64);
    }
    for &(i, j) in &pairs {
        let (x, y) = (&u[i], &g[j]);
        code.insert(x.ip, y.ip as i64 - x.ip as i64);
        if x.addrs.len() != y.addrs.len() {
            bad += 1;
            continue;
        }
        for (&p, &q) in x.addrs.iter().zip(&y.addrs) {
            *data.entry(p).or_default().entry(q as i64 - p as i64).or_default() += 1;
        }
    }
    // addresses used by code: each one's most frequent delta (code targets included, as a check)
    let text_end = us.sections.iter().find(|s| s.name == ".text").map(|s| s.va + s.vsize).unwrap();
    let mut conflict = 0;
    let mut refs: BTreeMap<u32, i64> = BTreeMap::new();
    for (&p, ds) in &data {
        let (&d, _) = ds.iter().max_by_key(|e| e.1).unwrap();
        if ds.len() > 1 {
            conflict += 1;
            eprintln!("conflict {p:x}: {ds:?}");
        }
        if p >= text_end {
            refs.insert(p, d);
        } else if let Some(&c) = code.get(&p) {
            if c != d {
                eprintln!("target {p:x}: code {c} ref {d}");
            }
        } else {
            // a target inside a changed stretch, if it is near the matched code around it (a
            // pointer to a function the other version has no more points anywhere)
            let near = code.range(..p).next_back().map(|e| *e.1);
            if near.is_some_and(|n| (n - d).abs() <= 0x100) {
                code.insert(p, d);
            } else {
                eprintln!("dropped target {p:x} {d}");
            }
        }
    }
    // data: a single address with a delta far from both neighbours' is no run
    let list: Vec<(u32, i64)> = refs.iter().map(|(&a, &d)| (a, d)).collect();
    for k in 1..list.len().saturating_sub(1) {
        let (a, d) = list[k];
        if (d - list[k - 1].1).abs() > 0x400 && (d - list[k + 1].1).abs() > 0x400 {
            eprintln!("dropped data {a:x} {d}");
            refs.remove(&a);
        }
    }
    eprintln!("{} matched, {} substituted, {} skips, {bad} mismatched operands, {conflict} conflicts", pairs.len(), subst.len(), skipped.len());
    for (ui, di, gi, dj) in &skipped {
        eprintln!("skip us {ui:x} +{di} / other {gi:x} +{dj}");
    }

    // functions of the US version the other one has not: their start matched nothing
    let mut runs: BTreeMap<u32, Option<i64>> = code.iter().chain(&refs).map(|(&a, &d)| (a, Some(d))).collect();
    #[derive(serde::Deserialize)]
    struct F {
        addr: String,
        size: u32,
    }
    let fs: Vec<F> = serde_json::from_str(include_str!("../data/functions.json")).unwrap();
    for f in fs {
        let a = u32::from_str_radix(f.addr.trim_start_matches("0x"), 16).unwrap();
        if code.contains_key(&a) {
            continue;
        }
        // a start in bytes the sweep took for something else: the same instructions at the
        // place the runs give it
        let d = code.range(..a).next_back().map_or(0, |e| *e.1);
        let ops = |img: &Image, at: u32| {
            let b = img.read(at, 32).unwrap_or_default();
            let mut dec = Decoder::with_ip(32, &b, at as u64, DecoderOptions::NONE);
            (0..3).map(|_| {
                let i = dec.decode();
                (i.mnemonic(), i.len())
            }).collect::<Vec<_>>()
        };
        if ops(&us, a) == ops(&ot, (a as i64 + d) as u32) {
            continue;
        }
        eprintln!("gone: function {a:x} ({} bytes)", f.size);
        let after = code.range(a + f.size..).next().map(|e| *e.1);
        runs.retain(|&x, _| !(a..a + f.size).contains(&x));
        runs.insert(a, None);
        runs.entry(a + f.size).or_insert(after);
    }

    // runs: an address maps with the delta of the last known address at or below it; "-":
    // the other version has nothing there
    println!("# FCopLAPD.exe address map (addresses only): <US address> <delta to this version>,");
    println!("# from the address on until the next line ('-': not in this version). Made by");
    println!("# examples/vermap.rs.");
    let mut last = None;
    for (&a, &d) in &runs {
        if last != Some(d) {
            match d {
                Some(d) => println!("{a:x} {d}"),
                None => println!("{a:x} -"),
            }
            last = Some(d);
        }
    }
}
