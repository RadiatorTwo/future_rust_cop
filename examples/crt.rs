//! Recompiles the whole EXE with Cranelift, prints statistics, and checks
//! the C runtime's string, 64-bit division and float-to-integer functions against Rust on
//! random inputs.
//!
//!     cargo run --release --example crt [path to FCopLAPD.exe]

use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use fcop::jit::{Compiled, compile};
use fcop::rt::{Cpu, EAX, EDX, ESP, Memory};
use fcop::smc::Smc;
use std::path::{Path, PathBuf};
use std::time::Instant;

const STACK_LO: u32 = 0x0010_0000;
const STACK_HI: u32 = 0x0040_0000;
const SCRATCH: u32 = 0x0100_0000;
const RET_MAGIC: u32 = 0x7ffe_0000;

fn find_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::args().nth(1).or_else(|| std::env::var("FCOP_EXE").ok()) {
        return Some(PathBuf::from(p));
    }
    fn walk(d: &Path) -> Option<PathBuf> {
        for e in std::fs::read_dir(d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(x) = walk(&p) {
                    return Some(x);
                }
            } else if p.file_name()?.to_str()?.eq_ignore_ascii_case("FCopLAPD.exe") {
                return Some(p);
            }
        }
        None
    }
    walk(Path::new("game_files"))
}

/// A cdecl/stdcall call into guest code; returns edx:eax.
fn call(c: &Compiled, cpu: &mut Cpu, mem: &Memory, addr: u32, args: &[u32]) -> u64 {
    let mut sp = STACK_HI - 0x1000;
    for &a in args.iter().rev() {
        sp -= 4;
        mem.wr32(sp, a);
    }
    sp -= 4;
    mem.wr32(sp, RET_MAGIC);
    cpu.r[ESP] = sp;
    c.call(cpu, addr);
    (cpu.r[EDX] as u64) << 32 | cpu.r[EAX] as u64
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// short strings over a small alphabet, so searches find something
    fn string(&mut self, max: u64) -> Vec<u8> {
        let n = self.below(max + 1);
        (0..n).map(|_| b"abcdA\xe9"[self.below(6) as usize]).collect()
    }
}

fn put(mem: &Memory, a: u32, s: &[u8]) {
    mem.slice(a, s.len() + 1)[..s.len()].copy_from_slice(s);
    mem.wr8(a + s.len() as u32, 0);
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() {
        return Some(0);
    }
    h.windows(n.len()).position(|w| w == n)
}

fn run_tests(c: &Compiled, mem: &Memory) -> bool {
    let mut cpu = Cpu::new(mem);
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (a, b, d) = (SCRATCH, SCRATCH + 0x1000, SCRATCH + 0x2000);
    let mut all_ok = true;
    let mut check = |name: &str, f: &mut dyn FnMut(&mut Cpu, &mut Rng) -> Result<(), String>| {
        let n = 2000;
        let mut fails = 0;
        let mut first = None;
        for _ in 0..n {
            if let Err(e) = f(&mut cpu, &mut rng) {
                fails += 1;
                first.get_or_insert(e);
            }
        }
        match first {
            None => println!("  {name:<10} ok ({n} random cases)"),
            Some(e) => {
                println!("  {name:<10} FAILED {fails}/{n}: {e}");
                all_ok = false;
            }
        }
    };
    let ptr = |base: u32, off: Option<usize>| off.map_or(0, |o| base + o as u32);

    check("strchr", &mut |cpu, rng| {
        let s = rng.string(40);
        let ch = if rng.below(8) == 0 { 0 } else { b"abcdxA\xe9"[rng.below(7) as usize] };
        put(mem, a, &s);
        let want = if ch == 0 { Some(s.len()) } else { s.iter().position(|&x| x == ch) };
        let got = call(c, cpu, mem, 0x4780d0, &[a, ch as u32]) as u32;
        (got == ptr(a, want)).then_some(()).ok_or(format!("strchr({s:?}, {ch}) = {got:#x}"))
    });
    check("strncpy", &mut |cpu, rng| {
        let s = rng.string(40);
        let n = rng.below(50) as usize;
        put(mem, a, &s);
        mem.slice(d, 64).fill(0xee);
        let got = call(c, cpu, mem, 0x478350, &[d, a, n as u32]) as u32;
        let mut want = vec![0u8; n];
        let k = s.len().min(n);
        want[..k].copy_from_slice(&s[..k]);
        let out = mem.slice(d, n + 1).to_vec();
        (got == d && out[..n] == want[..] && out[n] == 0xee).then_some(()).ok_or(format!("strncpy({s:?}, {n}) -> {out:?}"))
    });
    check("strstr", &mut |cpu, rng| {
        let (h, n) = (rng.string(40), rng.string(3));
        put(mem, a, &h);
        put(mem, b, &n);
        let got = call(c, cpu, mem, 0x4784e0, &[a, b]) as u32;
        (got == ptr(a, find(&h, &n))).then_some(()).ok_or(format!("strstr({h:?}, {n:?}) = {got:#x}"))
    });
    check("strcspn", &mut |cpu, rng| {
        let (s, set) = (rng.string(40), rng.string(3));
        put(mem, a, &s);
        put(mem, b, &set);
        let want = s.iter().position(|x| set.contains(x)).unwrap_or(s.len()) as u32;
        let got = call(c, cpu, mem, 0x4831a0, &[a, b]) as u32;
        (got == want).then_some(()).ok_or(format!("strcspn({s:?}, {set:?}) = {got}, want {want}"))
    });
    check("strncmp", &mut |cpu, rng| {
        let x = rng.string(8);
        let y = if rng.below(3) == 0 { x.clone() } else { rng.string(8) };
        let n = rng.below(12) as usize;
        put(mem, a, &x);
        put(mem, b, &y);
        let (mut xs, mut ys) = (x.clone(), y.clone());
        xs.push(0);
        ys.push(0);
        let mut want = std::cmp::Ordering::Equal;
        for k in 0..n {
            want = xs[k].cmp(&ys[k]);
            if want.is_ne() || xs[k] == 0 {
                break;
            }
        }
        let got = (call(c, cpu, mem, 0x4831e0, &[a, b, n as u32]) as u32 as i32).cmp(&0);
        (got == want).then_some(()).ok_or(format!("strncmp({x:?}, {y:?}, {n}) = {got:?}, want {want:?}"))
    });
    check("strpbrk", &mut |cpu, rng| {
        let (s, set) = (rng.string(40), rng.string(3));
        put(mem, a, &s);
        put(mem, b, &set);
        let want = ptr(a, s.iter().position(|x| set.contains(x)));
        let got = call(c, cpu, mem, 0x483220, &[a, b]) as u32;
        (got == want).then_some(()).ok_or(format!("strpbrk({s:?}, {set:?}) = {got:#x}"))
    });
    let wide = |rng: &mut Rng| match rng.below(3) {
        0 => rng.next() >> rng.below(64),
        1 => rng.next() & 0xffff_ffff,
        _ => rng.next(),
    };
    check("aulldiv", &mut |cpu, rng| {
        let (x, y) = (wide(rng), wide(rng).max(1));
        let got = call(c, cpu, mem, 0x47e960, &[x as u32, (x >> 32) as u32, y as u32, (y >> 32) as u32]);
        (got == x / y).then_some(()).ok_or(format!("{x} / {y} = {got}"))
    });
    check("aullrem", &mut |cpu, rng| {
        let (x, y) = (wide(rng), wide(rng).max(1));
        let got = call(c, cpu, mem, 0x47e9d0, &[x as u32, (x >> 32) as u32, y as u32, (y >> 32) as u32]);
        (got == x % y).then_some(()).ok_or(format!("{x} % {y} = {got}"))
    });
    check("ftol", &mut |cpu, rng| {
        // ST0 -> edx:eax, truncated, through fnstcw/fldcw/fistp qword
        let x = (rng.next() as i64 >> rng.below(64)) as f64 / (1u64 << rng.below(20)) as f64;
        let top = cpu.top;
        cpu.fpush(x);
        let got = call(c, cpu, mem, 0x478090, &[]) as i64;
        let ok = got == x.trunc() as i64 && cpu.top == top && cpu.fcw == 0x37f;
        ok.then_some(()).ok_or(format!("ftol({x}) = {got}, top {}, cw {:#x}", cpu.top, cpu.fcw))
    });
    all_ok
}

fn main() {
    // guest calls nest like the original's: run on a thread with a large stack
    let ok = std::thread::Builder::new().stack_size(256 << 20).spawn(run).unwrap().join().unwrap();
    std::process::exit(if ok { 0 } else { 1 });
}

fn run() -> bool {
    let Some(exe) = find_exe() else {
        eprintln!("FCopLAPD.exe not found: pass its path, set FCOP_EXE, or put the game under game_files/");
        std::process::exit(2);
    };
    let img = Image::load(&exe).unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
    let ranges = load_functions(Path::new("data/functions.json")).expect("data/functions.json");
    let mut code = Code::new(&img);
    let t = Instant::now();
    let prog = Program::build(&mut code, &ranges);
    println!("found {} functions ({} extra entry points) in {} ms", prog.fns.len(), prog.fns.len() - ranges.len(), t.elapsed().as_millis());

    let smc = Smc::find(&prog, &mut code);
    println!(
        "self-modifying code: {} immediates and branch offsets, {} displacements read at run time; {} rewritten instructions",
        smc.imm.len(),
        smc.disp.len(),
        smc.variants.len()
    );
    let compiled = compile(&prog, &smc, &mut code, &[]);
    let s = &compiled.stats;
    println!("compiled {} functions, {} instructions: lifting {} ms, Cranelift {} ms", s.functions, s.instructions, s.lift_ms, s.compile_ms);
    println!("{} functions ({:.1}%) have every instruction supported", s.complete, 100.0 * s.complete as f64 / s.functions as f64);
    let mut missing: Vec<_> = s.missing.iter().collect();
    missing.sort_by(|a, b| b.1.cmp(a.1));
    let top: Vec<String> = missing.iter().take(12).map(|(m, n)| format!("{m} {n}")).collect();
    println!("not supported yet (instructions): {}", top.join(", "));
    if !s.failed.is_empty() {
        println!("{} functions failed to compile, e.g. {:#x}: {}", s.failed.len(), s.failed[0].0, s.failed[0].1);
    }

    let mem = Memory::reserve();
    for sec in &img.sections {
        let size = sec.vsize.max(sec.rsize);
        mem.map(sec.va, sec.va + size);
        let bytes = img.read(sec.va, size as usize).unwrap();
        mem.slice(sec.va, bytes.len()).copy_from_slice(&bytes);
    }
    mem.map(STACK_LO, STACK_HI);
    mem.map(SCRATCH, SCRATCH + 0x10000);
    println!("running C runtime functions:");
    run_tests(&compiled, &mem)
}
