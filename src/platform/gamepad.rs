//! The gamepad's own bindings, used besides the game's (issue #8). The game has one binding
//! per action (controls.txt: keyboard or joystick); players want the keyboard and a pad at the
//! same time. So each player has a second set of 14 bindings for their own pad (player 1 pad
//! 1, player 2 pad 2; `dinput::player_pad`), kept in `gamepad.txt` in the game folder, in
//! controls.txt's format (default: our Xbox profile, `dinput::xbox_profile`).
//!
//! binding_down 0x4162a0 (player, &binding) is hooked: an action is down when the game's
//! binding or ours is. It also notes how far each action is pushed (`amounts`): an action held
//! only by stick directions moves the robot in proportion (`dinput::robot_physics`).
//!
//! The "Gamepad" buttons (Preferences, pause bar) open the game's Controller dialog (113) on
//! these bindings: `begin` puts them where the game keeps its own (0x511c20, player 2 +0x80),
//! keys are not captured (`editing`, `user32`), saving controls.txt (0x417bc0) writes
//! gamepad.txt instead, and `end` puts the game's back.

use crate::ver::Us;
use super::{alloc, cstr, dinput, free, game_dir, guest_call, log, mem, put_cstr};
use crate::rt::{Cpu, EAX, ESP, call_original};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const BINDING_DOWN: Us = Us(0x4162a0);
pub const SAVE_CONTROLS: Us = Us(0x417bc0);
/// The Controller dialog and its procedure.
pub const CONTROLLER: u32 = 113;
pub const CONTROLLER_PROC: Us = Us(0x402bb0);
/// The game's bindings: 14 (device, key) pairs per player, 0x80 apart.
const BINDINGS: Us = Us(0x511c20);
const SIZE: u32 = 0x80;
const ACTIONS: u32 = 14;
/// The default profiles the Controller dialog's OK sets (two dwords).
const PROFILE_CHOICES: Us = Us(0x4a01b0);
const FILE: &str = "gamepad.txt";

/// Our bindings in guest memory (both players, `SIZE` apart); 0 until loaded.
static TABLES: Mutex<u32> = Mutex::new(0);
/// Whether the Controller dialog shows our bindings.
static EDITING: AtomicBool = AtomicBool::new(false);
/// The game's bindings and profile choices while it does.
static KEPT: Mutex<Option<(Vec<u8>, [u32; 2])>> = Mutex::new(None);
/// How far each player's actions were pushed at their last read: None when a digital binding
/// holds the action (or nothing does).
static AMOUNTS: Mutex<[[Option<f32>; ACTIONS as usize]; 2]> = Mutex::new([[None; ACTIONS as usize]; 2]);

/// Our bindings, loaded at their first use.
fn tables(c: &mut Cpu) -> u32 {
    let mut t = TABLES.lock().unwrap();
    if *t != 0 {
        return *t;
    }
    let m = mem();
    let a = alloc(2 * SIZE);
    m.slice(a, 2 * SIZE as usize).fill(0);
    let xbox = dinput::xbox_profile();
    for p in 0..2 {
        for i in 0..ACTIONS * 2 {
            m.wr32(a + SIZE * p + 4 * i, m.rd32(xbox + 4 * i));
        }
    }
    *t = a;
    drop(t);
    load(c, a);
    a
}

fn path() -> std::path::PathBuf {
    game_dir().join(FILE)
}

/// Read gamepad.txt into `table` (lines the game cannot read keep the default).
fn load(c: &mut Cpu, table: u32) {
    let Ok(text) = std::fs::read_to_string(path()) else { return };
    let buf = alloc(0x80);
    for (k, line) in text.lines().take(2 * ACTIONS as usize).enumerate() {
        let (p, i) = (k as u32 / ACTIONS, k as u32 % ACTIONS);
        put_cstr(buf, line.trim(), 0x80);
        guest_call(c, dinput::BINDING_PARSE.at(), &[buf, table + SIZE * p + 8 * i]);
    }
    free(buf);
}

/// Write `table` to gamepad.txt as the game writes controls.txt.
fn save(c: &mut Cpu, table: u32) {
    let mut out = String::new();
    for p in 0..2 {
        for i in 0..ACTIONS {
            let name = guest_call(c, dinput::BINDING_NAME.at(), &[table + SIZE * p + 8 * i]);
            out += &cstr(name);
            out += "\r\n";
        }
    }
    if let Err(e) = std::fs::write(path(), out) {
        log!("warn: cannot save {FILE}: {e}");
    }
}

/// The amounts of player p's actions at their last read (`dinput::analog_for`).
pub fn amounts(p: u32) -> [Option<f32>; ACTIONS as usize] {
    AMOUNTS.lock().unwrap()[p.min(1) as usize]
}

/// binding_down hook: the game's binding or ours.
pub fn binding_down(c: &mut Cpu) {
    let m = mem();
    let (player, binding) = (m.rd32(c.r[ESP] + 4), m.rd32(c.r[ESP] + 8));
    let read = |b: u32| (m.rd32(b), m.rd32(b + 4));
    let game = read(binding);
    call_original(c, BINDING_DOWN.at());
    let action = binding.wrapping_sub(BINDINGS.at() + SIZE * player) / 8;
    // read_controls also asks for Escape (not one of the 14)
    if player > 1 || binding < BINDINGS.at() + SIZE * player || action >= ACTIONS || EDITING.load(Ordering::Relaxed) {
        return;
    }
    let ours = read(tables(c) + SIZE * player + 8 * action);
    let game_down = c.r[EAX] & 0xff != 0;
    let game_amount = dinput::binding_amount(dinput::current_pad(player).stick, game);
    let pad = dinput::player_pad(player);
    let our_down = ours.0 == 1 && pad.is_some_and(|p| dinput::key_down(&p, ours.1));
    let our_amount = pad.and_then(|p| dinput::binding_amount(p.stick, ours));
    let digital = game_down && game_amount.is_none() || our_down && our_amount.is_none();
    let amount = [(game_down, game_amount), (our_down, our_amount)]
        .into_iter()
        .filter_map(|(down, a)| if down { a } else { None })
        .reduce(f32::max);
    AMOUNTS.lock().unwrap()[player as usize][action as usize] = if digital { None } else { amount };
    if our_down {
        c.r[EAX] = c.r[EAX] & !0xff | 1;
    }
}

/// Saving controls.txt (the Controller dialog's OK): gamepad.txt while it shows ours.
pub fn save_controls(c: &mut Cpu) {
    if editing() {
        save(c, BINDINGS.at());
        // The original is not called on this path, so emulate its ret.
        c.r[ESP] += 4;
    } else {
        call_original(c, SAVE_CONTROLS.at());
    }
}

/// Whether the Controller dialog shows our bindings.
pub fn editing() -> bool {
    EDITING.load(Ordering::Relaxed)
}

/// Before the Controller dialog opens for the pad: ours where the game keeps its bindings.
pub fn begin(c: &mut Cpu) {
    let t = tables(c);
    let m = mem();
    let kept = m.slice(BINDINGS.at(), 2 * SIZE as usize).to_vec();
    *KEPT.lock().unwrap() = Some((kept, [m.rd32(PROFILE_CHOICES.at()), m.rd32(PROFILE_CHOICES.at() + 4)]));
    let ours = m.slice(t, 2 * SIZE as usize).to_vec();
    m.slice(BINDINGS.at(), ours.len()).copy_from_slice(&ours);
    EDITING.store(true, Ordering::Relaxed);
}

/// After it closed: ours (changed when OK was pressed) back to our table, the game's back.
pub fn end() {
    let Some((kept, choices)) = KEPT.lock().unwrap().take() else { return };
    let m = mem();
    let edited = m.slice(BINDINGS.at(), 2 * SIZE as usize).to_vec();
    m.slice(*TABLES.lock().unwrap(), edited.len()).copy_from_slice(&edited);
    m.slice(BINDINGS.at(), kept.len()).copy_from_slice(&kept);
    m.wr32(PROFILE_CHOICES.at(), choices[0]);
    m.wr32(PROFILE_CHOICES.at() + 4, choices[1]);
    EDITING.store(false, Ordering::Relaxed);
}
