//! DirectInput 5 for the game's joysticks (it reads the keyboard with GetAsyncKeyState), on
//! gamepads through gilrs.
//!
//! joy_init 0x412590 (run at start and again later, after its cleanup 0x412740 released
//! everything) enumerates attached joysticks into room for two (no bounds check: we report at
//! most two), creates each one, asks for IDirectInputDevice2A, sets c_dfDIJoystick (80-byte
//! DIJOYSTATE), a range of -1000...1000 and a dead zone of 25% on X and Y, and acquires it.
//! Each frame joy_poll 0x412b80 polls and reads the state; any error but DIERR_INPUTLOST makes
//! it give the pad up, so we always answer DI_OK. The game reads only lX, lY (by their sign:
//! inside the dead zone they must be exactly 0) and buttons 0...16 (joy_key 0x412d70); with
//! two pads player i uses pad i, with one both share it. The original code loses the axis
//! magnitude there; hooks around player robot physics restore proportional translation and
//! turning in local games while keeping its direction handling, collisions, and gravity.
//!
//! Our button order makes the game's own joystick profile 1 a modern layout: 0 RT gun, 1 LT
//! heavy weapon, 2 B special, 3 A jump, 4 X action, 5 Y change target, 6 LB / 7 RB jink, 8 Back
//! map, 9 R3 camera, 10 L3, 11...14 D-pad up, down, left, right (free to bind). The left stick
//! gives lX/lY. Start is the Escape key (pause). The Controller dialog takes left-stick
//! directions and buttons 0...9 only (joy_capture 0x412c10): hooked for 10...16 and the right
//! stick. Right-stick directions use our binding keys 0x16...0x19; joy_key and controls.txt
//! parsing/formatting are hooked too, so those assignments work and persist.
//!
//! gilrs lives on the main thread and is polled at every `window::pump`; the game's threads
//! read the last state. `FCOP_PAD_LOG=1` logs pads and the game's use of them. `pad@...` items
//! in FCOP_INPUT drive a scripted pad (pad 0, before the real ones).

use super::{Import, alloc, arg, cstr, free, guest_call, log, mem, put_cstr};
use crate::rt::{Cpu, EAX, ESP, call_original};
use gilrs::{Axis, Button, Gilrs};
use std::cell::{Cell, RefCell};
use std::sync::Mutex;

const DI_OK: u32 = 0;
const E_NOTIMPL: u32 = 0x8000_4001;
const E_NOINTERFACE: u32 = 0x8000_4002;
const E_INVALIDARG: u32 = 0x8007_0057;
const DIERR_DEVICENOTREG: u32 = 0x8004_0154;

const DIDEVTYPE_JOYSTICK: u32 = 4;
const DIDEVTYPEJOYSTICK_GAMEPAD: u32 = 3;
const DIDC_ATTACHED: u32 = 1;
const DIPROP_RANGE: u32 = 4;
const DIPROP_DEADZONE: u32 = 5;
const DIPH_DEVICE: u32 = 0;
const DIPH_BYOFFSET: u32 = 1;

/// A GUID as its 16 bytes in memory.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (d1.to_le_bytes(), d2.to_le_bytes(), d3.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
}

const DI_D4: [u8; 8] = [0xbf, 0xc7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00];
const IID_IUNKNOWN: [u8; 16] = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IDIRECTINPUTA: [u8; 16] = guid(0x8952_1360, 0xaa8a, 0x11cf, DI_D4);
const IID_IDIRECTINPUTDEVICEA: [u8; 16] = guid(0x5944_e680, 0xc92e, 0x11cf, DI_D4);
const IID_IDIRECTINPUTDEVICE2A: [u8; 16] = guid(0x5944_e682, 0xc92e, 0x11cf, DI_D4);
/// Pad n's instance GUID has 0xfc0b0000 + n in its first dword; the product GUID is the base.
const PAD_GUID: u32 = 0xfc0b_0000;

fn pad_guid(n: u32) -> [u8; 16] {
    guid(PAD_GUID + n, 0xd5a0, 0x11cf, DI_D4)
}

fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var("FCOP_PAD_LOG").is_ok_and(|v| v != "0"))
}

macro_rules! pad_log {
    ($($t:tt)*) => { if trace() { log!("pad: {}", format!($($t)*)) } };
}

// ---- pad state --------------------------------------------------------------------------------

/// One pad as the game sees it: left and right sticks (-1...1, y down), our DirectInput
/// buttons (bit n = button n), Start.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Pad {
    stick: [f32; 4],
    buttons: u32,
    start: bool,
}

/// A connected pad: its state, the buttons pressed since the game last read it (a short press
/// between two reads still counts: the Controller dialog reads every 250 ms), gilrs' id (None:
/// the scripted pad), its name.
struct Entry {
    pad: Pad,
    pressed: u32,
    id: Option<usize>,
    name: String,
}

/// The connected pads (the scripted one first).
static PADS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
/// Start's new state on the first pad, for the next pump (it becomes the Escape key).
static START: Mutex<Option<bool>> = Mutex::new(None);
/// The scripted pad, once FCOP_INPUT has a `pad@` item.
static SCRIPTED: Mutex<Option<Pad>> = Mutex::new(None);

// Analog movement in the current robot-physics call. The original game turns the stick
// into four bits; the movement-vector hooks below put its magnitude back.
thread_local! {
    static ANALOG_MOVE: Cell<Option<[f32; 2]>> = const { Cell::new(None) };
}

fn scripted_pad() -> bool {
    static S: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *S.get_or_init(|| std::env::var("FCOP_INPUT").is_ok_and(|v| v.contains("pad@")))
}

thread_local! {
    /// gilrs on the main thread; None when it could not start (then no pads)
    /// (never dropped: its thread must not end at exit)
    static GILRS: RefCell<Option<Option<&'static mut Gilrs>>> = const { RefCell::new(None) };
}

const BUTTONS: [Button; 13] = [
    Button::East,
    Button::South,
    Button::West,
    Button::North,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::Select,
    Button::RightThumb,
    Button::LeftThumb,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
];
/// our button numbers for `BUTTONS`
const BUTTON_NUMBERS: [u32; 13] = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];

/// Our button for a gilrs event that presses one.
fn pressed_by(e: &gilrs::EventType) -> Option<u32> {
    match *e {
        gilrs::EventType::ButtonChanged(Button::RightTrigger2, v, _) if v > 0.3 => Some(0),
        gilrs::EventType::ButtonChanged(Button::LeftTrigger2, v, _) if v > 0.3 => Some(1),
        gilrs::EventType::ButtonPressed(b, _) => BUTTONS.iter().position(|&x| x == b).map(|k| BUTTON_NUMBERS[k]),
        _ => None,
    }
}

fn read(g: &gilrs::Gamepad) -> Pad {
    let trigger = |b| g.button_data(b).map_or(0.0, |d| d.value()) > 0.3;
    let mut buttons = trigger(Button::RightTrigger2) as u32 | (trigger(Button::LeftTrigger2) as u32) << 1;
    for (b, n) in BUTTONS.into_iter().zip(BUTTON_NUMBERS) {
        buttons |= (g.is_pressed(b) as u32) << n;
    }
    Pad {
        stick: [
            g.value(Axis::LeftStickX),
            -g.value(Axis::LeftStickY),
            g.value(Axis::RightStickX),
            -g.value(Axis::RightStickY),
        ],
        buttons,
        start: g.is_pressed(Button::Start),
    }
}

fn new_gilrs() -> Option<&'static mut Gilrs> {
    let g = Gilrs::new().map_err(|e| log!("warn: no gamepads: {e}")).ok()?;
    Some(Box::leak(Box::new(g)))
}

/// Start gilrs on the main thread before the game: pads are found some time after (Windows
/// reports them ~75 ms later), and the game looks for them once, at its start.
pub fn start() {
    GILRS.with(|g| {
        g.borrow_mut().get_or_insert_with(new_gilrs);
    });
}

/// Read the pads (main thread, at every pump). Returns Start's new state when it changed on the
/// first pad (it becomes the Escape key).
pub fn poll() -> Option<bool> {
    update();
    START.lock().unwrap().take()
}

fn update() {
    let scripted = {
        let mut s = SCRIPTED.lock().unwrap();
        if scripted_pad() {
            s.get_or_insert_default();
        }
        *s
    };
    let mut pads = PADS.lock().unwrap();
    let pressed_before = |id| pads.iter().find(|e| e.id == id).map_or(0, |e| e.pressed);
    let mut now: Vec<Entry> = scripted
        .map(|pad| Entry { pad, pressed: pressed_before(None) | pad.buttons, id: None, name: "Scripted pad".to_string() })
        .into_iter()
        .collect();
    GILRS.with(|g| {
        let mut g = g.borrow_mut();
        let Some(g) = g.get_or_insert_with(new_gilrs) else { return };
        let mut pressed: Vec<(usize, u32)> = Vec::new();
        while let Some(e) = g.next_event() {
            match e.event {
                gilrs::EventType::Connected => pad_log!("connected: {}", g.gamepad(e.id).name()),
                gilrs::EventType::Disconnected => pad_log!("disconnected: {}", g.gamepad(e.id).name()),
                ref ev => {
                    if let Some(b) = pressed_by(ev) {
                        pressed.push((e.id.into(), 1 << b));
                    }
                }
            }
        }
        now.extend(g.gamepads().map(|(id, p)| {
            let id: usize = id.into();
            let pad = read(&p);
            let new = pressed.iter().filter(|e| e.0 == id).fold(0, |a, e| a | e.1);
            Entry { pad, pressed: pressed_before(Some(id)) | new | pad.buttons, id: Some(id), name: p.name().to_string() }
        }));
    });
    let was = pads.first().is_some_and(|e| e.pad.start);
    let start = now.first().is_some_and(|e| e.pad.start);
    *pads = now;
    if was != start {
        *START.lock().unwrap() = Some(start);
    }
}

/// Pad n as the game reads it: its buttons held or pressed since the last read.
fn pad(n: u32) -> Pad {
    // fresh on the main thread (the Controller dialog reads from there)
    if GILRS.with(|g| g.borrow().is_some()) {
        update();
    }
    let mut pads = PADS.lock().unwrap();
    let Some(e) = pads.get_mut(n as usize) else { return Pad::default() };
    let mut p = e.pad;
    p.buttons |= std::mem::take(&mut e.pressed);
    p
}

/// The scripted pad's controls (FCOP_INPUT `pad@A*500`, `pad@LX=-1*1000`): our buttons in
/// order (the D-pad's too), then Start and both sticks' axes (y up, as gilrs has it).
pub const CONTROLS: [&str; 20] = [
    "RT", "LT", "B", "A", "X", "Y", "LB", "RB", "Back", "R3", "L3", "Up", "Down", "Left", "Right", "Start", "LX", "LY",
    "RX", "RY",
];

/// Set one control of the scripted pad (`CONTROLS[k]`): held when `v` is not 0, or the axis' value.
pub fn script(k: usize, v: f32) {
    let mut s = SCRIPTED.lock().unwrap();
    let p = s.get_or_insert_default();
    let on = v != 0.0;
    match k {
        0..=14 => p.buttons = p.buttons & !(1 << k) | (on as u32) << k,
        15 => p.start = on,
        16 => p.stick[0] = v,
        17 => p.stick[1] = -v,
        18 => p.stick[2] = v,
        _ => p.stick[3] = -v,
    }
}

/// Make sure the pads were read once (EnumDevices may come before the first pump).
fn ensure_polled() {
    super::window::pump(std::time::Duration::ZERO);
}

// ---- IDirectInputA ----------------------------------------------------------------------------
// object: vtable, references

fn guid_at(p: u32) -> [u8; 16] {
    mem().slice(p, 16).try_into().unwrap()
}

fn direct_input_create(c: &mut Cpu) -> u32 {
    let (version, out) = (arg(c, 1), arg(c, 2));
    let o = super::com_new(DIRECT_INPUT, 4);
    mem().wr32(o + 4, 1);
    mem().wr32(out, o);
    pad_log!("DirectInputCreateA(version {version:#x})");
    DI_OK
}

fn di_query(c: &mut Cpu) -> u32 {
    let (this, iid, out) = (arg(c, 0), guid_at(arg(c, 1)), arg(c, 2));
    if iid != IID_IUNKNOWN && iid != IID_IDIRECTINPUTA {
        mem().wr32(out, 0);
        return E_NOINTERFACE;
    }
    add_ref(c);
    mem().wr32(out, this);
    DI_OK
}

/// AddRef and Release of both interfaces: the count is at +4.
fn add_ref(c: &mut Cpu) -> u32 {
    let o = arg(c, 0);
    let n = mem().rd32(o + 4) + 1;
    mem().wr32(o + 4, n);
    n
}

fn release(c: &mut Cpu) -> u32 {
    let o = arg(c, 0);
    let n = mem().rd32(o + 4).saturating_sub(1);
    mem().wr32(o + 4, n);
    if n == 0 {
        free(o);
    }
    n
}

fn create_device(c: &mut Cpu) -> u32 {
    let (g, out) = (guid_at(arg(c, 1)), arg(c, 2));
    let n = u32::from_le_bytes(g[..4].try_into().unwrap()).wrapping_sub(PAD_GUID);
    if n >= 2 || g != pad_guid(n) {
        mem().wr32(out, 0);
        return DIERR_DEVICENOTREG;
    }
    let d = super::com_new(DEVICE, DEVICE_SIZE);
    let m = mem();
    m.wr32(d + 4, 1);
    m.wr32(d + 8, n);
    for axis in 0..2 {
        m.wr32(d + RANGE + 8 * axis, 0);
        m.wr32(d + RANGE + 8 * axis + 4, 0xffff);
    }
    m.wr32(out, d);
    pad_log!("CreateDevice: pad {n}");
    DI_OK
}

/// EnumDevices(type, callback, ref, flags): the pads (at most two), DIDEVICEINSTANCEA (DX5).
fn enum_devices(c: &mut Cpu) -> u32 {
    let (kind, cb, user) = (arg(c, 1) & 0xff, arg(c, 2), arg(c, 3));
    if kind != 0 && kind != DIDEVTYPE_JOYSTICK {
        return DI_OK;
    }
    ensure_polled();
    let names: Vec<String> = PADS.lock().unwrap().iter().take(2).map(|e| e.name.clone()).collect();
    let ddi = alloc(0x244);
    let m = mem();
    for (n, name) in names.iter().enumerate() {
        pad_log!("EnumDevices: pad {n}: {name}");
        m.slice(ddi, 0x244).fill(0);
        m.wr32(ddi, 0x244);
        m.slice(ddi + 4, 16).copy_from_slice(&pad_guid(n as u32));
        m.slice(ddi + 0x14, 16).copy_from_slice(&pad_guid(0));
        m.wr32(ddi + 0x24, DIDEVTYPE_JOYSTICK | DIDEVTYPEJOYSTICK_GAMEPAD << 8);
        put_cstr(ddi + 0x28, name, 260);
        put_cstr(ddi + 0x12c, name, 260);
        if guest_call(c, cb, &[ddi, user]) == 0 {
            break;
        }
    }
    free(ddi);
    DI_OK
}

fn ok(_: &mut Cpu) -> u32 {
    DI_OK
}

fn not_impl(_: &mut Cpu) -> u32 {
    E_NOTIMPL
}

static DIRECT_INPUT: &[Import] = &[
    ("IDirectInputA::QueryInterface", 3, di_query),
    ("IDirectInputA::AddRef", 1, add_ref),
    ("IDirectInputA::Release", 1, release),
    ("IDirectInputA::CreateDevice", 4, create_device),
    ("IDirectInputA::EnumDevices", 5, enum_devices),
    ("IDirectInputA::GetDeviceStatus", 2, ok),
    ("IDirectInputA::RunControlPanel", 3, ok),
    ("IDirectInputA::Initialize", 3, ok),
];

// ---- IDirectInputDevice2A ---------------------------------------------------------------------
// object: vtable, references, pad number, X and Y: range (min, max), dead zone (0...10000)

const RANGE: u32 = 0xc;
const DEAD_ZONE: u32 = 0x1c;
const DEVICE_SIZE: u32 = 0x20;

fn dev_query(c: &mut Cpu) -> u32 {
    let (this, iid, out) = (arg(c, 0), guid_at(arg(c, 1)), arg(c, 2));
    if ![IID_IUNKNOWN, IID_IDIRECTINPUTDEVICEA, IID_IDIRECTINPUTDEVICE2A].contains(&iid) {
        mem().wr32(out, 0);
        return E_NOINTERFACE;
    }
    add_ref(c);
    mem().wr32(out, this);
    DI_OK
}

/// GetCapabilities(DIDEVCAPS): attached, no force feedback, 2 axes, 15 buttons, no POV.
fn get_capabilities(c: &mut Cpu) -> u32 {
    let p = arg(c, 1);
    let m = mem();
    let size = m.rd32(p);
    if size < 24 {
        return E_INVALIDARG;
    }
    m.slice(p + 4, size as usize - 4).fill(0);
    for (k, v) in [DIDC_ATTACHED, DIDEVTYPE_JOYSTICK | DIDEVTYPEJOYSTICK_GAMEPAD << 8, 2, 15, 0].into_iter().enumerate() {
        m.wr32(p + 4 + 4 * k as u32, v);
    }
    DI_OK
}

/// The axes (0 X, 1 Y) a property header (dwObj +8, dwHow +0xc) names.
fn axes_of(h: u32) -> std::ops::Range<u32> {
    match (mem().rd32(h + 0xc), mem().rd32(h + 8)) {
        (DIPH_DEVICE, _) => 0..2,
        (DIPH_BYOFFSET, o @ (0 | 4)) => o / 4..o / 4 + 1,
        _ => 0..0,
    }
}

/// SetProperty(property, header): the range and the dead zone of X and Y; others are accepted.
fn set_property(c: &mut Cpu) -> u32 {
    let (d, prop, h) = (arg(c, 0), arg(c, 1), arg(c, 2));
    let m = mem();
    for axis in axes_of(h) {
        match prop {
            DIPROP_RANGE => {
                m.wr32(d + RANGE + 8 * axis, m.rd32(h + 0x10));
                m.wr32(d + RANGE + 8 * axis + 4, m.rd32(h + 0x14));
            }
            DIPROP_DEADZONE => m.wr32(d + DEAD_ZONE + 4 * axis, m.rd32(h + 0x10)),
            _ => {}
        }
    }
    pad_log!("SetProperty({prop}) of pad {}", m.rd32(d + 8));
    DI_OK
}

fn get_property(c: &mut Cpu) -> u32 {
    let (d, prop, h) = (arg(c, 0), arg(c, 1), arg(c, 2));
    let m = mem();
    let axis = axes_of(h).start.min(1);
    match prop {
        DIPROP_RANGE => {
            m.wr32(h + 0x10, m.rd32(d + RANGE + 8 * axis));
            m.wr32(h + 0x14, m.rd32(d + RANGE + 8 * axis + 4));
        }
        DIPROP_DEADZONE => m.wr32(h + 0x10, m.rd32(d + DEAD_ZONE + 4 * axis)),
        _ => return E_NOTIMPL,
    }
    DI_OK
}

fn acquire(c: &mut Cpu) -> u32 {
    pad_log!("Acquire: pad {}", mem().rd32(arg(c, 0) + 8));
    DI_OK
}

/// An axis value in the device's range: the stick outside its dead zone, else exactly the
/// centre (the game tests the sign).
fn axis_value(d: u32, axis: u32, p: &Pad) -> u32 {
    let m = mem();
    let (lo, hi) = (m.rd32(d + RANGE + 8 * axis) as i32 as f32, m.rd32(d + RANGE + 8 * axis + 4) as i32 as f32);
    let dz = m.rd32(d + DEAD_ZONE + 4 * axis).min(9999) as f32 / 10000.0;
    let s = p.stick[axis as usize].clamp(-1.0, 1.0);
    let v = if s.abs() > dz { s.signum() * (s.abs() - dz) / (1.0 - dz) } else { 0.0 };
    let centre = (lo + hi) / 2.0;
    if v == 0.0 {
        return centre.floor() as i32 as u32;
    }
    let x = centre + v * (hi - lo) / 2.0;
    // away from the centre, so a small push never reads as the centre
    (if v > 0.0 { x.ceil() } else { x.floor() }).clamp(lo, hi) as i32 as u32
}

/// GetDeviceState(size, DIJOYSTATE): lX, lY, the other axes centred, POVs centred, buttons.
fn get_device_state(c: &mut Cpu) -> u32 {
    let (d, size, out) = (arg(c, 0), arg(c, 1), arg(c, 2));
    let m = mem();
    let p = pad(m.rd32(d + 8));
    let mut s = [0u8; 0x50];
    s[0..4].copy_from_slice(&axis_value(d, 0, &p).to_le_bytes());
    s[4..8].copy_from_slice(&axis_value(d, 1, &p).to_le_bytes());
    s[0x20..0x30].fill(0xff);
    for b in 0..32 {
        s[0x30 + b] = if p.buttons >> b & 1 != 0 { 0x80 } else { 0 };
    }
    let n = (size as usize).min(s.len());
    m.slice(out, n).copy_from_slice(&s[..n]);
    DI_OK
}

static DEVICE: &[Import] = &[
    ("IDirectInputDevice2A::QueryInterface", 3, dev_query),
    ("IDirectInputDevice2A::AddRef", 1, add_ref),
    ("IDirectInputDevice2A::Release", 1, release),
    ("IDirectInputDevice2A::GetCapabilities", 2, get_capabilities),
    ("IDirectInputDevice2A::EnumObjects", 4, ok),
    ("IDirectInputDevice2A::GetProperty", 3, get_property),
    ("IDirectInputDevice2A::SetProperty", 3, set_property),
    ("IDirectInputDevice2A::Acquire", 1, acquire),
    ("IDirectInputDevice2A::Unacquire", 1, ok),
    ("IDirectInputDevice2A::GetDeviceState", 3, get_device_state),
    ("IDirectInputDevice2A::GetDeviceData", 5, not_impl),
    ("IDirectInputDevice2A::SetDataFormat", 2, ok),
    ("IDirectInputDevice2A::SetEventNotification", 2, ok),
    ("IDirectInputDevice2A::SetCooperativeLevel", 3, ok),
    ("IDirectInputDevice2A::GetObjectInfo", 4, not_impl),
    ("IDirectInputDevice2A::GetDeviceInfo", 2, not_impl),
    ("IDirectInputDevice2A::RunControlPanel", 3, ok),
    ("IDirectInputDevice2A::Initialize", 4, ok),
    ("IDirectInputDevice2A::CreateEffect", 5, not_impl),
    ("IDirectInputDevice2A::EnumEffects", 4, ok),
    ("IDirectInputDevice2A::GetEffectInfo", 3, not_impl),
    ("IDirectInputDevice2A::GetForceFeedbackState", 2, not_impl),
    ("IDirectInputDevice2A::SendForceFeedbackCommand", 2, not_impl),
    ("IDirectInputDevice2A::EnumCreatedEffectObjects", 4, ok),
    ("IDirectInputDevice2A::Escape", 2, not_impl),
    ("IDirectInputDevice2A::Poll", 1, ok),
    ("IDirectInputDevice2A::SendDeviceData", 5, not_impl),
];

// ---- the Controller dialog and extended axes ---------------------------------------------------

/// The original keys 0x12...0x15 are left, right, up, down on the left stick. These four
/// continue that private key space for the right stick; they are handled by our hooks only.
const RIGHT_LEFT: u32 = 0x16;
const RIGHT_RIGHT: u32 = 0x17;
const RIGHT_UP: u32 = 0x18;
const RIGHT_DOWN: u32 = 0x19;
const RIGHT_KEYS: [(&str, u32); 4] = [
    // The Controller dialog reads at most 19 characters plus NUL from each field. With its
    // "Joystick: " prefix these names make exactly 18 characters and survive OK/save.
    ("RStick L", RIGHT_LEFT),
    ("RStick R", RIGHT_RIGHT),
    ("RStick U", RIGHT_UP),
    ("RStick D", RIGHT_DOWN),
];
const AXIS_DEAD_ZONE: f32 = 0.25;

fn right_key_from_text(text: &str) -> Option<u32> {
    text.split_once(':').and_then(|(device, name)| {
        device.trim().eq_ignore_ascii_case("Joystick").then(|| name.trim()).and_then(|name| {
            RIGHT_KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|e| e.1)
        })
    })
}

fn right_stick_key(p: &Pad) -> Option<u32> {
    let [x, y] = [p.stick[2], p.stick[3]];
    if x < -AXIS_DEAD_ZONE {
        Some(RIGHT_LEFT)
    } else if x > AXIS_DEAD_ZONE {
        Some(RIGHT_RIGHT)
    } else if y < -AXIS_DEAD_ZONE {
        Some(RIGHT_UP)
    } else if y > AXIS_DEAD_ZONE {
        Some(RIGHT_DOWN)
    } else {
        None
    }
}

fn right_stick_down(p: &Pad, key: u32) -> bool {
    match key {
        RIGHT_LEFT => p.stick[2] < -AXIS_DEAD_ZONE,
        RIGHT_RIGHT => p.stick[2] > AXIS_DEAD_ZONE,
        RIGHT_UP => p.stick[3] < -AXIS_DEAD_ZONE,
        RIGHT_DOWN => p.stick[3] > AXIS_DEAD_ZONE,
        _ => false,
    }
}

fn current_pad(n: u32) -> Pad {
    PADS.lock().unwrap().get(n as usize).map_or(Pad::default(), |e| e.pad)
}

/// joy_key(pad, key): extend the game's four axis keys with the right stick directions.
pub const JOY_KEY: u32 = 0x412d70;

pub fn joy_key(c: &mut Cpu) {
    let (pad, key) = (mem().rd32(c.r[ESP] + 4), mem().rd32(c.r[ESP] + 8));
    if (RIGHT_LEFT..=RIGHT_DOWN).contains(&key) {
        c.r[EAX] = right_stick_down(&current_pad(pad), key) as u32;
        // The original is not called on this path, so emulate its ret.
        c.r[ESP] += 4;
    } else {
        call_original(c, JOY_KEY);
    }
}

/// joy_capture(pad, &binding): the stick direction or button pressed, as a binding (device 1,
/// key); al = 1 when one was. The game looks at buttons 0...9 only: hooked for 10...16 (L3).
pub const JOY_CAPTURE: u32 = 0x412c10;
const JOY_STATES: u32 = 0x49fd98;

pub fn joy_capture(c: &mut Cpu) {
    let m = mem();
    let (pad, binding) = (m.rd32(c.r[ESP] + 4), m.rd32(c.r[ESP] + 8));
    call_original(c, JOY_CAPTURE);
    if c.r[EAX] & 0xff != 0 || pad >= 2 {
        return;
    }
    if let Some(key) = right_stick_key(&current_pad(pad)) {
        m.wr32(binding, 1);
        m.wr32(binding + 4, key);
        c.r[EAX] = c.r[EAX] & !0xff | 1;
        return;
    }
    let buttons = JOY_STATES + 0x50 * pad + 0x30;
    if let Some(b) = (10..=16).find(|&b| m.rd8(buttons + b) & 0x80 != 0) {
        m.wr32(binding, 1);
        m.wr32(binding + 4, b);
        c.r[EAX] = c.r[EAX] & !0xff | 1;
    }
}

/// controls.txt parsing and display. The game's fixed name table ends at the left stick and
/// button 16, so the new right-stick keys need names in both directions to survive a restart.
pub const BINDING_PARSE: u32 = 0x417870;
pub const BINDING_NAME: u32 = 0x4179e0;
const BINDING_NAME_BUF: u32 = 0x4a0410;

pub fn binding_parse(c: &mut Cpu) {
    let m = mem();
    let (text, binding) = (m.rd32(c.r[ESP] + 4), m.rd32(c.r[ESP] + 8));
    let text = cstr(text);
    let custom = right_key_from_text(&text);
    if let Some(key) = custom {
        m.wr32(binding, 1);
        m.wr32(binding + 4, key);
        c.r[EAX] = 1;
        // The original is not called on this path, so emulate its ret.
        c.r[ESP] += 4;
    } else {
        // The original parser writes the device before it knows whether the name is valid.
        // Keep an invalid/truncated dialog field from leaving a half-updated live binding.
        let old = [m.rd32(binding), m.rd32(binding + 4)];
        call_original(c, BINDING_PARSE);
        if c.r[EAX] & 0xff == 0 {
            m.wr32(binding, old[0]);
            m.wr32(binding + 4, old[1]);
        }
    }
}

pub fn binding_name(c: &mut Cpu) {
    let binding = mem().rd32(c.r[ESP] + 4);
    let custom = (mem().rd32(binding) == 1)
        .then(|| mem().rd32(binding + 4))
        .and_then(|key| RIGHT_KEYS.iter().find(|e| e.1 == key));
    if let Some((name, _)) = custom {
        put_cstr(BINDING_NAME_BUF, &format!("Joystick: {name}"), 64);
        c.r[EAX] = BINDING_NAME_BUF;
        // The original is not called on this path, so emulate its ret.
        c.r[ESP] += 4;
    } else {
        call_original(c, BINDING_NAME);
    }
}

// ---- analog player movement ------------------------------------------------------------------

/// Player robot physics and the helpers that build its desired horizontal movement vector.
pub const ROBOT_PHYSICS: u32 = 0x442120;
pub const HOVER_MOVE_VECTOR: u32 = 0x443e80;
pub const WALK_MOVE_VECTOR: u32 = 0x444f40;

const PLAYERS: u32 = 0x511fd0;
const BINDINGS: u32 = 0x511c20;
const BINDINGS_SIZE: u32 = 0x80;
const JOY_COUNT: u32 = 0x49fe4c;
const NET: u32 = 0x511fa0;
const ROBOT_DATA: u32 = 0xac;
const CONTROLS_BITS: u32 = 0x30;
const HEADING: u32 = 0xb0;
const MOVE_BITS: u32 = 0xf000_0000;
const TURN_BITS: u32 = 0x3000_0000;

/// DirectInput's per-axis dead zone, as configured by the game. The remaining travel is
/// rescaled, so leaving the dead zone starts at zero speed.
fn movement_axis(v: f32) -> f32 {
    const DEAD_ZONE: f32 = 0.25;
    let v = v.clamp(-1.0, 1.0);
    if v.abs() <= DEAD_ZONE {
        0.0
    } else {
        v.signum() * (v.abs() - DEAD_ZONE) / (1.0 - DEAD_ZONE)
    }
}

/// Stick value for a player whose four movement actions are bound to joystick axes. Network
/// input contains only direction bits, not magnitude; retaining digital movement there keeps
/// both peers' simulations deterministic.
fn analog_for(actor: u32) -> Option<[f32; 2]> {
    let m = mem();
    if m.rd32(NET) as i32 >= 0 {
        return None;
    }
    let player = (0..2).find(|&p| m.rd32(PLAYERS + 4 * p) == actor)?;
    let bindings = BINDINGS + BINDINGS_SIZE * player;
    let axes_bound = (0..4).all(|k| {
        let b = bindings + 8 * k;
        m.rd32(b) == 1 && (0x12..=0x15).contains(&m.rd32(b + 4))
    });
    if !axes_bound {
        return None;
    }
    let pad = if m.rd32(JOY_COUNT) > 1 { player } else { 0 };
    let pads = PADS.lock().unwrap();
    let stick = pads.get(pad as usize)?.pad.stick;
    Some([movement_axis(stick[0]), movement_axis(stick[1])])
}

fn scale_move_speed(speed: i32, stick: [f32; 2]) -> i32 {
    let amount = stick[0].hypot(stick[1]).min(1.0);
    (speed as f32 * amount).round() as i32
}

fn move_vector(c: &mut Cpu, addr: u32) {
    ANALOG_MOVE.with(|a| {
        if let Some(stick) = a.get() {
            let speed = mem().rd32(c.r[ESP] + 8) as i32;
            mem().wr32(c.r[ESP] + 8, scale_move_speed(speed, stick) as u32);
        }
    });
    call_original(c, addr);
}

pub fn hover_move_vector(c: &mut Cpu) {
    move_vector(c, HOVER_MOVE_VECTOR);
}

pub fn walk_move_vector(c: &mut Cpu) {
    move_vector(c, WALK_MOVE_VECTOR);
}

/// Run the original physics with proportional desired translation, then scale only the
/// heading change requested by left/right. The heading is a 12-bit angle.
pub fn robot_physics(c: &mut Cpu) {
    let m = mem();
    let actor = m.rd32(c.r[ESP] + 4);
    let data = m.rd32(actor + ROBOT_DATA);
    let controls = m.rd32(data + CONTROLS_BITS);
    let analog = (controls & MOVE_BITS != 0).then(|| analog_for(actor)).flatten();
    let before = m.rd16(actor + HEADING) as i32 & 0xfff;
    let previous = ANALOG_MOVE.replace(analog);
    call_original(c, ROBOT_PHYSICS);
    ANALOG_MOVE.set(previous);

    let Some(stick) = analog else { return };
    if controls & TURN_BITS == 0 {
        return;
    }
    let after = m.rd16(actor + HEADING) as i32 & 0xfff;
    let delta = (after - before + 0x800).rem_euclid(0x1000) - 0x800;
    let heading = before + (delta as f32 * stick[0].abs()).round() as i32;
    m.wr16(actor + HEADING, heading.rem_euclid(0x1000) as u16);
}

#[cfg(test)]
mod analog_tests {
    use super::{
        Pad, RIGHT_DOWN, RIGHT_KEYS, RIGHT_LEFT, RIGHT_RIGHT, RIGHT_UP, movement_axis, right_key_from_text, right_stick_down,
        right_stick_key, scale_move_speed,
    };

    #[test]
    fn dead_zone_is_removed_and_remaining_travel_is_rescaled() {
        assert_eq!(movement_axis(0.25), 0.0);
        assert!((movement_axis(0.50) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(movement_axis(-1.0), -1.0);
    }

    #[test]
    fn movement_speed_follows_radial_stick_travel() {
        assert_eq!(scale_move_speed(12_000, [0.0, 0.25]), 3_000);
        assert_eq!(scale_move_speed(12_000, [0.8, 0.8]), 12_000);
        assert_eq!(scale_move_speed(-9_000, [0.0, 0.5]), -4_500);
    }

    #[test]
    fn right_stick_directions_use_extended_binding_keys() {
        let pad = |x, y| Pad { stick: [0.0, 0.0, x, y], ..Pad::default() };
        assert_eq!(right_stick_key(&pad(-1.0, 0.0)), Some(RIGHT_LEFT));
        assert_eq!(right_stick_key(&pad(1.0, 0.0)), Some(RIGHT_RIGHT));
        assert_eq!(right_stick_key(&pad(0.0, -1.0)), Some(RIGHT_UP));
        assert_eq!(right_stick_key(&pad(0.0, 1.0)), Some(RIGHT_DOWN));
        assert_eq!(right_stick_key(&pad(0.2, -0.2)), None);
        let diagonal = pad(0.8, -0.8);
        assert!(right_stick_down(&diagonal, RIGHT_RIGHT));
        assert!(right_stick_down(&diagonal, RIGHT_UP));
        for (name, key) in RIGHT_KEYS {
            let text = format!("Joystick: {name}");
            assert!(text.len() < 20);
            assert_eq!(right_key_from_text(&text), Some(key));
        }
    }
}

pub const IMPORTS: &[Import] = &[("DirectInputCreateA", 4, direct_input_create)];
