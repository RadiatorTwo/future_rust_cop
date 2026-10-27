//! The frame rate in missions. The game's logic stays as it was made: its clock counts ticks of
//! 1/300 s and the mission loop (0x406820) runs a frame (0x407360) every 8 ticks (37.5 fps).
//! Much of the logic computes (rate * ticks) >> 8 and goes wrong with shorter frames, so more
//! frames are only drawn: while the loop waits for its next frame (it reads the clock at
//! 0x406a56 until 8 ticks have passed), the frame's render block runs again and is shown, with
//! the actors and the camera put where they were at a time between two of the last frames
//! (by their game time, [0x511ee4]). The frame itself is drawn that way too, so every picture
//! shows the game a little more than a frame (8 ticks) late, and moves smoothly.
//!
//! The render block of 0x407360: 0x426eb0, 0x452880(0, player, &view), 0x44fd20 (not when
//! paused), 0x453260(0, player, &view), 0x4075b0(0, 1), 0x434c90(player) (the HUD, unless
//! [0x511c08] says otherwise), 0x4114c0(0) (the display list to Glide); 0x4153a0 shows it.
//! 0x44fd20 is logic, though: it rebuilds the grid of where the actors are (0x4a31b0), which
//! the next frame's hits use, and empties per-frame lists. It runs only in the game's own
//! frame, with the real places. Drawing changes nothing else but the game's random generators
//! (a muzzle flash picks its look; index and table), what the visibility pass 0x452880 leaves
//! for the logic (each view's centre and cell) and the display list, all kept (a network
//! game compares the generators and lost its synchronisation): the list (0x38-byte records
//! from [0x499b0c] up to [0x511dc0]) already holds what the loop added after the frame's
//! render (0x407d70: messages, an indicator at 0x4357f0) for the next frame to draw; drawing
//! empties it (0x40ca10 links the records by depth and resets the end), so every picture
//! puts those records back. Only with Glide, one view (0x407360) and while playing.
//!
//! The network game waits in 0x406b20 instead (the same test in two loops), from a start one
//! frame older, so its frames come in pairs (7 and 1 ticks, or 8 and 0; the other computer
//! waits for the host's frame at 0x406cc6, without sleeping). Things do not move in step with
//! the ticks of a pair's frames, only with the pair's 8 (the small frame hardly moves them):
//! only frames 8 ticks after the last kept are kept, so pictures move evenly. Its frame is
//! 0x4076d0: two players, two views, each computer shows only its own player's ([0x511fa0] &
//! 0xf: the host 0, the other 1); pictures draw that one, with its camera selected. It runs
//! the visibility pass before 0x426eb0, with the real places: its own picture runs it again
//! with the shown ones (else the terrain and walls chosen did not fit the camera and shook).
//!
//! The player chooses the rate in Preferences (REG_DWORD `FrameRate` in the game's key,
//! frames per second; missing or 0: the original; 0xffffffff: unlimited).

use super::{advapi32, mem};
use crate::rt::{Cpu, EAX, ESP, call_original};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const CLOCK: u32 = 0x4130c0;
pub const RENDER_START: u32 = 0x426eb0;
pub const RENDER_END: u32 = 0x4114c0;
pub const UNPAUSED: u32 = 0x44fd20;

/// Where the mission loop's waits call the clock (return addresses): 0x406a30 alone, and
/// the network game's 0x406b20 (two loops). There the frames are not 8 ticks apart: the
/// start it waits from ([0x511e4c]) is the end of the wait before the last one.
const WAITING: [u32; 3] = [0x406a56, 0x4071e4, 0x407282];
/// Where the network game's other computer waits for the host's next frame (it reads the
/// clock between its receives): the frame is due when that comes.
const RECEIVING: u32 = 0x406cc6;
/// The ticks at the start of the last frame.
const LAST: u32 = 0x511e4c;
/// The ticks of a frame.
const FRAME: u32 = 8;
/// The game time of the frame being run (ticks; in a network game the host's clock).
const GAME_TIME: u32 = 0x511ee4;

/// The clock (0x412f80 reads QueryPerformanceCounter into it): seconds per count, and the
/// time (doubles) at its start.
const PER_COUNT: u32 = 0x49fe70;
const START: u32 = 0x49fe50;
/// Its factors from time to ticks (3 and 0.0001).
const SCALE: [u32; 2] = [0x484618, 0x484620];

/// The actors: their number, and a pointer to each.
const ACTOR_COUNT: u32 = 0x50e420;
const ACTORS: u32 = 0x50e428;
/// An actor's 3x3 matrix (1.0 = 0x10000) at +0x2c and its position (20.12) at +0x50.
const PLACE: u32 = 0x2c;
const PLACE_N: usize = 12;
/// The player robot (its handler) has three more models placed in its data ([+0xac]): the
/// side weapons and the lights, records of 0x4c bytes with the same place at +4 (drawn by
/// 0x447ba0, which also takes the lights' glow from the last one).
const ROBOT: u32 = 0x446800;
const ROBOT_MODELS: [u32; 3] = [0x178, 0x1c4, 0x210];
/// The camera: its position (+0), yaw (+0x20, 4096 a turn), matrices and copies up to +0x88.
const CAMERA: u32 = 0x511f48;
const CAMERA_N: usize = 0x88 / 4;
const YAW: usize = 0x20 / 4;
/// What is not interpolated in the camera: a flag and an unused word.
const CAMERA_KEEP: [usize; 2] = [0x1c / 4, 0x24 / 4];

/// The player whose view is drawn, the game's state (4 playing), its views (1 one), nonzero
/// with Glide, and the HUD's condition.
const PLAYER: u32 = 0x511fd0;
const STATE: u32 = 0x511fb8;
const VIEWS: u32 = 0x511f50;
const GLIDE: u32 = 0x511fa4;
const HUD_OWNER: u32 = 0x511c08;
/// The game's two random generators (0x43fe30, 0x43fe70): each an index (0...19) and a table
/// of 20 dwords it adds into. Drawing uses the first (muzzle flashes); in a network game both
/// computers must keep the same.
const RANDOM: [(u32, u32); 2] = [(0x50e370, 0x4a2b08), (0x50e36c, 0x4a2ab8)];
/// What the visibility pass 0x452880 leaves for the logic: each view's centre (x, z) and
/// its cell (bytes x, z). The next frame's logic wakes objects near it (0x42e570) and builds
/// the actor grid around it (0x44fd20): pictures must not move it.
const VIEW_STATE: [(u32, usize); 2] = [(0x4a39b8, 16), (0x4a3b00, 4)];
/// The network game (>= 0: its flags; < 0: none) and split screen (byte, nonzero).
const NET: u32 = 0x511fa0;
const SPLIT: u32 = 0x511f54;
/// The visibility pass: 0x452880(view, player, &view record).
const VISIBILITY: u32 = 0x452880;
/// 0x4076d0 calls 0x426eb0 for view 1 from here (the return address); its view records are
/// at [esp+0x18] (view 0) and [esp+0x3c] (view 1) there.
const VIEW1_START: u32 = 0x407b5a;
const VIEW_RECORD: [u32; 2] = [0x18, 0x3c];
/// Selects the camera by its number: 0x4348e0(robot data [+0xac] byte +0xac).
const CAMERA_SELECT: u32 = 0x4348e0;
/// The display list: the start of its records and their end.
const LIST_START: u32 = 0x499b0c;
const LIST_END: u32 = 0x511dc0;

/// No limit: pictures one after another while the loop waits.
pub const UNLIMITED: u32 = u32::MAX;

/// The frame rates a player can choose (0: the original's 37.5).
pub const CHOICES: &[u32] = &[0, 60, 75, 100, 120, 144, 165, 240, 300, UNLIMITED];

/// The rate in effect (0: the original).
static RATE: AtomicU32 = AtomicU32::new(0);

/// The saved choice (frames per second, 0: the original).
pub fn chosen() -> u32 {
    let fps = advapi32::value("", "FrameRate").and_then(|(_, d)| Some(u32::from_le_bytes(d.get(..4)?.try_into().ok()?))).unwrap_or(0);
    if CHOICES.contains(&fps) { fps } else { 0 }
}

/// At start, after the EXE is loaded.
pub fn init() {
    RATE.store(chosen(), Ordering::Relaxed);
}

/// Preferences' OK: saved, and used from the next frame on.
pub fn set_chosen(fps: u32) {
    advapi32::set_value("", "FrameRate", 4, &fps.to_le_bytes());
    RATE.store(fps, Ordering::Relaxed);
}

fn f64_at(a: u32) -> f64 {
    let m = mem();
    f64::from_bits(m.rd32(a) as u64 | (m.rd32(a + 4) as u64) << 32)
}

/// The clock's time in ticks, with fractions, now (not as of the game's last read: in its
/// own frame that may be a while ago, and the picture showed an earlier time than the one
/// before: things moved back and forth).
fn ticks() -> f64 {
    let time = super::kernel32::start_time().elapsed().as_micros() as f64 * f64_at(PER_COUNT);
    (time - f64_at(START)) * f64_at(SCALE[0]) * f64_at(SCALE[1])
}

/// Drawn frames in between: a higher rate is chosen and the game draws as we know it.
fn active() -> bool {
    let m = mem();
    RATE.load(Ordering::Relaxed) != 0 && m.rd8(GLIDE) != 0 && m.rd32(STATE) == 4 && (m.rd32(VIEWS) == 1 || network())
}

/// The view shown on this computer: in a network game its player's number (the host 0, the
/// other 1; 0x40ca10 draws only that view's display list), else 0.
fn local_view() -> u32 {
    let net = mem().rd32(NET);
    if net as i32 >= 0 { net & 0xf } else { 0 }
}

/// That view's player.
fn local_player() -> u32 {
    mem().rd32(PLAYER + 4 * local_view())
}

/// A network game, one view: two players, each computer draws its own with the two-view frame
/// 0x4076d0, which renders the other player's view too, unseen.
fn network() -> bool {
    let m = mem();
    m.rd32(NET) as i32 >= 0 && m.rd32(VIEWS) == 2 && m.rd8(SPLIT) == 0
}

// ---- what is drawn ----------------------------------------------------------------------------

/// Where the actors and the camera were when a frame was drawn: each place by its address.
struct Places {
    actors: Vec<(u32, [i32; PLACE_N])>,
    camera: [i32; CAMERA_N],
}

fn read(a: u32, out: &mut [i32]) {
    let m = mem();
    for (k, v) in out.iter_mut().enumerate() {
        *v = m.rd32(a + 4 * k as u32) as i32;
    }
}

fn write(a: u32, vals: &[i32]) {
    let m = mem();
    for (k, &v) in vals.iter().enumerate() {
        m.wr32(a + 4 * k as u32, v as u32);
    }
}

impl Places {
    fn put(&self) {
        for (a, p) in &self.actors {
            write(*a, p);
        }
        write(mem().rd32(CAMERA), &self.camera);
    }

    fn now() -> Places {
        let m = mem();
        let place = |a: u32| {
            let mut p = [0; PLACE_N];
            read(a, &mut p);
            (a, p)
        };
        let mut actors = Vec::new();
        for k in 0..m.rd32(ACTOR_COUNT).min(1024) {
            let a = m.rd32(ACTORS + 4 * k);
            actors.push(place(a + PLACE));
            if m.rd32(a + 4) == ROBOT {
                let data = m.rd32(a + 0xac);
                actors.extend(ROBOT_MODELS.iter().map(|&r| place(data + r + 4)));
            }
        }
        let mut camera = [0; CAMERA_N];
        read(m.rd32(CAMERA), &mut camera);
        Places { actors, camera }
    }

    /// Put everything a fraction `f` of the way from `self` to `to`; what moved too far for
    /// one frame (a jump) is put where it is in `to`.
    fn between(&self, to: &Places, f: f64) {
        let lerp = |a: i32, b: i32| a + ((b as f64 - a as f64) * f).round() as i32;
        for (k, (a, p)) in to.actors.iter().enumerate() {
            // the list keeps its order; an actor that was not there is left as it is
            let from = match self.actors.get(k) {
                Some(x) if x.0 == *a => &x.1,
                _ => match self.actors.iter().find(|x| x.0 == *a) {
                    Some(x) => &x.1,
                    None => continue,
                },
            };
            let mut v = *p;
            if (0..9).all(|i| (p[i] - from[i]).abs() <= 0x8000) {
                for i in 0..9 {
                    v[i] = lerp(from[i], p[i]);
                }
            }
            if (9..12).all(|i| (p[i] - from[i]).abs() <= 0x10000) {
                for i in 9..12 {
                    v[i] = lerp(from[i], p[i]);
                }
            }
            write(*a, &v);
        }
        let (a, b) = (&self.camera, &to.camera);
        if (0..3).all(|i| (b[i] - a[i]).abs() <= 0x10000) {
            let mut v = *b;
            for i in 0..CAMERA_N {
                if i == YAW {
                    let d = ((b[i] - a[i] + 0x800) & 0xfff) - 0x800;
                    v[i] = a[i] + (d as f64 * f).round() as i32;
                } else if !CAMERA_KEEP.contains(&i) {
                    v[i] = lerp(a[i], b[i]);
                }
            }
            write(mem().rd32(CAMERA), &v);
        }
    }
}

/// The places the last frames drawn by the game left, with their game time (oldest first).
static HISTORY: Mutex<VecDeque<(f64, Places)>> = Mutex::new(VecDeque::new());
/// How far behind the newest frame pictures are (ticks): a frame, and a margin for frames
/// that come late (else pictures caught up with the newest and stood still).
const LAG: f64 = FRAME as f64 + 2.0;
/// How far the game time is behind the clock when a frame is drawn (ticks, averaged).
static BEHIND: Mutex<Option<f64>> = Mutex::new(None);
/// The real places while a picture is drawn with others (put back at its end).
static REAL: Mutex<Option<Places>> = Mutex::new(None);
/// A picture between the game's frames is being drawn.
static SHOWING: AtomicBool = AtomicBool::new(false);
/// The frame (its start tick) the game drew in its own way, and drew unpaused (+1: none).
static DRAWN: AtomicU32 = AtomicU32::new(0);
static UNPAUSED_DRAWN: AtomicU32 = AtomicU32::new(0);
/// What the visibility pass left for the logic, while the two-view frame draws with its own.
static VIEWS_KEPT: Mutex<Option<Vec<Vec<u8>>>> = Mutex::new(None);
/// When the next picture is due (ticks).
static NEXT: Mutex<f64> = Mutex::new(0.0);

/// The ticks between pictures.
fn period() -> f64 {
    match RATE.load(Ordering::Relaxed) {
        UNLIMITED => 0.0,
        fps => 300.0 / fps.max(1) as f64,
    }
}

/// 0x426eb0 starts the render block. In the game's own frame its places are kept with its
/// game time (when 8 ticks have passed since the last kept). Every picture, the frame's own
/// too, shows the game as it was `LAG` ticks before the newest it drew, as far as the clock
/// has gone on since: between the two frames around that time. So how the game splits its
/// time into frames does not matter (the network game's take 8 and 0 or 7 and 1 ticks in
/// turn: a pair is kept as one). In a network game only this computer's view is done.
pub fn render_start(c: &mut Cpu) {
    let m = mem();
    // the two-view frame starts a render block for each view: only this computer's is shown
    let showing = SHOWING.swap(false, Ordering::Relaxed);
    let view = local_view();
    let second = !showing && network() && (m.rd32(c.r[ESP]) == VIEW1_START) != (view == 1);
    if active() && !second {
        let now = ticks();
        let mut history = HISTORY.lock().unwrap();
        let mut behind = BEHIND.lock().unwrap();
        if !showing {
            DRAWN.store(m.rd32(LAST).wrapping_add(1), Ordering::Relaxed);
            *NEXT.lock().unwrap() = now + period();
            let time = m.rd32(GAME_TIME) as f64;
            // a clock that started again, or a long pause in between: start over
            if history.back().is_some_and(|&(t, _)| time < t || time > t + 4.0 * FRAME as f64) {
                history.clear();
                *behind = None;
            }
            // a frame of at least 8 ticks since the last kept: the network game's pairs of
            // frames (8 + 0, 7 + 1 ticks) do not move things in step with their ticks, a pair as
            // a whole does
            if history.back().is_none_or(|&(t, _)| time >= t + FRAME as f64) {
                history.push_back((time, Places::now()));
                if history.len() > 4 {
                    history.pop_front();
                }
                let b = now - time;
                *behind = Some(behind.map_or(b, |x| x + 0.1 * (b - x)));
            }
        }
        if let Some(b) = *behind {
            let shown = now - b - LAG;
            // the two frames around that time; before the oldest: the oldest
            let k = history.iter().position(|&(t, _)| t > shown).unwrap_or(history.len());
            if k < history.len() {
                *REAL.lock().unwrap() = Some(Places::now());
                if k == 0 {
                    history[0].1.between(&history[0].1, 0.0);
                } else {
                    let ((t0, from), (t1, to)) = (&history[k - 1], &history[k]);
                    from.between(to, ((shown - t0) / (t1 - t0)).clamp(0.0, 1.0));
                }
                // the two-view frame ran the visibility pass before, with the real camera: the
                // terrain and walls it chose did not fit the camera drawn with (they shook)
                if !showing && network() {
                    let views: Vec<Vec<u8>> = VIEW_STATE.iter().map(|&(a, n)| m.slice(a, n).to_vec()).collect();
                    *VIEWS_KEPT.lock().unwrap() = Some(views);
                    let regs = c.r;
                    let record = c.r[ESP] + 4 + VIEW_RECORD[view as usize & 1];
                    c.r[ESP] -= 0x40;
                    super::guest_call(c, VISIBILITY, &[view, local_player(), record]);
                    c.r = regs;
                }
            }
        }
    }
    call_original(c, RENDER_START);
}

/// 0x4114c0 ends the render block: the real places come back.
pub fn render_end(c: &mut Cpu) {
    call_original(c, RENDER_END);
    if let Some(views) = VIEWS_KEPT.lock().unwrap().take() {
        for (&(a, n), v) in VIEW_STATE.iter().zip(&views) {
            mem().slice(a, n).copy_from_slice(v);
        }
    }
    if let Some(real) = REAL.lock().unwrap().take() {
        real.put();
    }
}

/// 0x44fd20 (in the game's own frame, when not paused): with the real places. No pictures
/// in between while paused.
pub fn unpaused(c: &mut Cpu) {
    UNPAUSED_DRAWN.store(mem().rd32(LAST).wrapping_add(1), Ordering::Relaxed);
    let shown = REAL.lock().unwrap().as_ref().map(|real| {
        let shown = Places::now();
        real.put();
        shown
    });
    call_original(c, UNPAUSED);
    if let Some(shown) = shown {
        shown.put();
    }
}

/// Draw and show a picture of now.
fn picture(c: &mut Cpu) {
    let m = mem();
    let regs = c.r;
    let random: Vec<(u32, Vec<u8>)> = RANDOM.iter().map(|&(seed, table)| (m.rd32(seed), m.slice(table, 80).to_vec())).collect();
    let views: Vec<Vec<u8>> = VIEW_STATE.iter().map(|&(a, n)| m.slice(a, n).to_vec()).collect();
    let (start, end) = (m.rd32(LIST_START), m.rd32(LIST_END));
    let waiting = m.slice(start, end.saturating_sub(start) as usize).to_vec();
    let esp = c.r[ESP];
    let view = esp - 0x200;
    c.r[ESP] = esp - 0x400;
    let (v, player) = (local_view(), local_player());
    let (net, camera) = (network(), m.rd32(CAMERA));
    if net {
        // the loop leaves the last player's camera selected
        let number = m.rd8(m.rd32(player + 0xac) + 0xac) as i8 as i32 as u32;
        super::guest_call(c, CAMERA_SELECT, &[number]);
    }
    SHOWING.store(true, Ordering::Relaxed);
    super::guest_call(c, RENDER_START, &[]);
    super::guest_call(c, VISIBILITY, &[v, player, view]);
    super::guest_call(c, 0x453260, &[v, player, view]);
    let hud = m.rd32(HUD_OWNER);
    if net {
        // in 0x4076d0's order, with its indicator (the one-view frame adds it after)
        super::guest_call(c, 0x434c90, &[player]);
        super::guest_call(c, 0x4075b0, &[v, 1]);
        super::guest_call(c, 0x4357f0, &[v]);
    } else {
        super::guest_call(c, 0x4075b0, &[0, 1]);
        if hud == 0 || m.rd8(hud + 0x11f) == 0 {
            super::guest_call(c, 0x434c90, &[player]);
        }
    }
    super::guest_call(c, RENDER_END, &[v]);
    super::guest_call(c, 0x4153a0, &[]);
    for (&(seed, table), (s, t)) in RANDOM.iter().zip(&random) {
        m.wr32(seed, *s);
        m.slice(table, 80).copy_from_slice(t);
    }
    m.slice(start, waiting.len()).copy_from_slice(&waiting);
    m.wr32(LIST_END, end);
    for (&(a, n), v) in VIEW_STATE.iter().zip(&views) {
        m.slice(a, n).copy_from_slice(v);
    }
    m.wr32(CAMERA, camera);
    c.r = regs;
}

/// 0x4130c0() reads the clock (ticks). The mission loop calls it in a busy loop until the
/// frame's 8 ticks have passed. There pictures are drawn when they are due, and in between
/// it sleeps (with the turn handed over) until shortly before the next picture or frame,
/// which keeps a core free. Sleeps can last longer than asked, so the last 2 ms are spun.
pub fn clock(c: &mut Cpu) {
    let m = mem();
    let from = m.rd32(c.r[ESP]);
    call_original(c, CLOCK);
    let receiving = from == RECEIVING;
    if !receiving && !WAITING.contains(&from) {
        return;
    }
    let last = m.rd32(LAST);
    let mut wake = last.wrapping_add(FRAME) as f64;
    if !receiving {
        let passed = c.r[EAX].wrapping_sub(last) as i32;
        // the loop's own test; outside 0..8 (a clock that started again) it is left alone
        if !(0..FRAME as i32).contains(&passed) {
            return;
        }
    }
    let drawn = last.wrapping_add(1);
    if active() && DRAWN.load(Ordering::Relaxed) == drawn && UNPAUSED_DRAWN.load(Ordering::Relaxed) == drawn {
        let now = ticks();
        let next = *NEXT.lock().unwrap();
        let next = if now >= next {
            let eax = c.r[EAX];
            picture(c);
            c.r[EAX] = eax;
            // late pictures are not made up for
            let n = (next + period()).max(now + period() / 2.0);
            *NEXT.lock().unwrap() = n;
            n
        } else {
            next
        };
        wake = wake.min(next);
    }
    // the host's message is not waited for by sleeping
    if receiving {
        return;
    }
    // nominal 300 ticks a second; never longer than a frame
    let left = ((wake - ticks()) / 300.0).min(FRAME as f64 / 300.0) - 0.002;
    if left > 0.0 {
        super::turn_give();
        std::thread::sleep(std::time::Duration::from_secs_f64(left));
        super::turn_take();
    }
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
