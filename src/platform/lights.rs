//! Dynamic lights for Glide missions: what glows lights its surroundings. Muzzle flashes,
//! shots, explosions, the robots' police lights and other glowing effects colour the ground,
//! walls and robots near them, as they come and go.
//!
//! Sources without knowing the game's effects: every additive primitive of the 3D picture
//! (blend ONE or SRC_ALPHA, ONE: the game's glows) is a light (`glide::add` gives `light`): at
//! its centre and depth (`ao.rs`: projected points, or the sprite it belongs to), pulled a
//! little toward the camera (a glow lies on what it lights), its colour the primitive's (the
//! average of its texture rectangle times its vertex or constant colour), its strength and reach
//! by its size in the world. Lights near each other are merged, the strongest `MAX` kept.
//!
//! Applied with the AO when the HUD starts (`glide::post_3d`, after the AO): per pixel the
//! sum of the lights within reach (falling off smoothly to 0 at it, by the angle to the
//! surface's normal from the depth, less at the light's core), the picture times 1 + that sum
//! (it stands in for the surface's colour) plus a little of it (dark asphalt got nearly
//! nothing), not where the mask says something glowing or transparent was drawn.
//!
//! REG_DWORD `DynamicLights` (default off), `FCOP_LIGHTS=0/1`; in the Advanced Graphics
//! dialog. `FCOP_GBUFFER=lights` shows the light alone.

use super::ao;
use super::gl::compile;
use super::light::Setting;
use glow::HasContext;

static SETTING: Setting = Setting::with_default("DynamicLights", "FCOP_LIGHTS", false);

pub fn enabled() -> bool {
    SETTING.get()
}

/// The Advanced Graphics dialog's OK: saved, and used from the next frame on.
pub fn set_enabled(on: bool) {
    SETTING.set(on);
}

/// Lit smoke (PLAN.md 2a step 6): REG_DWORD `LitSmoke` (default on), `FCOP_LIT_SMOKE=0/1`;
/// works only with the dynamic lights.
static SMOKE_SETTING: Setting = Setting::with_default("LitSmoke", "FCOP_LIT_SMOKE", true);

/// The setting alone (the dialog's box).
pub fn smoke_chosen() -> bool {
    SMOKE_SETTING.get()
}

/// Smoke takes the lights: chosen, and the lights on.
pub fn smoke_enabled() -> bool {
    enabled() && SMOKE_SETTING.get()
}

/// The Advanced Graphics dialog's OK.
pub fn set_smoke(on: bool) {
    SMOKE_SETTING.set(on);
}

/// The most lights applied to a picture.
const MAX: usize = 32;
/// The picture times 1 + this much of the light.
const STRENGTH: f32 = 1.6;
/// And plus this much of it, as if every surface were at least this bright.
const ADDED: f32 = 0.12;
/// Additive smoke times 1 + this much of the light reaching it (`glide`'s fragment shader).
pub const SMOKE_STRENGTH: f32 = 2.0;
/// With shadows on: the strongest this many lights cast them.
const SHADOWED: i32 = 8;

/// A light: where (target pixels from the bottom left, z), its colour (0...1, times its
/// strength) and its size in the world (object units).
#[derive(Clone, Copy, Debug)]
pub struct Light {
    pub at: [f32; 3],
    pub colour: [f32; 3],
    pub size: f32,
}

/// A light from a glowing primitive: its corners (target pixels from the bottom left) and z
/// (0 unknown), its colour; `focal` the camera's pixels per unit at z = 1.
pub fn light(corners: &[(f32, f32)], z: &[f32], colour: [f32; 3], focal: f32) -> Option<Light> {
    let zs: Vec<f32> = z.iter().copied().filter(|&z| z > 0.0).collect();
    if zs.is_empty() || colour.iter().all(|&c| c < 0.02) {
        return None;
    }
    let z = zs.iter().sum::<f32>() / zs.len() as f32;
    let n = corners.len() as f32;
    let c = corners.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n));
    // its extent's diagonal in pixels, in the world at its depth
    let (x0, x1) = corners.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = corners.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    let size = ((x1 - x0).hypot(y1 - y0) * z / focal).max(1.0);
    Some(Light { at: [c.0, c.1, z], colour, size })
}

/// The brightest police glow seen (its red, as f32 bits).
static POLICE_PEAK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// A police light's red and blue phases (seconds each).
const POLICE_PHASE: f64 = 0.25;

/// The light colours of a police glow whose red is `red` (0...1). The game draws only red
/// glows at the light bar's ends, blinking offset (one end always dark: a blue of the dark one
/// mixed with the other's red to a constant purple). So the bar flashes on its own clock:
/// `POLICE_PHASE` red, then as long blue (looks darker: more of it, a little red left), as
/// bright as the brightest glow seen, at every glow of the bar.
pub fn police(red: f32) -> Vec<[f32; 3]> {
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    let peak = f32::from_bits(POLICE_PEAK.load(Relaxed)).max(red);
    POLICE_PEAK.store(peak.to_bits(), Relaxed);
    if peak < 0.02 {
        return Vec::new();
    }
    let t = START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64();
    if (t / POLICE_PHASE) as u64 % 2 == 0 {
        vec![[1.0, 0.04, 0.03].map(|k| k * peak)]
    } else {
        vec![[0.03, 0.12, 1.2].map(|k| k * peak), [0.08 * peak, 0.0, 0.0]]
    }
}

/// A police light's glow is small (its triangles ~30 pixels), its light reaches far.
const POLICE_SIZE: f32 = 190.0;

pub fn police_size(l: Light) -> Light {
    Light { size: l.size.max(POLICE_SIZE), ..l }
}

/// A rocket's trail: many small pieces (each nearly no light by its size), merged to a few
/// lights along it; each `share` of the light of one as large as `TRAIL_SIZE`. Its red
/// ribbon has many triangles (0.15 each), its smoke puffs fewer (0.4).
const TRAIL_SIZE: f32 = 150.0;

pub fn trail(l: Light, share: f32) -> Light {
    Light { size: l.size.max(TRAIL_SIZE), colour: l.colour.map(|c| c * share), ..l }
}

/// A light's place in view space (as the shaders' `P`), its reach and colour times strength.
fn view(l: &Light, target: (u32, u32), focal: f32) -> ([f32; 3], f32, [f32; 3]) {
    let reach = (l.size * 4.0).clamp(250.0, 1600.0);
    // pulled toward the camera by a fifth of its reach
    let z = (l.at[2] - reach * 0.2).max(l.at[2] * 0.5);
    let p = [(l.at[0] - target.0 as f32 * 0.5) * z / focal, (l.at[1] - target.1 as f32 * 0.5) * z / focal, z];
    // small ones (sparks) weaker, large ones (explosions) up to 1.2
    let mut k = (l.size / 150.0).clamp(0.25, 1.2);
    // whitish ones (muzzle flashes) weaker: they brightened every channel alike
    let (lo, hi) = l.colour.iter().fold((f32::MAX, 0.0f32), |a, &c| (a.0.min(c), a.1.max(c)));
    if hi > 0.0 {
        k *= 1.0 - 0.45 * (lo / hi).powi(2);
    }
    (p, reach, l.colour.map(|c| c * k))
}

/// Lights near each other and of a like colour as one (summed colour, their place weighed by
/// it), the strongest `MAX` (by colour times reach).
fn merge(lights: &[Light], target: (u32, u32), focal: f32) -> Vec<([f32; 3], f32, [f32; 3])> {
    let mut v: Vec<_> = lights.iter().map(|l| view(l, target, focal)).collect();
    let power = |l: &([f32; 3], f32, [f32; 3])| (l.2[0] + l.2[1] + l.2[2]) * l.1;
    v.sort_by(|a, b| power(b).total_cmp(&power(a)));
    let mut out: Vec<([f32; 3], f32, [f32; 3])> = Vec::new();
    for l in v {
        let near = out.iter().position(|o| {
            let d = (0..3).map(|i| (o.0[i] - l.0[i]).powi(2)).sum::<f32>().sqrt();
            d < 0.4 * o.1.max(l.1) && alike(o.2, l.2)
        });
        match near {
            Some(k) => {
                let o = &mut out[k];
                let (wa, wb) = (o.2.iter().sum::<f32>(), l.2.iter().sum::<f32>());
                let w = wb / (wa + wb).max(1e-6);
                for i in 0..3 {
                    o.0[i] += (l.0[i] - o.0[i]) * w;
                    o.2[i] = (o.2[i] + l.2[i]).min(2.5);
                }
                o.1 = o.1.max(l.1);
            }
            None if out.len() < MAX => out.push(l),
            None => {}
        }
    }
    out
}

/// Two colours of nearly the same hue (police red and blue stay apart).
fn alike(a: [f32; 3], b: [f32; 3]) -> bool {
    let n = |c: [f32; 3]| (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt().max(1e-6);
    (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (n(a) * n(b)) > 0.9
}

const LIGHT_FS: &str = r#"
const int MAX = 32;
uniform int count;
uniform vec4 lpos[MAX];   // view space, reach
uniform vec3 lcol[MAX];
uniform float strength;
uniform int show;         // 0 the light as a factor, 3 the light alone
uniform float added;      // this much of the light is added (dark surfaces get some too)
uniform int shadowed;     // the first this many lights cast shadows (shadows.rs)
out vec4 o;
void main() {
    ivec2 q = ivec2(gl_FragCoord.xy);
    vec2 g = G(q);
    float z = g.r;
    vec3 sum = vec3(0.0);
    if (z > 0.0) {
        vec3 p = P(vec2(q), z);
        vec3 n = N(q, z, p, 1);
        float ign = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
        for (int i = 0; i < count; i++) {
            vec3 l = lpos[i].xyz - p;
            float d = length(l);
            float r = lpos[i].w;
            if (d >= r) continue;
            float a = 1.0 - d / r;
            // less at its core: what the glow sits on (its robot) is not flooded
            a *= a * mix(0.3, 1.0, smoothstep(0.0, 0.2 * r, d));
            // mostly by the angle, a little all round (the normals are rough)
            float ndl = d > 0.0 ? max(dot(n, l / d), 0.0) : 1.0;
            // blocked on its way (not near the light: what holds it)
            if (i < shadowed && d > 0.1 * r) a *= 1.0 - shadow(p, n, l / d, d - 0.08 * r, ign, 16, 1.0);
            sum += lcol[i] * a * (0.2 + 0.8 * ndl);
        }
        sum *= strength * (1.0 - clamp(g.g, 0.0, 1.0));
    }
    o = vec4(sum * added, 1.0);
}
"#;

/// The pass's programs (single-sampled and multisampled depth).
pub struct Lighting {
    progs: [glow::Program; 2],
    vao: glow::VertexArray,
}

impl Lighting {
    pub fn new(gl: &glow::Context) -> Lighting {
        let prog = |ms: bool| compile(gl, ao::VS, &format!("{}{}{}{LIGHT_FS}", if ms { "#define MS\n" } else { "" }, ao::COMMON, super::shadows::GLSL));
        let vao = unsafe { gl.create_vertex_array().expect("GL vertex array") };
        Lighting { progs: [prog(false), prog(true)], vao }
    }

    pub fn delete(&mut self, gl: &glow::Context) {
        unsafe {
            for p in self.progs {
                super::gl::delete_program(gl, p);
            }
            gl.delete_vertex_array(self.vao);
        }
    }

    /// The lights over the 3D picture: `depth` the attachment, `target` its size, `out` a
    /// framebuffer with only the picture's colour, `clip` the scissor box. Gives the merged
    /// lights as the shaders take them (`lpos`, `lcol`), for the smoke of the next picture.
    pub fn run(&mut self, gl: &glow::Context, depth: glow::Texture, ms: bool, target: (u32, u32), out: glow::Framebuffer, clip: [i32; 4], lights: &[Light]) -> (Vec<f32>, Vec<f32>) {
        let show = ao::debug_view();
        if lights.is_empty() && show != 3 || matches!(show, 1 | 2 | 4) {
            return Default::default();
        }
        let (w, h) = (target.0 as i32, target.1 as i32);
        let focal = 4096.0 * h as f32 / 1920.0;
        let merged = merge(lights, target, focal);
        let pos: Vec<f32> = merged.iter().flat_map(|l| [l.0[0], l.0[1], l.0[2], l.1]).collect();
        let col: Vec<f32> = merged.iter().flat_map(|l| l.2).collect();
        unsafe {
            let p = self.progs[ms as usize];
            gl.use_program(Some(p));
            let u = |n: &'static str| super::gl::uniform(gl, p, n);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(if ms { glow::TEXTURE_2D_MULTISAMPLE } else { glow::TEXTURE_2D }, Some(depth));
            gl.uniform_1_i32(u("gdepth").as_ref(), 0);
            gl.uniform_2_i32(u("size").as_ref(), w, h);
            gl.uniform_1_f32(u("focal").as_ref(), focal);
            gl.uniform_1_i32(u("count").as_ref(), merged.len() as i32);
            if !merged.is_empty() {
                gl.uniform_4_f32_slice(u("lpos").as_ref(), &pos);
                gl.uniform_3_f32_slice(u("lcol").as_ref(), &col);
            }
            gl.uniform_1_f32(u("strength").as_ref(), STRENGTH);
            gl.uniform_1_i32(u("show").as_ref(), show);
            gl.uniform_1_f32(u("added").as_ref(), 1.0);
            gl.uniform_1_i32(u("shadowed").as_ref(), if super::shadows::enabled() { SHADOWED } else { 0 });
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out));
            gl.viewport(0, 0, w, h);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(clip[0], clip[1], clip[2], clip[3]);
            if show == 0 {
                // picture * (1 + light)
                gl.enable(glow::BLEND);
                gl.blend_func(glow::DST_COLOR, glow::ONE);
            } else {
                gl.disable(glow::BLEND);
            }
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            if show == 0 {
                // picture + a share of the light
                gl.blend_func(glow::ONE, glow::ONE);
                gl.uniform_1_f32(u("added").as_ref(), ADDED);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        (pos, col)
    }
}
