//! Differential test: every function runs from the same random states in Unicorn (the
//! original x86 code) and as recompiled code; registers, flags, the x87 stack and memory
//! must agree afterwards. A call to an import ends a run on both sides (compared as far as
//! it got). Functions whose Unicorn run faults or runs too long are untested.
//!
//!     cargo run --release --features difftest --bin difftest -- <FCopLAPD.exe> [addr...] [-n trials] [-v] [-j workers]
//!
//! The parent starts worker processes (`--worker`), each compiling the whole program once;
//! a worker that crashes or hangs in a function is restarted after it, and the function is
//! reported as failing.

use fcop::discover::{Code, Program};
use fcop::image::{Image, load_functions};
use fcop::jit::{Compiled, compile};
use fcop::rt::{self, Cpu, ESP, Event, Memory, f80_to_f64};
use fcop::smc::Smc;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use unicorn_engine::{Arch, HookType, MemType, Mode, Prot, RegisterX86 as R, Unicorn};

const SENTINEL: u32 = 0xdead_0000;
/// import address table entries point here: a call through one is an import call
const FOREIGN: u32 = 0x7f00_0000;
const SCRATCH: u32 = 0x1000_0000;
const SCRATCH_N: u32 = 0x1_0000;
const STACK: u32 = 0x2000_0000;
const STACK_N: u32 = 0x10_0000;
const ESP0: u32 = STACK + 0x8_0000;
const PAGE: u32 = 0x1000;
/// pages mapped on demand (random pointers; page 0 too: the game's pointers in .data are
/// still null, and reading zeros lets more functions run) per run, at most
const DEMAND_MAX: usize = 256;
const UC_STEPS: usize = 1_000_000;
/// a recompiled run taking longer than this is taken as hung (Unicorn's own runs are
/// limited by UC_STEPS)
const HANG: Duration = Duration::from_secs(10);
const REGS: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
const UREG: [R; 8] = [R::EAX, R::ECX, R::EDX, R::EBX, R::ESP, R::EBP, R::ESI, R::EDI];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exe = args.get(1).unwrap_or_else(|| usage());
    if args.get(2).map(String::as_str) == Some("--worker") {
        let addrs = args[3..].iter().take_while(|a| *a != "-n").map(|a| parse_addr(a)).collect();
        let n = args.iter().position(|a| a == "-n").map_or(6, |k| args[k + 1].parse().unwrap());
        worker(exe, addrs, n);
        return;
    }
    let mut addrs = Vec::new();
    let (mut n, mut verbose, mut jobs) = (6usize, false, std::thread::available_parallelism().map_or(4, |n| n.get()));
    let mut k = 2;
    while k < args.len() {
        match args[k].as_str() {
            "-n" => {
                n = args[k + 1].parse().unwrap();
                k += 1;
            }
            "-j" => {
                jobs = args[k + 1].parse().unwrap();
                k += 1;
            }
            "-v" => verbose = true,
            a => addrs.push(parse_addr(a)),
        }
        k += 1;
    }
    if addrs.is_empty() {
        addrs = World::new(exe).prog.fns.keys().copied().collect();
    }
    parent(exe, addrs, n, verbose, jobs);
}

fn usage() -> ! {
    eprintln!("usage: difftest <FCopLAPD.exe> [addr...] [-n trials] [-v] [-j workers]");
    std::process::exit(2)
}

fn parse_addr(a: &str) -> u32 {
    u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap_or_else(|_| usage())
}

// ---- the parent: workers, hangs, crashes, the summary ------------------------------------------

struct Worker {
    child: Child,
    todo: Vec<u32>,
    /// the function being tested, and since when its recompiled run goes on
    current: Option<(u32, Option<Instant>)>,
    ready: bool,
}

/// Lines from all workers: (worker, line or None at its end).
type Lines = mpsc::Sender<(usize, Option<String>)>;

fn spawn(exe: &str, id: usize, todo: Vec<u32>, n: usize, tx: &Lines) -> Worker {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.arg(exe).arg("--worker").args(todo.iter().map(|a| format!("{a:x}"))).arg("-n").arg(n.to_string());
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("starting a worker");
    let out = BufReader::new(child.stdout.take().unwrap());
    let tx = tx.clone();
    std::thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            let _ = tx.send((id, Some(line)));
        }
        let _ = tx.send((id, None));
    });
    Worker { child, todo, current: None, ready: false }
}

fn parent(exe: &str, addrs: Vec<u32>, n: usize, verbose: bool, jobs: usize) {
    let t0 = Instant::now();
    let (tx, rx) = mpsc::channel();
    let jobs = jobs.min(addrs.len()).max(1);
    let mut workers: Vec<Option<Worker>> =
        (0..jobs).map(|k| Some(spawn(exe, k, addrs.iter().skip(k).step_by(jobs).copied().collect(), n, &tx))).collect();
    // address -> (passed, failed, flags differ only, details)
    let mut results: BTreeMap<u32, (usize, usize, usize, Vec<String>)> = BTreeMap::new();
    while workers.iter().any(Option::is_some) {
        let msg = rx.recv_timeout(Duration::from_millis(500));
        // a worker stuck in one function: kill it, the function fails, go on after it
        for w in workers.iter_mut().flatten() {
            if let Some((a, Some(since))) = w.current
                && since.elapsed() > HANG
            {
                let _ = w.child.kill();
                results.insert(a, (0, 1, 0, vec!["hangs (recompiled side)".into()]));
                w.current = None;
                w.todo.retain(|&x| x != a);
            }
        }
        let Ok((id, line)) = msg else { continue };
        let slot = &mut workers[id];
        let Some(wk) = slot.as_mut() else { continue };
        let Some(l) = line else {
            // the worker ended: finished, crashed in a function or was killed
            let mut wk = slot.take().unwrap();
            let _ = wk.child.wait();
            if let Some((a, _)) = wk.current {
                results.insert(a, (0, 1, 0, vec!["crashes (recompiled side)".into()]));
                wk.todo.retain(|&x| x != a);
            } else if !wk.ready && !wk.todo.is_empty() {
                eprintln!("a worker failed to start");
                std::process::exit(1);
            }
            if !wk.todo.is_empty() {
                *slot = Some(spawn(exe, id, wk.todo, n, &tx));
            }
            continue;
        };
        let (kind, rest) = l.split_once(' ').unwrap_or((&l, ""));
        match kind {
            "ready" => wk.ready = true,
            "S" => wk.current = Some((parse_addr(rest), None)),
            "J" => wk.current = wk.current.map(|(a, _)| (a, Some(Instant::now()))),
            "j" => wk.current = wk.current.map(|(a, _)| (a, None)),
            "R" => {
                // R addr passed failed flags-only details (tab-separated)
                let f: Vec<&str> = rest.splitn(5, ' ').collect();
                let a = parse_addr(f[0]);
                let detail = f.get(4).filter(|d| !d.is_empty()).map(|d| d.split('\t').map(String::from).collect()).unwrap_or_default();
                results.insert(a, (f[1].parse().unwrap(), f[2].parse().unwrap(), f[3].parse().unwrap(), detail));
                wk.todo.retain(|&x| x != a);
                wk.current = None;
            }
            _ => {}
        }
    }
    let (mut pass, mut fail, mut flags, mut untested) = (0, 0, 0, 0);
    let mut failing = Vec::new();
    for (&a, (p, f, fl, detail)) in &results {
        if *fl > 0 && *f == 0 {
            flags += 1;
            if verbose {
                for d in detail {
                    println!("{a:#x} {d}");
                }
            }
        } else if *f > 0 {
            fail += 1;
            failing.push(a);
            if verbose {
                for d in detail {
                    println!("{a:#x} {d}");
                }
            }
        } else if *p > 0 {
            pass += 1;
        } else {
            untested += 1;
            if verbose {
                for d in detail {
                    println!("{a:#x} {d}");
                }
            }
        }
    }
    println!(
        "{} functions: {pass} pass, {fail} fail, {flags} differ only in flags at the end, {untested} untested \
         (every trial faulted or ran too long in Unicorn) in {:.0} s",
        results.len(),
        t0.elapsed().as_secs_f64()
    );
    if !failing.is_empty() {
        let list: Vec<String> = failing.iter().take(100).map(|a| format!("{a:#x}")).collect();
        println!("failing: {}", list.join(" "));
    }
}

// ---- the world both sides start from -------------------------------------------------------------

struct World {
    img: Image,
    prog: Program,
    /// the image as loaded, IAT entries pointing at FOREIGN
    image: Vec<u8>,
    lo: u32,
    hi: u32,
}

impl World {
    fn new(exe: &str) -> World {
        let img = Image::load(Path::new(exe)).unwrap_or_else(|e| panic!("{exe}: {e}"));
        let ranges = load_functions(Path::new("data/functions.json")).expect("data/functions.json");
        let mut code = Code::new(&img);
        let prog = Program::build(&mut code, &ranges);
        let lo = img.base;
        let hi = img.sections.iter().map(|s| s.va + s.vsize.max(s.rsize)).max().unwrap().next_multiple_of(PAGE);
        let mut image = img.read(lo, (hi - lo) as usize).unwrap_or_else(|| vec![0; (hi - lo) as usize]);
        for s in &img.sections {
            let n = s.vsize.max(s.rsize) as usize;
            let bytes = img.read(s.va, n).unwrap();
            image[(s.va - lo) as usize..][..n].copy_from_slice(&bytes);
        }
        for (k, imp) in img.imports().iter().enumerate() {
            image[(imp.iat - lo) as usize..][..4].copy_from_slice(&(FOREIGN + 4 * k as u32).to_le_bytes());
        }
        World { img, prog, image, lo, hi }
    }

    /// Memory regions compared after a run.
    fn compared(&self) -> [(u32, u32); 3] {
        [(self.lo, self.hi - self.lo), (SCRATCH, SCRATCH_N), (ESP0 - 0x8000, 0x8040)]
    }
}

struct Start {
    regs: [u32; 8],
    stack: Vec<u8>,
    scratch: Vec<u8>,
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
}

/// A random start: small numbers, pointers into the scratch area and anything.
fn start(seed: u64) -> Start {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    for _ in 0..4 {
        rng.next();
    }
    let val = |rng: &mut Rng| match rng.below(20) {
        0..8 => rng.below(256) as u32,
        8..15 => (SCRATCH + rng.below(SCRATCH_N as u64 / 2) as u32) & !3,
        _ => rng.next() as u32,
    };
    let mut regs = [0; 8];
    for r in &mut regs {
        *r = val(&mut rng);
    }
    regs[ESP] = ESP0;
    let stack = (0..32).flat_map(|_| val(&mut rng).to_le_bytes()).collect();
    let pattern: Vec<u8> = (0..256).map(|_| rng.next() as u8).collect();
    let scratch = pattern.iter().copied().cycle().take(SCRATCH_N as usize).collect();
    Start { regs, stack, scratch }
}

/// How a run ended, and the state then.
#[derive(Debug, PartialEq, Clone, Copy)]
enum End {
    Ok,
    Import(u32),
    /// at this address
    Fault(u32),
    Timeout,
    Other(Event),
}

struct State {
    regs: [u32; 8],
    flags: u32,
    top: u32,
    fsw: u16,
    st: Vec<f64>,
    mem: Vec<Vec<u8>>,
    pages: Vec<u32>,
}

/// CF PF ZF SF DF OF, as in EFLAGS
const FLAG_MASK: u32 = 0x1 | 0x4 | 0x40 | 0x80 | 0x400 | 0x800;

// ---- Unicorn -----------------------------------------------------------------------------------------

fn run_unicorn(w: &World, addr: u32, s: &Start) -> (End, Option<State>) {
    #[derive(Default)]
    struct Hit {
        import: Option<u32>,
        pages: Vec<u32>,
    }
    let mut uc = Unicorn::new_with_data(Arch::X86, Mode::MODE_32, Hit::default()).unwrap();
    uc.mem_map(w.lo as u64, (w.hi - w.lo) as u64, Prot::ALL).unwrap();
    uc.mem_write(w.lo as u64, &w.image).unwrap();
    uc.mem_map(SCRATCH as u64, SCRATCH_N as u64, Prot::ALL).unwrap();
    uc.mem_write(SCRATCH as u64, &s.scratch).unwrap();
    uc.mem_map(STACK as u64, STACK_N as u64, Prot::ALL).unwrap();
    uc.mem_write(ESP0 as u64, &s.stack).unwrap();
    uc.mem_map(FOREIGN as u64, PAGE as u64, Prot::ALL).unwrap();
    uc.mem_write(FOREIGN as u64, &[0xcc; PAGE as usize]).unwrap();
    let esp = ESP0 - 4;
    uc.mem_write(esp as u64, &SENTINEL.to_le_bytes()).unwrap();
    for (r, v) in UREG.iter().zip(s.regs) {
        uc.reg_write(*r, v as u64).unwrap();
    }
    uc.reg_write(R::ESP, esp as u64).unwrap();
    uc.reg_write(R::FPCW, 0x27f).unwrap();
    uc.add_code_hook(FOREIGN as u64, (FOREIGN + PAGE - 1) as u64, |uc, a, _| {
        uc.get_data_mut().import.get_or_insert(a as u32);
        let _ = uc.emu_stop();
    })
    .unwrap();
    uc.add_mem_hook(HookType::MEM_UNMAPPED, 1, 0, |uc, _t: MemType, a, size, _v| {
        let a = a as u32;
        for p in [a & !(PAGE - 1), a.wrapping_add(size as u32 - 1) & !(PAGE - 1)] {
            if uc.get_data().pages.contains(&p) {
                continue;
            }
            if uc.get_data().pages.len() >= DEMAND_MAX || uc.mem_map(p as u64, PAGE as u64, Prot::ALL).is_err() {
                return false;
            }
            uc.get_data_mut().pages.push(p);
        }
        true
    })
    .unwrap();
    let res = uc.emu_start(addr as u64, SENTINEL as u64, 0, UC_STEPS);
    let end = match uc.get_data().import {
        Some(a) => End::Import(a),
        None if res.is_err() => End::Fault(uc.reg_read(R::EIP).unwrap() as u32),
        None if uc.reg_read(R::EIP).unwrap() as u32 != SENTINEL => End::Timeout,
        None => End::Ok,
    };
    if !matches!(end, End::Ok | End::Import(_)) {
        return (end, None);
    }
    let mut regs = [0; 8];
    for (k, r) in UREG.iter().enumerate() {
        regs[k] = uc.reg_read(*r).unwrap() as u32;
    }
    let fsw = uc.reg_read(R::FPSW).unwrap() as u16;
    let top = (fsw as u32 >> 11) & 7;
    let st = (0..(8 - top) & 7)
        .map(|i| {
            let b = uc.reg_read_long(R::ST0 as i32 + i as i32).unwrap();
            f80_to_f64(b[..10].try_into().unwrap())
        })
        .collect();
    let mut pages = uc.get_data().pages.clone();
    pages.sort_unstable();
    let mem = w.compared().iter().chain(pages.iter().map(|p| (*p, PAGE)).collect::<Vec<_>>().iter()).map(|&(a, n)| uc.mem_read_as_vec(a as u64, n as usize).unwrap()).collect();
    let flags = uc.reg_read(R::EFLAGS).unwrap() as u32 & FLAG_MASK;
    (end, Some(State { regs, flags, top, fsw: fsw & 0x4700, st, mem, pages }))
}

// ---- the recompiled side -------------------------------------------------------------------------

/// Pages committed on demand in guest memory by the exception handler during a run.
mod demand {
    use super::{DEMAND_MAX, PAGE};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_DECOMMIT, PAGE_READWRITE, VirtualAlloc, VirtualFree};

    static BASE: AtomicUsize = AtomicUsize::new(0);
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    pub static PAGES: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;

    unsafe extern "system" fn on_fault(info: *mut EXCEPTION_POINTERS) -> i32 {
        let rec = unsafe { &*(*info).ExceptionRecord };
        if !ACTIVE.load(Ordering::Relaxed) || rec.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS {
            return CONTINUE_SEARCH;
        }
        let (base, a) = (BASE.load(Ordering::Relaxed), rec.ExceptionInformation[1]);
        if a < base || a - base >= 1 << 32 {
            return CONTINUE_SEARCH;
        }
        let p = (a - base) as u32 & !(PAGE - 1);
        let Ok(mut pages) = PAGES.try_lock() else { return CONTINUE_SEARCH };
        if pages.len() >= DEMAND_MAX || pages.contains(&p) {
            return CONTINUE_SEARCH;
        }
        let ok = unsafe { !VirtualAlloc((base + p as usize) as _, PAGE as usize, MEM_COMMIT, PAGE_READWRITE).is_null() };
        if !ok {
            return CONTINUE_SEARCH;
        }
        pages.push(p);
        CONTINUE_EXECUTION
    }

    pub fn install(base: *mut u8) {
        BASE.store(base as usize, Ordering::Relaxed);
        // the page behind 4 GB, reached by an access at 0xffffffff (a CPU wraps around to 0):
        // always there, so such a run differs from Unicorn instead of crashing
        unsafe { VirtualAlloc(base.add(1 << 32) as _, PAGE as usize, MEM_COMMIT, PAGE_READWRITE) };
        unsafe { AddVectoredExceptionHandler(1, Some(on_fault)) };
    }

    pub fn active(on: bool) {
        ACTIVE.store(on, Ordering::Relaxed);
    }

    /// Give back the pages of the last run.
    pub fn reset() {
        let base = BASE.load(Ordering::Relaxed);
        for p in PAGES.lock().unwrap().drain(..) {
            unsafe { VirtualFree((base + p as usize) as _, PAGE as usize, MEM_DECOMMIT) };
        }
    }
}

fn on_event(cpu: &mut Cpu, e: Event) {
    // stop at the first event; foreign calls are compared like Unicorn's import stops
    if cpu.stop == 0 {
        cpu.stop = 1;
        LAST.with(|l| l.set(Some(e)));
    }
}

thread_local! {
    static LAST: std::cell::Cell<Option<Event>> = const { std::cell::Cell::new(None) };
}

fn run_recompiled(w: &World, c: &Compiled, mem: &Memory, addr: u32, s: &Start) -> (End, Option<State>) {
    mem.slice(w.lo, w.image.len()).copy_from_slice(&w.image);
    mem.slice(SCRATCH, SCRATCH_N as usize).copy_from_slice(&s.scratch);
    mem.slice(STACK, STACK_N as usize).fill(0);
    mem.slice(ESP0, s.stack.len()).copy_from_slice(&s.stack);
    let mut cpu = Cpu::new(mem);
    cpu.r = s.regs;
    cpu.r[ESP] = ESP0 - 4;
    mem.wr32(ESP0 - 4, SENTINEL);
    cpu.fcw = 0x27f;
    LAST.with(|l| l.set(None));
    demand::reset();
    demand::active(true);
    c.call(&mut cpu, addr);
    demand::active(false);
    let end = match LAST.with(|l| l.get()) {
        None => End::Ok,
        Some(Event::Foreign(a)) if (FOREIGN..FOREIGN + PAGE).contains(&a) => End::Import(a),
        Some(e) => End::Other(e),
    };
    let mut pages = demand::PAGES.lock().unwrap().clone();
    pages.sort_unstable();
    let st = (0..(8 - cpu.top) & 7).map(|i| cpu.st(i)).collect();
    let flags = (cpu.cf as u32) | (cpu.pf as u32) << 2 | (cpu.zf as u32) << 6 | (cpu.sf as u32) << 7 | (cpu.of as u32) << 11 | (cpu.df as u32) << 10;
    let regions: Vec<(u32, u32)> = w.compared().into_iter().chain(pages.iter().map(|&p| (p, PAGE))).collect();
    let mem = regions.iter().map(|&(a, n)| mem.slice(a, n as usize).to_vec()).collect();
    (end, Some(State { regs: cpu.r, flags, top: cpu.top, fsw: cpu.fsw & 0x4700, st, mem, pages }))
}

fn diff(w: &World, u: &State, r: &State) -> Vec<String> {
    let mut out: Vec<String> = (0..8).filter(|&k| u.regs[k] != r.regs[k]).map(|k| format!("{} {:#x} vs {:#x}", REGS[k], u.regs[k], r.regs[k])).collect();
    if u.flags != r.flags {
        out.push(format!("flags {:#x} vs {:#x}", u.flags, r.flags));
    }
    if u.top != r.top {
        out.push(format!("x87 TOP {} vs {}", u.top, r.top));
    } else {
        for (k, (x, y)) in u.st.iter().zip(&r.st).enumerate() {
            if x.to_bits() != y.to_bits() && !(x.is_nan() && y.is_nan()) {
                out.push(format!("st{k} {x:e} vs {y:e}"));
            }
        }
    }
    if u.fsw != r.fsw {
        out.push(format!("x87 C0-C3 {:#x} vs {:#x}", u.fsw, r.fsw));
    }
    if u.pages != r.pages {
        out.push(format!("pages mapped on demand: {} vs {}", u.pages.len(), r.pages.len()));
        return out;
    }
    let regions: Vec<(u32, u32)> = w.compared().into_iter().chain(u.pages.iter().map(|&p| (p, PAGE))).collect();
    for ((a, _), (x, y)) in regions.iter().zip(u.mem.iter().zip(&r.mem)) {
        if let Some(k) = (0..x.len()).find(|&k| x[k] != y[k]) {
            let e = (k + 8).min(x.len());
            out.push(format!("mem {:#x}: {:02x?} vs {:02x?}", a + k as u32, &x[k..e], &y[k..e]));
        }
    }
    out
}

// ---- a worker --------------------------------------------------------------------------------------

fn worker(exe: &str, addrs: Vec<u32>, n: usize) {
    // guest calls nest on the host stack: everything runs on a thread with a large one
    let exe = exe.to_string();
    std::thread::Builder::new().stack_size(256 << 20).spawn(move || work(&exe, addrs, n)).unwrap().join().unwrap();
}

fn work(exe: &str, addrs: Vec<u32>, n: usize) {
    let w = World::new(exe);
    let mut code = Code::new(&w.img);
    let smc = Smc::find(&w.prog, &mut code);
    let c = compile(&w.prog, &smc, &mut code, &[]);
    rt::set_handler(on_event);
    let mem = Memory::reserve();
    mem.map(w.lo, w.hi);
    mem.map(SCRATCH, SCRATCH + SCRATCH_N);
    mem.map(STACK, STACK + STACK_N);
    demand::install(mem.base);
    let mut o = std::io::stdout().lock();
    writeln!(o, "ready").unwrap();
    o.flush().unwrap();
    for a in addrs {
        writeln!(o, "S {a:x}").unwrap();
        o.flush().unwrap();
        let (mut pass, mut fail, mut flags_only, mut detail) = (0, 0, 0, Vec::new());
        let mut skipped = Vec::new();
        for t in 0..n {
            let s = start(a as u64 * 100 + t as u64);
            let (ue, us) = run_unicorn(&w, a, &s);
            if !matches!(ue, End::Ok | End::Import(_)) {
                skipped.push(ue);
                continue;
            }
            writeln!(o, "J").unwrap();
            o.flush().unwrap();
            let (re, rs) = run_recompiled(&w, &c, &mem, a, &s);
            writeln!(o, "j").unwrap();
            let d = if ue != re { vec![format!("ends {ue:?} vs {re:?}")] } else { diff(&w, us.as_ref().unwrap(), rs.as_ref().unwrap()) };
            if d.is_empty() {
                pass += 1;
            } else {
                // flags left at the end are mostly ones x86 leaves undefined (OF after a
                // shift by more than 1, ZF after imul, ...): counted apart
                if d.iter().all(|x| x.starts_with("flags")) {
                    flags_only += 1;
                } else {
                    fail += 1;
                }
                detail.push(format!("trial {t}: {}", d[..d.len().min(6)].join("; ")));
            }
        }
        if pass + fail + flags_only == 0 {
            // why Unicorn could not run it
            let faults = skipped.iter().filter(|e| matches!(e, End::Fault(_))).count();
            let first = skipped.iter().find_map(|e| if let End::Fault(x) = e { Some(*x) } else { None });
            let at = first.map_or(String::new(), |x| format!(", first at {x:#x}"));
            detail.push(format!("untested: {faults} faults{at}, {} too long", skipped.len() - faults));
        }
        writeln!(o, "R {a:x} {pass} {fail} {flags_only} {}", detail.join("\t")).unwrap();
        o.flush().unwrap();
    }
}
