//! Debug cheats: by name in FCOP_INPUT ("30000:win") and, with FCOP_CHEATS=1, on Shift+F1...
//! (Ctrl is player 1's special weapon). A cheat only asks; it runs in the mission loop's
//! frame wait (0x406a30, every frame, split screen too), between two frames of the game's
//! logic, and waits there while no mission is played.

use super::{log, mem};
use crate::rt::{call_original, Cpu};
use std::sync::Mutex;

/// The mission loop's wait for the next frame.
pub const FRAME_WAIT: u32 = 0x406a30;

/// 4 while a mission is played (8 paused, 0xb over).
const GAME_STATE: u32 = 0x511fb8;
/// Pointer to the game record: progress +0x40, goal +0x44 (won when equal).
const GAME: u32 = 0x511f80;
/// The players' actors (two pointers); an actor's data at +0xac.
const PLAYERS: u32 = 0x511fd0;
/// An actor's health (16 bits; the robot's data keeps a copy at +0x58 for the HUD, written
/// every frame).
const HEALTH: u32 = 0x1c;
/// In a robot's data: the three weapons' ammunition.
const AMMO: u32 = 0x84;

pub struct Cheat {
    pub name: &'static str,
    /// Shift + this function key (1 = F1)
    pub key: u8,
    pub what: &'static str,
    run: fn(),
}

pub const CHEATS: &[Cheat] = &[
    Cheat { name: "win", key: 1, what: "win the mission (as its script does)", run: win },
    Cheat { name: "lose", key: 2, what: "lose the mission", run: lose },
    Cheat { name: "health", key: 3, what: "health as at the mission's start", run: health },
    Cheat { name: "ammo", key: 4, what: "ammunition as at the mission's start", run: ammo },
];

struct State {
    /// cheats asked for, run at the next frame of a mission
    pending: Vec<usize>,
    /// each player's actor and its health and ammunition when the mission started
    start: [(u32, [u32; 4]); 2],
}

static STATE: Mutex<State> = Mutex::new(State { pending: Vec::new(), start: [(0, [0; 4]); 2] });

pub fn find(name: &str) -> Option<usize> {
    CHEATS.iter().position(|c| c.name == name)
}

/// Shift+F<n> when FCOP_CHEATS is set (the first call, at start, lists them).
pub fn by_key(f: u8) -> Option<usize> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| {
        let on = std::env::var("FCOP_CHEATS").is_ok_and(|v| v != "0");
        if on {
            for c in CHEATS {
                log!("cheat: Shift+F{} {} ({})", c.key, c.name, c.what);
            }
        }
        on
    });
    if on { CHEATS.iter().position(|c| c.key == f) } else { None }
}

pub fn request(k: usize) {
    STATE.lock().unwrap().pending.push(k);
}

/// The names, for an unknown one in FCOP_INPUT.
pub fn names() -> String {
    CHEATS.iter().map(|c| c.name).collect::<Vec<_>>().join(", ")
}

fn player(p: u32) -> u32 {
    mem().rd32(PLAYERS + 4 * p)
}

/// Where value `i` of an actor is: its health, then the ammunition in its data.
fn field(actor: u32, i: usize) -> u32 {
    if i == 0 { actor + HEALTH } else { mem().rd32(actor + 0xac) + AMMO + 4 * (i as u32 - 1) }
}

fn values(actor: u32) -> [u32; 4] {
    let m = mem();
    std::array::from_fn(|i| if i == 0 { m.rd16(field(actor, 0)) as u32 } else { m.rd32(field(actor, i)) })
}

pub fn frame_wait(c: &mut Cpu) {
    if mem().rd32(GAME_STATE) == 4 {
        let run = {
            let mut s = STATE.lock().unwrap();
            for p in 0..2 {
                let actor = player(p);
                if actor != 0 && s.start[p as usize].0 != actor {
                    s.start[p as usize] = (actor, values(actor));
                }
            }
            std::mem::take(&mut s.pending)
        };
        for k in run {
            log!("cheat: {}", CHEATS[k].name);
            (CHEATS[k].run)();
        }
    }
    call_original(c, FRAME_WAIT);
}

fn win() {
    let m = mem();
    let g = m.rd32(GAME);
    m.wr32(g + 0x40, m.rd32(g + 0x44));
    m.wr32(GAME_STATE, 0xb);
}

fn lose() {
    let m = mem();
    let g = m.rd32(GAME);
    m.wr32(g + 0x40, m.rd32(g + 0x44).wrapping_sub(1));
    m.wr32(GAME_STATE, 0xb);
}

fn restore(fields: std::ops::Range<usize>) {
    let start = STATE.lock().unwrap().start;
    for p in 0..2 {
        let actor = player(p);
        if actor != 0 && start[p as usize].0 == actor {
            let before = values(actor);
            for i in fields.clone() {
                let v = start[p as usize].1[i];
                if i == 0 { mem().wr16(field(actor, i), v as u16) } else { mem().wr32(field(actor, i), v) }
            }
            log!("cheat: player {} {:x?} -> {:x?}", p + 1, &before[fields.clone()], &values(actor)[fields.clone()]);
        }
    }
}

fn health() {
    restore(0..1);
}

fn ammo() {
    restore(1..4);
}
