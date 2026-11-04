//! Screen-space shadows for Glide missions (PLAN.md 2a step 3, first part): a sun casts short
//! shadows (under robots, at walls and buildings), and the strongest dynamic lights (`lights.rs`)
//! are blocked by what stands between them and a surface.
//!
//! No shadow map: per pixel a short ray march in view space toward the light through the depth
//! attachment (`ao.rs`): a step that lands behind the depth there, by less than an assumed
//! thickness, is in shadow. Only what is on screen and in front casts shadows.
//!
//! The sun: a direction in the world (y up, from above and a little aside), turned into view
//! space by the camera's matrix. 0x434760 builds it at the camera [0x511f48] + 0x58: 3x3 i32,
//! 16.16, its rows the camera's right, up and forward in the world (yaw w: forward (sin w, 0,
//! cos w), right (cos w, 0, -sin w); pitch > 0 looks down). View space = game camera space:
//! x right, y up (0x457340: sy = 960 - y * 4096 / z), z ahead. The projection hooks (`wide.rs`)
//! note the camera of each view as it is drawn (`camera`), `glide::post_3d` takes them in that
//! order (split screen: two).
//!
//! The sun's shadow goes into the AO's half-resolution pass (blurred and applied with it); the
//! lights' shadows into the lights' pass. REG_DWORD `Shadows` (default off), `FCOP_SHADOWS=0/1`;
//! in the Advanced Graphics dialog. `FCOP_GBUFFER=shadows` shows the sun's shadow alone.

use super::light::Setting;
use super::mem;
use crate::ver::Us;
use std::sync::Mutex;

static SETTING: Setting = Setting::with_default("Shadows", "FCOP_SHADOWS", false);

pub fn enabled() -> bool {
    SETTING.get()
}

/// The Advanced Graphics dialog's OK: saved, and used from the next frame on.
pub fn set_enabled(on: bool) {
    SETTING.set(on);
}

const CAMERA: Us = Us(0x511f48);
/// The camera's matrix (rows: right, up, forward in the world; 16.16).
const MATRIX: u32 = 0x58;

/// Toward the sun in the world (y up): high, from the right and behind the start's view.
const SUN: [f32; 3] = [0.45, 1.0, -0.3];
/// How far a shadow reaches (object units), and how dark it is (0...1).
pub const SUN_LENGTH: f32 = 900.0;
pub const SUN_STRENGTH: f32 = 0.45;

/// The cameras of this frame's views, in the order they were first projected with: the
/// camera's address and the sun in its view space.
static VIEWS: Mutex<Vec<(u32, [f32; 3])>> = Mutex::new(Vec::new());
/// The views' post passes so far this frame.
static DONE: Mutex<usize> = Mutex::new(0);

/// A projection hook: the camera in use draws a view.
pub fn camera() {
    let m = mem();
    let cam = m.rd32(CAMERA.at());
    if cam == 0 {
        return;
    }
    let mut v = VIEWS.lock().unwrap();
    if v.iter().any(|l| l.0 == cam) || v.len() >= 4 {
        return;
    }
    let row = |r: u32| (0..3).map(|c| m.rd32(cam + MATRIX + 12 * r + 4 * c) as i32 as f32 / 65536.0).collect::<Vec<_>>();
    let n = (SUN[0] * SUN[0] + SUN[1] * SUN[1] + SUN[2] * SUN[2]).sqrt();
    let s = SUN.map(|c| c / n);
    let d = [0, 1, 2].map(|r| row(r).iter().zip(s).map(|(a, b)| a * b).sum::<f32>());
    v.push((cam, d));
}

/// At every frame shown: the next frame's views come anew.
pub fn clear() {
    VIEWS.lock().unwrap().clear();
    *DONE.lock().unwrap() = 0;
}

/// The post passes of the next view: toward the sun in its view space.
pub fn next_view() -> [f32; 3] {
    let mut done = DONE.lock().unwrap();
    let v = VIEWS.lock().unwrap();
    let d = v.get(*done).or(v.last()).map_or([0.0, 1.0, 0.0], |l| l.1);
    *done += 1;
    d
}

/// GLSL after `ao::COMMON`: how much of the way from p (normal n) toward `dir` (view space,
/// unit) over `len` units is blocked, 0...1; `j` a jitter 0...1.
pub const GLSL: &str = r#"
float shadow(vec3 p, vec3 n, vec3 dir, float len, float j, int steps) {
    float ndl = dot(n, dir);
    // faces turned away are the game's own shading
    if (ndl <= 0.02) return 0.0;
    // off the surface a little (rough depth), more where the ray grazes it
    vec3 o = p + n * (0.004 * p.z + 4.0) + dir * (0.006 * p.z / max(ndl, 0.2));
    // what lies behind a depth by more than this is not blocked by it (thin things)
    float thick = max(220.0, 0.02 * p.z);
    for (int i = 0; i < steps; i++) {
        float f = (float(i) + j) / float(steps);
        float t = len * f * f;
        vec3 s = o + dir * t;
        if (s.z <= 50.0) break;
        vec2 q = s.xy * focal / s.z + vec2(size) * 0.5;
        if (q.x < 0.0 || q.y < 0.0 || q.x >= float(size.x) || q.y >= float(size.y)) break;
        float zs = G(ivec2(q)).r;
        if (zs <= 0.0) continue;
        float dz = s.z - zs;
        if (dz > 0.003 * s.z + 2.0 && dz < thick) {
            // softer toward the shadow's end
            return 1.0 - smoothstep(0.6, 1.0, f);
        }
    }
    return 0.0;
}
"#;
