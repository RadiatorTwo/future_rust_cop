//! Savestates (test tool): the whole game as it is now, written to `savestate.bin` in the game
//! folder and put back later, also after a restart, as an emulator does. Cheats `save`
//! (Shift+F7) and `load` (Shift+F8), with FCOP_CHEATS=1 or by name in FCOP_INPUT; one slot.
//!
//! Both run in the mission loop's frame wait (0x406a30, `cheats.rs`), between two frames of
//! the game's logic. Recompiled code returns through the host's stack and loads every register
//! from `Cpu` after a call, so the host's frames hold nothing but where they are: at the same
//! point of the same call chain (the guest's stack pointer and return address the same) the
//! guest's memory and registers can be put back. Kept: the EXE's pages (data, and the code the
//! software renderer rewrites), the main thread's stack and TIB, the heap with its bookkeeping
//! (other threads' stacks are there too), the registers, the guest's clock (it reads the saved
//! time again on loading), events' state and files' positions.
//!
//! What the host holds besides (Glide's textures, sound buffers, kernel objects, threads) is
//! the new run's: it fits the saved memory only when the mission started the same way. So at
//! the first frame of each mission (hook of the mission loop 0x406820) a fingerprint is taken
//! of the heap's blocks, the kernel objects, the sound buffers and the textures' TMU addresses;
//! a savestate loads only in the mission it was made in, with the same fingerprint (else a
//! message says which part differs). Sounds playing are not kept.

use super::{MemoryCopy, kernel32, log, mem};
use crate::rt::{Cpu, ESP, call_original};
use crate::ver::Us;
use std::io::{Read, Write};
use std::sync::Mutex;

/// The mission loop: a new mission (or the same again) starts.
pub const MISSION_LOOP: Us = Us(0x406820);
/// In a network game this computer's view, else negative: never there.
const NET_VIEW: Us = Us(0x511fa0);

const MAGIC: &[u8; 8] = b"FCOPSAV1";
const PARTS: [&str; 4] = ["heap blocks", "kernel objects", "sound buffers", "textures"];

/// The fingerprint of the running mission's start (each part's hash), taken at its first frame.
static START: Mutex<Option<[u64; 4]>> = Mutex::new(None);

/// Hook of the mission loop.
pub fn mission_loop(c: &mut Cpu) {
    *START.lock().unwrap() = None;
    call_original(c, MISSION_LOOP.at());
}

/// Every frame wait: the fingerprint at a mission's first.
pub fn frame() {
    let mut s = START.lock().unwrap();
    if s.is_none() {
        *s = Some(fingerprint());
    }
}

fn hash(words: impl Iterator<Item = u32>) -> u64 {
    words.fold(0xcbf2_9ce4_8422_2325, |h, w| (h ^ w as u64).wrapping_mul(0x100_0000_01b3))
}

fn fingerprint() -> [u64; 4] {
    [
        hash(super::heap_blocks().into_iter().flat_map(|(a, s)| [a, s])),
        hash(kernel32::object_kinds().into_iter().flat_map(|(h, k)| [h, k])),
        hash(super::dsound::buffers().into_iter()),
        hash(super::glide::texture_addresses().into_iter()),
    ]
}

fn network() -> bool {
    (mem().rd32(NET_VIEW.at()) as i32) >= 0
}

fn mission() -> String {
    kernel32::LAST_MISSION.lock().unwrap().clone()
}

fn path() -> std::path::PathBuf {
    super::game_dir().join(crate::setup::SAVESTATE)
}

fn w32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn w64(w: &mut impl Write, v: u64) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn r32(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn r64(r: &mut impl Read) -> std::io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// The registers (not the pointer to guest memory, nor `stop`).
fn write_cpu(w: &mut impl Write, c: &Cpu) -> std::io::Result<()> {
    for v in c.r {
        w32(w, v)?;
    }
    w.write_all(&[c.cf, c.pf, c.zf, c.sf, c.of, c.df])?;
    w32(w, c.fs_base)?;
    w32(w, c.foreign)?;
    for k in 0..8 {
        w64(w, c.st[k].to_bits())?;
        w64(w, c.st_int[k] as u64)?;
        w64(w, c.st_int_of[k])?;
        w64(w, c.mm[k])?;
    }
    w32(w, c.top)?;
    w32(w, c.fsw as u32 | (c.fcw as u32) << 16)
}

fn read_cpu(r: &mut impl Read) -> std::io::Result<Cpu> {
    let mut c = Cpu::new(mem());
    for k in 0..8 {
        c.r[k] = r32(r)?;
    }
    let mut f = [0; 6];
    r.read_exact(&mut f)?;
    [c.cf, c.pf, c.zf, c.sf, c.of, c.df] = f;
    c.fs_base = r32(r)?;
    c.foreign = r32(r)?;
    for k in 0..8 {
        c.st[k] = f64::from_bits(r64(r)?);
        c.st_int[k] = r64(r)? as i64;
        c.st_int_of[k] = r64(r)?;
        c.mm[k] = r64(r)?;
    }
    c.top = r32(r)?;
    let w = r32(r)?;
    (c.fsw, c.fcw) = (w as u16, (w >> 16) as u16);
    Ok(c)
}

pub fn save(c: &mut Cpu) {
    if network() {
        log!("savestate: not in a network game");
        return;
    }
    let Some(start) = *START.lock().unwrap() else {
        log!("savestate: the mission's start was not seen");
        return;
    };
    let t = std::time::Instant::now();
    let memory = MemoryCopy::take();
    let write = || -> std::io::Result<()> {
        let tmp = path().with_extension("tmp");
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        w.write_all(MAGIC)?;
        let m = mission();
        w32(&mut w, m.len() as u32)?;
        w.write_all(m.as_bytes())?;
        for h in start {
            w64(&mut w, h)?;
        }
        w64(&mut w, kernel32::guest_us())?;
        write_cpu(&mut w, c)?;
        let states = kernel32::object_states();
        w32(&mut w, states.len() as u32)?;
        for (h, s) in states {
            w32(&mut w, h)?;
            w64(&mut w, s)?;
        }
        memory.write(&mut w)?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&tmp, path())
    };
    match write() {
        Ok(()) => log!("savestate: saved {} MB to {} in {} ms", memory.bytes() >> 20, path().display(), t.elapsed().as_millis()),
        Err(e) => log!("savestate: cannot write {}: {e}", path().display()),
    }
}

pub fn load(c: &mut Cpu) {
    let t = std::time::Instant::now();
    let mut read = || -> std::io::Result<Result<(), String>> {
        let mut r = std::io::BufReader::new(std::fs::File::open(path())?);
        let mut magic = [0; 8];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Ok(Err("not a savestate of this version".into()));
        }
        let n = r32(&mut r)?;
        if n > 4096 {
            return Ok(Err("not a savestate".into()));
        }
        let mut name = vec![0; n as usize];
        r.read_exact(&mut name)?;
        let name = String::from_utf8_lossy(&name).into_owned();
        if name != mission() {
            return Ok(Err(format!("made in {name}, this is {}", mission())));
        }
        let mut start = [0; 4];
        for h in &mut start {
            *h = r64(&mut r)?;
        }
        let now = START.lock().unwrap().unwrap_or_default();
        let differ: Vec<_> = (0..4).filter(|&k| start[k] != now[k]).map(|k| PARTS[k]).collect();
        if !differ.is_empty() {
            return Ok(Err(format!("this mission started otherwise ({}): start it as when saved", differ.join(", "))));
        }
        let clock = r64(&mut r)?;
        let cpu = read_cpu(&mut r)?;
        let mut states = Vec::new();
        for _ in 0..r32(&mut r)? {
            states.push((r32(&mut r)?, r64(&mut r)?));
        }
        let memory = MemoryCopy::read(&mut r)?;
        let sp = c.r[ESP];
        if cpu.r[ESP] != sp || mem().rd32(sp) != memory.rd32(sp) {
            return Ok(Err("made elsewhere in the game".into()));
        }
        memory.restore();
        let stop = c.stop;
        *c = cpu;
        c.stop = stop;
        kernel32::set_object_states(&states);
        kernel32::set_guest_us(clock);
        super::fps::forget();
        Ok(Ok(()))
    };
    match read() {
        Ok(Ok(())) => log!("savestate: loaded in {} ms", t.elapsed().as_millis()),
        Ok(Err(why)) => log!("savestate: not loaded: {why}"),
        Err(e) => log!("savestate: cannot read {}: {e}", path().display()),
    }
}
