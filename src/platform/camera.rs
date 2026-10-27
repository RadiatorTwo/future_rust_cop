//! More cameras for the player's robot, after the game's own seven on the camera key: close
//! behind it (the robot in view). First person (in its head, looking where it walks; the
//! robot is not drawn) is only reached with FCOP_CAMERA=first until its crashes are fixed.
//!
//! The game's robot_camera (0x4456a0) places the camera every frame: the camera key counts the
//! robot data's mode (+0xa4, 0...6), a table gives each mode's angles and distance, and it
//! sets the camera's angles with 0x434760 (yaw, pitch, roll; matrices from them) and its
//! position with 0x4346d0 (&position; the view translation from it), both on the camera in
//! [0x511f48]. In our modes it runs as usual (in its mode 0), then we set our angles and
//! position the same way. Positions are 20.12 with y up; a robot at yaw w (actor +0xb0, 4096 a
//! turn) faces (sin w, 0, cos w); a camera's yaw looks the same way, pitch > 0 looks down.

use super::{guest_call, mem};
use crate::rt::{call_original, Cpu, ESP};
use std::sync::Mutex;

/// robot_camera(actor, dt, dt / 2): places the player's camera every frame.
pub const ROBOT_CAMERA: u32 = 0x4456a0;
/// robot_draw(upper part): the robot's body, upper part, side weapons, lights, muzzle flash.
pub const ROBOT_DRAW: u32 = 0x447ba0;

/// The camera in use, a player's camera by its number (robot data +0xac), and setting the
/// current camera's angles and position.
const CAMERA: u32 = 0x511f48;
const CAMERA_OF: u32 = 0x434900;
const SET_ANGLES: u32 = 0x434760;
const SET_POSITION: u32 = 0x4346d0;
/// The players' actors.
const PLAYERS: u32 = 0x511fd0;

/// In the robot's data: the game's camera mode (0...6), and a timer the camera key sets to
/// 900 (it runs down by the frame's ticks): a press is that timer going up.
const MODE: u32 = 0xa4;
const KEY_TIMER: u32 = 0xae;

/// One unit in positions.
const UNIT: f64 = 4096.0;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Game,
    FirstPerson,
    Behind,
}

/// Each player's camera and when its key was last pressed, and a guest buffer for a position.
struct State {
    mode: [Mode; 2],
    last: [Option<std::time::Instant>; 2],
    buf: u32,
}

static STATE: Mutex<State> = Mutex::new(State { mode: [Mode::Game; 2], last: [None; 2], buf: 0 });

fn player_of(actor: u32) -> Option<usize> {
    (0..2).find(|&p| mem().rd32(PLAYERS + 4 * p as u32) == actor)
}

pub fn robot_camera(c: &mut Cpu) {
    let m = mem();
    let actor = m.rd32(c.r[ESP] + 4);
    let data = m.rd32(actor + 0xac);
    let Some(p) = player_of(actor) else { return call_original(c, ROBOT_CAMERA) };
    let before = m.rd32(data + MODE) & 0xff;
    let timer = m.rd16(data + KEY_TIMER) as i16;
    call_original(c, ROBOT_CAMERA);
    let after = m.rd32(data + MODE) & 0xff;
    // a press sets the timer up again (it only runs down otherwise)
    let pressed = m.rd16(data + KEY_TIMER) as i16 > timer;
    let mode = {
        let mut s = STATE.lock().unwrap();
        // one press can count twice while the key is held (we set the game's mode back to 0)
        let now = std::time::Instant::now();
        let pressed = pressed && s.last[p].is_none_or(|t| now.duration_since(t).as_millis() >= 250);
        if pressed {
            s.last[p] = Some(now);
        }
        // FCOP_CAMERA=first / behind: player 1 starts with that camera (tests)
        static START: std::sync::Once = std::sync::Once::new();
        START.call_once(|| {
            s.mode[0] = match std::env::var("FCOP_CAMERA").as_deref() {
                Ok("first") => Mode::FirstPerson,
                Ok("behind") => Mode::Behind,
                _ => Mode::Game,
            }
        });
        if pressed {
            // ours come after the game's last mode (it skips 5 and 6 when their view is
            // blocked, back to 0 at once), then its first again. First person is left out
            // for now (FCOP_CAMERA=first only): looking ahead the game crashed in its
            // visibility pass and projection (see PLAN.md)
            s.mode[p] = match s.mode[p] {
                Mode::Game if after == 0 && before != 0 => Mode::Behind,
                Mode::Game => Mode::Game,
                Mode::FirstPerson => Mode::Behind,
                Mode::Behind => {
                    // back to the game's first mode (the press counted it up to 1)
                    m.wr8(data + MODE, 0);
                    Mode::Game
                }
            };
        }
        s.mode[p]
    };
    if mode == Mode::Game {
        return;
    }
    // the game goes on in its first mode meanwhile
    m.wr8(data + MODE, 0);
    let yaw = m.rd32(actor + 0xb0) as i16 as i32;
    let w = yaw as f64 / 4096.0 * std::f64::consts::TAU;
    let (fx, fz) = (w.sin(), w.cos());
    let pos = [m.rd32(actor + 0x50) as i32 as f64, m.rd32(actor + 0x54) as i32 as f64, m.rd32(actor + 0x58) as i32 as f64];
    // eye: height above the robot's origin and forward of it; behind: back and up
    let (back, up, pitch) = match mode {
        Mode::FirstPerson => (-1.0, 3.5, 40),
        _ => (8.0, 5.0, 190),
    };
    let at = [pos[0] - fx * back * UNIT, pos[1] + up * UNIT, pos[2] - fz * back * UNIT];
    let cam = guest_call(c, CAMERA_OF, &[m.rd32(data + 0xac) as i8 as i32 as u32]);
    if cam == 0 {
        return;
    }
    let buf = {
        let mut s = STATE.lock().unwrap();
        if s.buf == 0 {
            s.buf = super::alloc(12);
        }
        s.buf
    };
    let saved = m.rd32(CAMERA);
    m.wr32(CAMERA, cam);
    let yaw = ((yaw & 0xfff) + 0x800) % 0x1000 - 0x800;
    guest_call(c, SET_ANGLES, &[yaw as u32, pitch as u32, 0]);
    for (k, v) in at.iter().enumerate() {
        m.wr32(buf + 4 * k as u32, *v as i32 as u32);
    }
    guest_call(c, SET_POSITION, &[buf]);
    m.wr32(CAMERA, saved);
}

/// In first person the robot is not drawn in its own view.
pub fn robot_draw(c: &mut Cpu) {
    let m = mem();
    let part = m.rd32(c.r[ESP] + 4);
    let robot = m.rd32(part + 0xb4);
    if mode_of(robot) == Mode::FirstPerson && own_view(c, robot) {
        // as if it returned: its return address off the stack
        c.r[ESP] += 4;
        return;
    }
    call_original(c, ROBOT_DRAW);
}

fn mode_of(robot: u32) -> Mode {
    player_of(robot).map_or(Mode::Game, |p| STATE.lock().unwrap().mode[p])
}

/// Whether the view being drawn is the robot's own.
fn own_view(c: &mut Cpu, robot: u32) -> bool {
    let m = mem();
    let data = m.rd32(robot + 0xac);
    guest_call(c, CAMERA_OF, &[m.rd32(data + 0xac) as i8 as i32 as u32]) == m.rd32(CAMERA)
}

/// After the HUD's target markers (0x435330) of `player`, which added display list records
/// from `from` on: in our cameras the line from the robot to its target (the last record,
/// type 0x3a) is taken out again, the markers stay.
pub fn drop_laser(player: u32, from: u32) {
    const LIST_END: u32 = 0x511dc0;
    let m = mem();
    let end = m.rd32(LIST_END);
    if mode_of(player) != Mode::Game && end >= from + 0x38 && m.rd8(end - 0x38 + 4) == 0x3a {
        m.wr32(LIST_END, end - 0x38);
    }
}
