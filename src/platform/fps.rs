//! The frame rate in missions. The game's clock counts ticks of 1/300 s; its mission loop
//! (0x406a30, network games 0x406b20) waits until a number of ticks has passed since the last
//! frame: 8 in the original (37.5 fps). Those counts are code fields (`smc::FRAME_LIMIT`);
//! writing 300 / fps there gives frames of exactly that many ticks, so the game's time and
//! the frames stay in step. The player chooses in Preferences (REG_DWORD `FrameRate` in the
//! game's key, frames per second; missing or 0: the original).
//!
//! The game moves everything by the frame time, but some things were made for 8-tick frames;
//! below 8 ticks they are corrected here:
//! - player weapons fired once per frame while their reload timer had run out (`weapon`);
//! - the player robot's physics runs in whole 8-tick steps, so it and the camera juddered
//!   (`robot`, `physics`, `camera`);
//! - the camera turns toward its target in 2-tick units (`turn`);
//! - hovering, the physics adds the frame time to the turn speed in every step (`physics`);
//! - effect emitters count their timers down by (rate * ticks) >> 8, which is 0 for short
//!   frames, and emit once a frame, so they lived and emitted far longer (`effects`, `emit`).

use super::{advapi32, mem};
use crate::rt::{Cpu, EAX, ESP, call_original};
use crate::smc::FRAME_LIMIT;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

pub const WEAPON: u32 = 0x458ed0;
pub const ROBOT: u32 = 0x446e00;
pub const PHYSICS: u32 = 0x442120;
pub const CAMERA: u32 = 0x4456a0;
pub const TURN: u32 = 0x445eb0;
pub const CLOCK: u32 = 0x4130c0;
pub const EFFECTS: u32 = 0x43d310;
pub const EMIT: u32 = 0x43d6f0;

/// Where the mission loop's wait (0x406a30) calls the clock: the return address.
const WAITING: u32 = 0x406a56;
/// The ticks at the start of the last frame.
const LAST: u32 = 0x511e4c;

/// The clock's ticks at the start of this frame.
const NOW: u32 = 0x511ee4;
/// The ticks of one frame in the original.
const ORIGINAL: u32 = 8;

/// The frame rates a player can choose (0: the original's 37.5); each a whole number of ticks.
pub const CHOICES: &[u32] = &[0, 60, 75, 100, 150, 300];

/// Ticks per frame in effect.
static TICKS: AtomicU32 = AtomicU32::new(ORIGINAL);

fn ticks_for(fps: u32) -> u32 {
    if CHOICES.contains(&fps) && fps != 0 { 300 / fps } else { ORIGINAL }
}

/// The saved choice (frames per second, 0: the original).
pub fn chosen() -> u32 {
    let fps = advapi32::value("", "FrameRate").and_then(|(_, d)| Some(u32::from_le_bytes(d.get(..4)?.try_into().ok()?))).unwrap_or(0);
    if CHOICES.contains(&fps) { fps } else { 0 }
}

/// Write the ticks per frame into the mission loop.
fn apply(fps: u32) {
    let t = ticks_for(fps);
    TICKS.store(t, Ordering::Relaxed);
    let m = mem();
    for (_, f, n) in FRAME_LIMIT {
        match n {
            1 => m.wr8(f, t as u8),
            _ => m.wr32(f, t),
        }
    }
}

/// At start, after the EXE is loaded.
pub fn init() {
    apply(chosen());
}

/// Preferences' OK: saved, and used from the next frame on.
pub fn set_chosen(fps: u32) {
    advapi32::set_value("", "FrameRate", 4, &fps.to_le_bytes());
    apply(fps);
}

fn faster() -> bool {
    TICKS.load(Ordering::Relaxed) < ORIGINAL
}

fn arg(c: &Cpu, n: u32) -> u32 {
    mem().rd32(c.r[ESP] + 4 + 4 * n)
}

/// The clock's time (doubles): now (set at every read) and at its start.
const TIME: u32 = 0x49fe60;
const START: u32 = 0x49fe50;
/// Its factors from time to ticks (3 and 0.0001).
const SCALE: [u32; 2] = [0x484618, 0x484620];

fn f64_at(a: u32) -> f64 {
    let m = mem();
    f64::from_bits(m.rd32(a) as u64 | (m.rd32(a + 4) as u64) << 32)
}

/// 0x4130c0() reads the clock (ticks). The mission loop calls it in a busy loop until the
/// frame's ticks have passed; there it sleeps instead (with the turn handed over) until
/// shortly before the tick the frame is due, which keeps a core free. Sleeps can last longer
/// than asked, so the last 2 ms are still spun.
pub fn clock(c: &mut Cpu) {
    let m = mem();
    let from = m.rd32(c.r[ESP]);
    call_original(c, CLOCK);
    if from != WAITING {
        return;
    }
    let limit = TICKS.load(Ordering::Relaxed);
    let passed = c.r[EAX].wrapping_sub(m.rd32(LAST)) as i32;
    // the loop's own test; outside 0..limit (a clock that started again) it is left alone
    if !(0..limit as i32).contains(&passed) {
        return;
    }
    let due = m.rd32(LAST).wrapping_add(limit);
    let ticks = (f64_at(TIME) - f64_at(START)) * f64_at(SCALE[0]) * f64_at(SCALE[1]);
    // nominal 300 ticks a second; never longer than a frame
    let left = ((due as f64 - ticks) / 300.0).min(limit as f64 / 300.0) - 0.002;
    if left > 0.0 {
        super::turn_give();
        std::thread::sleep(std::time::Duration::from_secs_f64(left));
        super::turn_take();
    }
}

// ---- weapons ----------------------------------------------------------------------------------

/// Per reload timer: the tick its last shot was due.
static SHOTS: Mutex<Vec<(u32, i32)>> = Mutex::new(Vec::new());

/// 0x458ed0(actor, weapon 0..2) fires a player weapon while its button is held. Each weapon
/// has a reload timer (dword at +0x78 + 4 * weapon in the struct at [actor+0xac]) that a shot
/// raises by the reload time and the frame time lowers. The machine gun's reload time is 0,
/// so it fired once per frame, and the others' were rounded up to whole frames. After a shot
/// the timer is set to the ticks left until the next shot of the original's rate (the reload
/// time rounded up to 8 ticks, counted from when the last shot was due): held fire keeps that
/// rate at any frame rate.
pub fn weapon(c: &mut Cpu) {
    let m = mem();
    let weapon = arg(c, 1);
    let b = m.rd32(arg(c, 0) + 0xac);
    let (timer, ammo) = (b + 0x78 + 4 * weapon, [b + 0x84 + 2 * weapon, b + 0x8a + 2 * weapon]);
    let rounds = || ammo.map(|a| m.rd16(a));
    let before = rounds();
    call_original(c, WEAPON);
    if !faster() || weapon >= 3 || rounds() == before {
        return;
    }
    let now = m.rd32(NOW) as i32;
    let period = ((m.rd32(timer) as i32 + 7) / 8 * 8).max(8);
    let mut shots = SHOTS.lock().unwrap();
    let k = shots.iter().position(|s| s.0 == timer).unwrap_or_else(|| {
        shots.push((timer, now));
        shots.len() - 1
    });
    let due = &mut shots[k].1;
    // held since the last shot: the next one is due a period after it; else from now on
    *due = if now >= *due && now - *due < 2 * period { *due + period } else { now };
    m.wr32(timer, (*due + period - now).max(0) as u32);
}

// ---- the player robot ---------------------------------------------------------------------------

/// The robot's matrix (3x3, 1.0 = 0x10000) at +0x2c and its position at +0x50.
const STATE: u32 = 0x2c;
const STATE_N: usize = 12;

#[derive(Clone, Copy, Default)]
struct Shown {
    actor: u32,
    /// the physics steps of the update running (None: nothing to show)
    steps: Option<u32>,
    before: [i32; STATE_N],
    step: [i32; STATE_N],
    shown: [i32; STATE_N],
    off: [i32; STATE_N],
}

static SHOWN: Mutex<Vec<Shown>> = Mutex::new(Vec::new());

fn shown_for(list: &mut Vec<Shown>, actor: u32) -> &mut Shown {
    let k = list.iter().position(|s| s.actor == actor).unwrap_or_else(|| {
        list.push(Shown { actor, ..Default::default() });
        list.len() - 1
    });
    &mut list[k]
}

/// 0x446e00(actor, dt) updates a player robot. Its physics (0x442120) runs in whole steps of
/// 8 ticks: steps = (dt + the rest at actor+0xb6) / 8, the rest stays in +0xb6. With shorter
/// frames most frames have no step or one, so the robot and the camera following it
/// juddered. Shown instead: the robot between its last two steps, by the ticks since the last
/// one (at most one step behind the physics), set right after the physics so that what the
/// update derives from it afterwards (its parts, the camera) follows. The real values come
/// back at the next update, unless the game moved the robot itself in between.
pub fn robot(c: &mut Cpu) {
    let m = mem();
    let (actor, dt) = (arg(c, 0), arg(c, 1) as i32);
    {
        let mut list = SHOWN.lock().unwrap();
        let s = shown_for(&mut list, actor);
        for i in 0..STATE_N {
            let a = actor + STATE + 4 * i as u32;
            if s.off[i] != 0 && m.rd32(a) as i32 == s.shown[i] {
                m.wr32(a, (s.shown[i] + s.off[i]) as u32);
            }
            s.off[i] = 0;
            s.before[i] = m.rd32(a) as i32;
        }
        s.steps = (faster() && dt > 0).then(|| (dt + (m.rd8(actor + 0xb6) & 7) as i32) as u32 >> 3);
    }
    call_original(c, ROBOT);
}

/// Put the robot where it is shown (once per update).
fn show(actor: u32) {
    let m = mem();
    let mut list = SHOWN.lock().unwrap();
    let s = shown_for(&mut list, actor);
    let Some(steps) = s.steps.take() else { return };
    let left = 8 - (m.rd8(actor + 0xb6) & 7) as i32; // ticks until the next step
    for i in 0..STATE_N {
        let a = actor + STATE + 4 * i as u32;
        let now = m.rd32(a) as i32;
        if steps > 0 {
            s.step[i] = (now - s.before[i]) / steps as i32;
        }
        if s.step[i].abs() > 0x4000 {
            s.step[i] = 0; // a jump, not a move
        }
        s.off[i] = s.step[i] * left / 8;
        s.shown[i] = now - s.off[i];
        m.wr32(a, s.shown[i] as u32);
    }
}

/// 0x442120(actor, dt, steps, ...): the robot's physics, `steps` steps of 8 ticks. Hovering
/// (mode 1 at [actor+0xac]+0xad, 0x442510), every step adds dt to the turn speed: with
/// 8-tick frames that was 8 a step. It gets 8 ticks for each step (the same in sum).
pub fn physics(c: &mut Cpu) {
    let m = mem();
    let actor = arg(c, 0);
    if faster() && m.rd8(m.rd32(actor + 0xac) + 0xad) == 1 {
        m.wr32(c.r[ESP] + 8, 8 * arg(c, 2));
    }
    call_original(c, PHYSICS);
    show(actor);
}

/// The frame's ticks while the effects (0x43d310) are updated, for `emit`.
static EFFECT_DT: AtomicU32 = AtomicU32::new(0);

/// 0x43d310(dt) updates every effect (the list at [0x4a2ab0]: emitters such as the sparks of
/// the robot's transformation, also what weapons and hits show). They are updated every
/// frame; `emit` corrects their emitters.
pub fn effects(c: &mut Cpu) {
    EFFECT_DT.store(if faster() { arg(c, 0) } else { 0 }, Ordering::Relaxed);
    call_original(c, EFFECTS);
    EFFECT_DT.store(0, Ordering::Relaxed);
}

/// Per emitter (effect, part): the fractions (of 256) its three timers did not lose yet, and
/// the ticks since it last emitted.
static EMITTERS: Mutex<Vec<((u32, u32), [u32; 3], u32)>> = Mutex::new(Vec::new());

/// 0x43d6f0(effect, part, a, b, c), called by 0x43d420 for every part of an effect: the
/// part's timers (bytes +0xc, +0xd, +0xe of its 16-byte record at [effect+0x10]) lose a, b
/// and c; while one runs, the part emits (0x43d810, if bit 0x80 of [effect+0xf]) and the
/// effect's count of emissions [effect+0x26] goes down. The amounts are (rate * dt) >> 8
/// with the rates at effect+0x1e/0x20/0x22: rounded down to 0 for short frames, so the timers
/// hardly ran down, and it emitted once a frame. Here the amounts keep their fractions, and
/// a part emits once per 8 ticks, as in the original.
pub fn emit(c: &mut Cpu) {
    let dt = EFFECT_DT.load(Ordering::Relaxed);
    if dt == 0 {
        return call_original(c, EMIT);
    }
    let m = mem();
    let (effect, part) = (arg(c, 0), arg(c, 1));
    let due = {
        let mut list = EMITTERS.lock().unwrap();
        let k = list.iter().position(|e| e.0 == (effect, part)).unwrap_or_else(|| {
            list.push(((effect, part), [0; 3], 0));
            list.len() - 1
        });
        let e = &mut list[k];
        for (n, rate) in [0x1e, 0x20, 0x22].into_iter().enumerate() {
            let total = e.1[n] + m.rd16(effect + rate) as u32 * dt;
            m.wr32(c.r[ESP] + 12 + 4 * n as u32, total >> 8);
            e.1[n] = total & 0xff;
        }
        e.2 += dt;
        let due = e.2 >= 8;
        if due {
            e.2 = (e.2 - 8).min(7);
        }
        due
    };
    let flags = effect + 0xf;
    let emits = m.rd8(flags) & 0x80 != 0;
    if !due && emits {
        m.wr8(flags, m.rd8(flags) & !0x80);
    }
    call_original(c, EMIT);
    if !due && emits {
        m.wr8(flags, m.rd8(flags) | 0x80);
    }
}

/// The frame's ticks while the robot's camera (0x4456a0) runs, for `turn`.
static CAMERA_DT: AtomicU32 = AtomicU32::new(0);

/// 0x4456a0(actor, dt): the camera of the robot (when the physics did not run this update,
/// the robot is put where it is shown here).
pub fn camera(c: &mut Cpu) {
    show(arg(c, 0));
    CAMERA_DT.store(if faster() { arg(c, 1) } else { 0 }, Ordering::Relaxed);
    call_original(c, CAMERA);
    CAMERA_DT.store(0, Ordering::Relaxed);
}

/// Per call site (yaw, pitch): the last angle returned and the fraction it dropped.
static TURNS: Mutex<Vec<(u32, i32, f64)>> = Mutex::new(Vec::new());

/// 0x445eb0(angle, target, n): the camera turns toward the target angle (4096 units a turn)
/// by n times angle = (31 * angle + target) / 32, at least n units. n counts 2-tick units of
/// the frame time (the rest carries over), so at 5 ticks a frame it alternated 2 and 3 and
/// the turn jerked. With shorter frames: the same curve for the frame's exact ticks, keeping
/// the fraction the integer angle drops.
pub fn turn(c: &mut Cpu) {
    let dt = CAMERA_DT.load(Ordering::Relaxed) as i32;
    if dt <= 0 {
        return call_original(c, TURN);
    }
    let m = mem();
    let site = m.rd32(c.r[ESP]);
    let signed = |a: u32| {
        let a = (a & 0xfff) as i32;
        if a >= 0x800 { a - 0x1000 } else { a }
    };
    let (mut cur, to, n) = (signed(arg(c, 0)), signed(arg(c, 1)), arg(c, 2) as i32);
    if to - cur >= 0x800 {
        cur += 0x1000;
    } else if cur - to >= 0x800 {
        cur -= 0x1000;
    }
    let d = to - cur;
    let mut turns = TURNS.lock().unwrap();
    let k = turns.iter().position(|t| t.0 == site).unwrap_or_else(|| {
        turns.push((site, i32::MIN, 0.0));
        turns.len() - 1
    });
    let t = &mut turns[k];
    // our own last result: go on from its exact value
    let from = if t.1 == cur { cur as f64 + t.2 } else { cur as f64 };
    let mut exact = to as f64 - (to as f64 - from) * (31.0f64 / 32.0).powf(dt as f64 / 2.0);
    let mut r = exact.round() as i32;
    if r == cur && d != 0 && n > 0 {
        // the game's minimum step
        r = cur + d.signum() * d.abs().min(n);
        exact = r as f64;
    }
    t.1 = r;
    t.2 = exact - r as f64;
    c.r[EAX] = r as u32;
    c.r[ESP] += 4; // ret
}

/// `FCOP_FPS=1`: log the frames shown per second (called at every frame shown).
pub fn frame_shown() {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static COUNT: Mutex<Option<(std::time::Instant, u32)>> = Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var_os("FCOP_FPS").is_some_and(|v| v != "0")) {
        return;
    }
    let mut c = COUNT.lock().unwrap();
    let (t0, n) = c.get_or_insert_with(|| (std::time::Instant::now(), 0));
    *n += 1;
    if t0.elapsed().as_secs_f32() >= 1.0 {
        eprintln!("fps: {:.1}", *n as f32 / t0.elapsed().as_secs_f32());
        *c = None;
    }
}
