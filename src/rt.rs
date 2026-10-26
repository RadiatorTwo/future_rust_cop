//! Run-time support for the recompiled code: the guest CPU state, guest memory, and the
//! helpers generated code calls.

use std::sync::OnceLock;

/// Guest CPU state shared by all recompiled functions. Generated code keeps registers and
/// flags in its own variables and stores them here around calls, returns and jumps out.
#[repr(C)]
pub struct Cpu {
    /// eax ecx edx ebx esp ebp esi edi
    pub r: [u32; 8],
    pub cf: u8,
    pub pf: u8,
    pub zf: u8,
    pub sf: u8,
    pub of: u8,
    pub df: u8,
    pub _pad: [u8; 2],
    /// guest address of the thread information block
    pub fs_base: u32,
    /// host address of guest address 0
    pub mem: *mut u8,
    /// address of the foreign call in progress (see `rt_lookup`)
    pub foreign: u32,
}

pub const EAX: usize = 0;
pub const ECX: usize = 1;
pub const EDX: usize = 2;
pub const EBX: usize = 3;
pub const ESP: usize = 4;
pub const ESI: usize = 6;
pub const EDI: usize = 7;

impl Cpu {
    pub fn new(mem: &Memory) -> Cpu {
        Cpu { r: [0; 8], cf: 0, pf: 0, zf: 0, sf: 0, of: 0, df: 0, _pad: [0; 2], fs_base: 0, mem: mem.base, foreign: 0 }
    }
}

/// The guest's 4 GB address space: guest address `a` is `base + a`, so game pointers stay
/// 32-bit on any host.
pub struct Memory {
    pub base: *mut u8,
}

impl Memory {
    pub fn reserve() -> Memory {
        #[cfg(windows)]
        let base = unsafe {
            use windows_sys::Win32::System::Memory::{MEM_RESERVE, PAGE_NOACCESS, VirtualAlloc};
            VirtualAlloc(std::ptr::null(), 1 << 32, MEM_RESERVE, PAGE_NOACCESS) as *mut u8
        };
        #[cfg(unix)]
        let base = unsafe {
            let p = libc::mmap(
                std::ptr::null_mut(),
                1 << 32,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            );
            if p == libc::MAP_FAILED { std::ptr::null_mut() } else { p as *mut u8 }
        };
        assert!(!base.is_null(), "cannot reserve 4 GB of address space");
        Memory { base }
    }

    /// Make guest addresses [lo, hi) readable and writable.
    pub fn map(&self, lo: u32, hi: u32) {
        let (lo, hi) = (lo & !0xfff, (hi as u64 + 0xfff) & !0xfff);
        let p = unsafe { self.base.add(lo as usize) };
        let n = (hi - lo as u64) as usize;
        #[cfg(windows)]
        let ok = unsafe {
            use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE, VirtualAlloc};
            !VirtualAlloc(p as _, n, MEM_COMMIT, PAGE_READWRITE).is_null()
        };
        #[cfg(unix)]
        let ok = unsafe { libc::mprotect(p as _, n, libc::PROT_READ | libc::PROT_WRITE) == 0 };
        assert!(ok, "cannot map guest memory {lo:#x}..{hi:#x}");
    }

    pub fn slice(&self, a: u32, n: usize) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.base.add(a as usize), n) }
    }
    pub fn rd8(&self, a: u32) -> u8 {
        unsafe { *self.base.add(a as usize) }
    }
    pub fn wr8(&self, a: u32, v: u8) {
        unsafe { *self.base.add(a as usize) = v }
    }
    pub fn rd32(&self, a: u32) -> u32 {
        unsafe { (self.base.add(a as usize) as *const u32).read_unaligned() }
    }
    pub fn wr32(&self, a: u32, v: u32) {
        unsafe { (self.base.add(a as usize) as *mut u32).write_unaligned(v) }
    }
}

/// Recompiled functions by guest address (sorted), and the stub for everything else.
pub struct Dispatch {
    pub table: Vec<(u32, usize)>,
    pub foreign_stub: usize,
}

pub static DISPATCH: OnceLock<Dispatch> = OnceLock::new();

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(101)
}

/// The function to run for a guest address: a recompiled function, or the foreign stub,
/// which passes the address on to `rt_foreign`.
pub extern "C" fn rt_lookup(cpu: *mut Cpu, addr: u32) -> *const u8 {
    let d = DISPATCH.get().expect("dispatch table not set");
    match d.table.binary_search_by_key(&addr, |e| e.0) {
        Ok(k) => d.table[k].1 as *const u8,
        Err(_) => {
            unsafe { (*cpu).foreign = addr };
            d.foreign_stub as *const u8
        }
    }
}

/// A call or jump to an address that is not recompiled code (import thunks come here later).
pub extern "C" fn rt_foreign(cpu: *mut Cpu) {
    let cpu = unsafe { &*cpu };
    die(format!("call to unknown address {:#x} (esp {:#x})", cpu.foreign, cpu.r[ESP]))
}

pub extern "C" fn rt_unimpl(_cpu: *mut Cpu, addr: u32) {
    die(format!("instruction at {addr:#x} is not supported yet"))
}

pub extern "C" fn rt_trap(_cpu: *mut Cpu, addr: u32, kind: u32) {
    let what = match kind {
        0 => "division by zero",
        1 => "int3/hlt/ud2",
        _ => "trap",
    };
    die(format!("{what} at {addr:#x}"))
}

pub extern "C" fn rt_cpuid(cpu: *mut Cpu) {
    let c = unsafe { &mut *cpu };
    // a plain Pentium with FPU, TSC and MMX
    match c.r[EAX] {
        0 => c.r[..4].copy_from_slice(&[1, 0x756e6547, 0x6c65746e, 0x49656e69]),
        _ => c.r[..4].copy_from_slice(&[0x543, 0, 0, 0x0080_8111]),
    }
}

pub extern "C" fn rt_rdtsc(cpu: *mut Cpu) {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    let t = START.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64;
    let c = unsafe { &mut *cpu };
    c.r[EAX] = t as u32;
    c.r[EDX] = (t >> 32) as u32;
}

/// String instructions: `op` = kind | size << 4 | rep << 8 (kind: 0 movs, 1 stos, 2 lods,
/// 3 scas, 4 cmps; rep: 0 none, 1 rep/repe, 2 repne).
pub extern "C" fn rt_string(cpu: *mut Cpu, op: u32) {
    let c = unsafe { &mut *cpu };
    let (kind, size, rep) = (op & 15, (op >> 4) & 15, op >> 8);
    let m = c.mem;
    let rd = |a: u32| -> u32 {
        unsafe {
            let p = m.add(a as usize);
            match size {
                1 => *p as u32,
                2 => (p as *const u16).read_unaligned() as u32,
                _ => (p as *const u32).read_unaligned(),
            }
        }
    };
    let wr = |a: u32, v: u32| unsafe {
        let p = m.add(a as usize);
        match size {
            1 => *p = v as u8,
            2 => (p as *mut u16).write_unaligned(v as u16),
            _ => (p as *mut u32).write_unaligned(v),
        }
    };
    let step = if c.df != 0 { (size as u32).wrapping_neg() } else { size };
    let mask = if size == 4 { u32::MAX } else { (1u32 << (8 * size)) - 1 };
    loop {
        if rep != 0 && c.r[ECX] == 0 {
            break;
        }
        match kind {
            0 => {
                wr(c.r[EDI], rd(c.r[ESI]));
                c.r[ESI] = c.r[ESI].wrapping_add(step);
                c.r[EDI] = c.r[EDI].wrapping_add(step);
            }
            1 => {
                wr(c.r[EDI], c.r[EAX] & mask);
                c.r[EDI] = c.r[EDI].wrapping_add(step);
            }
            2 => {
                c.r[EAX] = (c.r[EAX] & !mask) | rd(c.r[ESI]);
                c.r[ESI] = c.r[ESI].wrapping_add(step);
            }
            _ => {
                let (x, y) = if kind == 3 {
                    let v = (c.r[EAX] & mask, rd(c.r[EDI]));
                    c.r[EDI] = c.r[EDI].wrapping_add(step);
                    v
                } else {
                    let v = (rd(c.r[ESI]), rd(c.r[EDI]));
                    c.r[ESI] = c.r[ESI].wrapping_add(step);
                    c.r[EDI] = c.r[EDI].wrapping_add(step);
                    v
                };
                let r = x.wrapping_sub(y) & mask;
                let bits = 8 * size;
                c.cf = (x < y) as u8;
                c.of = ((((x ^ y) & (x ^ r)) >> (bits - 1)) & 1) as u8;
                c.zf = (r == 0) as u8;
                c.sf = ((r >> (bits - 1)) & 1) as u8;
                c.pf = ((r as u8).count_ones() % 2 == 0) as u8;
            }
        }
        if rep == 0 {
            break;
        }
        c.r[ECX] -= 1;
        if (kind == 3 || kind == 4) && ((rep == 1 && c.zf == 0) || (rep == 2 && c.zf != 0)) {
            break;
        }
    }
}
