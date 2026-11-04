//! Ambient occlusion (SSAO) for Glide missions: corners, the ground under robots and walls,
//! the gaps between buildings get darker, as light reaches them less.
//!
//! Depth: the game's display list holds only screen positions, so the projections tell it.
//! 0x457340 (object points, in place {sx, sy, z}) and 0x453740 (terrain vertex, out {sx, sy,
//! z / 16}: terrain coordinates are 16 times finer, so both are in object units) are hooked
//! (`wide.rs`) and every projected point's z is kept by its screen position in 2560x1920
//! units (`point`). The walker passes those units on unchanged (precise mode), so the Glide
//! renderer finds each 3D vertex's z by its position (`depths`): two points at the same place
//! (rare) are told apart by the primitive's other vertices; points the game made by clipping
//! at the near plane, and sprites, take the others' or none. Cleared at every frame shown.
//!
//! The renderer writes view-space z into a second colour attachment (RG32F, multisampled with
//! MSAA) for opaque 3D primitives, interpolated as 1/z; blended ones (smoke, glows) write
//! only a mask in its second channel, where the result is not darkened. When the HUD starts
//! (`walker.rs` after the 3D picture, `glide::post_3d`) the passes here run: AO at half
//! resolution (12 samples on a disc of `RADIUS` around each point, normals from the depth's
//! neighbours, occlusion = how far each sample rises above the surface's plane, within the
//! radius), a depth-aware blur in two passes, then the 3D picture times the AO, with depth-aware
//! upsampling, inside the view's clip window (split screen: each view its own).
//! The camera: sx = 1280 + x * 4096 / z (2560x1920 units, widescreen narrowed by k before), so a
//! target pixel is 4096 * height / 1920 pixels per unit at z = 1, in both directions.
//!
//! REG_DWORD `AmbientOcclusion` (default off), `FCOP_SSAO=0/1`; in the Advanced Graphics
//! dialog. `FCOP_GBUFFER=depth` or `=ao` shows the depth or the AO alone instead of the picture.

use super::gl::compile;
use super::light::Setting;
use glow::HasContext;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static SETTING: Setting = Setting::with_default("AmbientOcclusion", "FCOP_SSAO", false);

pub fn enabled() -> bool {
    SETTING.get()
}

/// Something needs the depth (AO, dynamic lights or shadows).
pub fn depth_wanted() -> bool {
    enabled() || super::lights::enabled() || super::shadows::enabled()
}

/// The Advanced Graphics dialog's OK: saved, and used from the next frame on.
pub fn set_enabled(on: bool) {
    SETTING.set(on);
}

/// `FCOP_GBUFFER`: 0 the picture, 1 the depth, 2 the AO (with the sun's shadow), 3 the dynamic
/// lights, 4 the sun's shadow alone.
pub(super) fn debug_view() -> i32 {
    static V: OnceLock<i32> = OnceLock::new();
    *V.get_or_init(|| match std::env::var("FCOP_GBUFFER").as_deref() {
        Ok("depth") => 1,
        Ok("ao") => 2,
        Ok("lights") => 3,
        Ok("shadows") => 4,
        _ => 0,
    })
}

/// The projected points of this frame: screen position (x | y << 16, 2560x1920 units) ->
/// up to two z (0: none).
static POINTS: Mutex<Option<HashMap<u32, [i16; 2]>>> = Mutex::new(None);

fn key(x: i16, y: i16) -> u32 {
    x as u16 as u32 | (y as u16 as u32) << 16
}

/// A point the game projected to (x, y) at depth z.
pub fn point(x: i16, y: i16, z: i16) {
    if z <= 0 {
        return;
    }
    let mut p = POINTS.lock().unwrap();
    let e = p.get_or_insert_with(HashMap::new).entry(key(x, y)).or_insert([0, 0]);
    if e[0] == 0 || e[0] == z {
        e[0] = z;
    } else if e[1] == 0 {
        e[1] = z;
    }
}

/// The projected sprites of this frame: their rectangle (x0, y0, x1, y1 in 2560x1920 units)
/// and z. Their corners are not projected points.
static SPRITES: Mutex<Vec<([i16; 4], i16)>> = Mutex::new(Vec::new());

/// A sprite the game projected to the centre (x, y), half size (w, h), at depth z.
pub fn sprite(x: i16, y: i16, w: i16, h: i16, z: i16) {
    if z <= 0 {
        return;
    }
    let (w, h) = (w.saturating_abs().max(1), h.saturating_abs().max(1));
    SPRITES.lock().unwrap().push(([x.saturating_sub(w), y.saturating_sub(h), x.saturating_add(w), y.saturating_add(h)], z));
}

/// The z of the nearest sprite whose rectangle holds (x, y).
fn sprite_at(x: f32, y: f32) -> Option<i16> {
    let (x, y) = (x as i32, y as i32);
    SPRITES
        .lock()
        .unwrap()
        .iter()
        .filter(|(r, _)| (r[0] as i32..=r[2] as i32).contains(&x) && (r[1] as i32..=r[3] as i32).contains(&y))
        .map(|(_, z)| *z)
        .min()
}

/// The z of the projected point nearest to (x, y) within `reach` units (a glow the game
/// placed without projecting a point or a sprite there: a rocket's smoke trail).
pub fn depth_near(x: f32, y: f32, reach: f32) -> Option<f32> {
    let p = POINTS.lock().unwrap();
    p.as_ref()?
        .iter()
        .map(|(&k, z)| ((k as u16 as i16 as f32 - x).hypot((k >> 16) as u16 as i16 as f32 - y), z[0]))
        .filter(|&(d, _)| d <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, z)| z as f32)
}

/// At every frame shown: the next frame projects anew.
pub fn clear() {
    if let Some(p) = POINTS.lock().unwrap().as_mut() {
        p.clear();
    }
    SPRITES.lock().unwrap().clear();
}

/// The z of a primitive's vertices (x, y in 2560x1920 units), 0 where none is known.
pub fn depths(verts: &[(f32, f32)]) -> Vec<f32> {
    let p = POINTS.lock().unwrap();
    let Some(p) = p.as_ref() else { return vec![0.0; verts.len()] };
    let found: Vec<[i16; 2]> = verts.iter().map(|&(x, y)| p.get(&key(x.round() as i16, y.round() as i16)).copied().unwrap_or([0, 0])).collect();
    // a reference: the vertices with one candidate (their median), else any candidate
    let mut sure: Vec<i16> = found.iter().filter(|f| f[0] != 0 && f[1] == 0).map(|f| f[0]).collect();
    sure.sort_unstable();
    let reference = sure.get(sure.len() / 2).copied().or_else(|| found.iter().find(|f| f[0] != 0).map(|f| f[0]));
    // none of its corners is a projected point: a sprite's, by its centre
    let reference = reference.or_else(|| {
        let n = verts.len().max(1) as f32;
        let (cx, cy) = verts.iter().fold((0.0, 0.0), |a, v| (a.0 + v.0 / n, a.1 + v.1 / n));
        sprite_at(cx, cy)
    });
    let Some(r) = reference else { return vec![0.0; verts.len()] };
    found
        .iter()
        .map(|f| match *f {
            [0, _] => r,
            [a, 0] => a,
            [a, b] => if (a - r).abs() <= (b - r).abs() { a } else { b },
        } as f32)
        .collect()
}

/// The occlusion's reach in object units, and how strong it darkens (0...1).
const RADIUS: f32 = 400.0;
const STRENGTH: f32 = 1.0;

pub(super) const VS: &str = r#"
void main() {
    gl_Position = vec4(float((gl_VertexID & 1) * 4 - 1), float((gl_VertexID >> 1) * 4 - 1), 0.0, 1.0);
}
"#;

/// Shared by the passes: the depth attachment (multisampled: sample 0) and view positions.
pub(super) const COMMON: &str = r#"
#ifdef MS
uniform sampler2DMS gdepth;
#else
uniform sampler2D gdepth;
#endif
uniform ivec2 size;     // the target in pixels
uniform float focal;    // pixels per unit at z = 1
vec2 G(ivec2 q) {
    return texelFetch(gdepth, clamp(q, ivec2(0), size - 1), 0).rg;
}
vec3 P(vec2 q, float z) {
    return vec3((q - vec2(size) * 0.5) * z / focal, z);
}
// the normal at q (depth z, place p) from the neighbours s pixels away on the side nearer in
// depth (not across an edge), facing the camera
vec3 N(ivec2 q, float z, vec3 p, int s) {
    float zr = G(q + ivec2(s, 0)).r, zl = G(q - ivec2(s, 0)).r;
    float zu = G(q + ivec2(0, s)).r, zd = G(q - ivec2(0, s)).r;
    vec3 dx = (zl <= 0.0 || zr > 0.0 && abs(zr - z) < abs(z - zl)) ? P(vec2(q + ivec2(s, 0)), zr > 0.0 ? zr : z) - p : p - P(vec2(q - ivec2(s, 0)), zl);
    vec3 dy = (zd <= 0.0 || zu > 0.0 && abs(zu - z) < abs(z - zd)) ? P(vec2(q + ivec2(0, s)), zu > 0.0 ? zu : z) - p : p - P(vec2(q - ivec2(0, s)), zd);
    vec3 n = cross(dx, dy);
    n = length(n) > 0.0 ? normalize(n) : vec3(0.0, 0.0, -1.0);
    return dot(n, p) > 0.0 ? -n : n;
}
"#;

const AO_FS: &str = r#"
uniform float radius;
uniform float max_px;
uniform int ao_on;
uniform int sun_on;       // the sun's shadow too (shadows.rs)
uniform vec3 sun;         // toward the sun, view space
uniform float sun_len;
uniform float sun_strength;
out vec4 o;
const int SAMPLES = 12;
void main() {
    ivec2 q = ivec2(gl_FragCoord.xy) * 2;
    float z = G(q).r;
    if (z <= 0.0) { o = vec4(1.0); return; }
    vec3 p = P(vec2(q), z);
    vec3 n = N(q, z, p, 2);
    // a different turn of the samples at every pixel (interleaved gradient noise), blurred after
    float ign = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
    float lit = sun_on != 0 ? 1.0 - sun_strength * shadow(p, n, sun, sun_len, ign, 16, 0.6) : 1.0;
    float rpx = min(radius * focal / z, max_px);
    if (ao_on == 0 || rpx < 2.0) { o = vec4(lit); return; }
    float a = 6.2831853 * ign;
    float occ = 0.0;
    for (int i = 0; i < SAMPLES; i++) {
        float t = (float(i) + 0.5) / float(SAMPLES);
        float ang = a + float(i) * 2.3999632;
        vec2 s = vec2(q) + vec2(cos(ang), sin(ang)) * t * rpx;
        float zs = G(ivec2(s)).r;
        if (zs <= 0.0) continue;
        vec3 v = P(s, zs) - p;
        float d = length(v);
        if (d < 1e-3) continue;
        occ += max(dot(v / d, n) - 0.05, 0.0) * (1.0 - smoothstep(0.5 * radius, radius, d));
    }
    o = vec4(pow(clamp(1.0 - 4.0 * occ / float(SAMPLES), 0.0, 1.0), 2.0) * lit);
}
"#;

const BLUR_FS: &str = r#"
uniform sampler2D ao;
uniform ivec2 dir;
out vec4 o;
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    ivec2 hs = textureSize(ao, 0);
    float z = G(p * 2).r;
    float sum = 0.0, wsum = 0.0;
    for (int k = -4; k <= 4; k++) {
        ivec2 s = clamp(p + dir * k, ivec2(0), hs - 1);
        float zs = G(s * 2).r;
        float w = exp(-float(k * k) / 10.0);
        if (z > 0.0) w *= max(1.0 - abs(zs - z) / (0.04 * z), 0.0);
        sum += texelFetch(ao, s, 0).r * w;
        wsum += w;
    }
    o = vec4(wsum > 0.0 ? sum / wsum : 1.0);
}
"#;

const APPLY_FS: &str = r#"
uniform sampler2D ao;
uniform float strength;
uniform int show;       // 0 the AO as a factor, 1 the depth, 2 the AO alone
out vec4 o;
void main() {
    ivec2 q = ivec2(gl_FragCoord.xy);
    vec2 g = G(q);
    float z = g.r;
    if (show == 1) { o = vec4(vec3(z > 0.0 ? exp(-z / 6000.0) : 0.0), 1.0); return; }
    float v = 1.0;
    if (z > 0.0) {
        // the four nearest half-resolution values, weighed by how near their depth is
        ivec2 hs = textureSize(ao, 0);
        vec2 h = (vec2(q) + 0.5) * 0.5 - 0.5;
        ivec2 b = ivec2(floor(h));
        vec2 f = h - floor(h);
        float sum = 0.0, wsum = 0.0;
        for (int k = 0; k < 4; k++) {
            ivec2 o2 = ivec2(k & 1, k >> 1);
            ivec2 c = clamp(b + o2, ivec2(0), hs - 1);
            float zc = G(c * 2).r;
            float w = (o2.x == 1 ? f.x : 1.0 - f.x) * (o2.y == 1 ? f.y : 1.0 - f.y);
            w *= max(1.0 - abs(zc - z) / (0.04 * z), 0.001);
            sum += texelFetch(ao, c, 0).r * w;
            wsum += w;
        }
        v = sum / wsum;
    }
    if (show == 2) { o = vec4(vec3(v), 1.0); return; }
    o = vec4(vec3(mix(1.0, v, strength * (1.0 - clamp(g.g, 0.0, 1.0)))), 1.0);
}
"#;

/// A half-resolution texture and its framebuffer.
type Half = (glow::Texture, glow::Framebuffer);

/// The passes' programs (single-sampled and multisampled depth), their half-resolution
/// textures and framebuffers.
pub struct Ssao {
    progs: [[glow::Program; 2]; 3],
    vao: glow::VertexArray,
    half: Option<((i32, i32), [Half; 2])>,
}

impl Ssao {
    pub fn new(gl: &glow::Context) -> Ssao {
        let prog = |fs: &str, ms: bool| compile(gl, VS, &format!("{}{COMMON}{}{fs}", if ms { "#define MS\n" } else { "" }, super::shadows::GLSL));
        let progs = [AO_FS, BLUR_FS, APPLY_FS].map(|fs| [prog(fs, false), prog(fs, true)]);
        let vao = unsafe { gl.create_vertex_array().expect("GL vertex array") };
        Ssao { progs, vao, half: None }
    }

    pub fn delete(&mut self, gl: &glow::Context) {
        unsafe {
            for p in self.progs.iter().flatten() {
                gl.delete_program(*p);
            }
            gl.delete_vertex_array(self.vao);
        }
        self.drop_half(gl);
    }

    fn drop_half(&mut self, gl: &glow::Context) {
        if let Some((_, h)) = self.half.take() {
            for (t, f) in h {
                unsafe {
                    gl.delete_framebuffer(f);
                    gl.delete_texture(t);
                }
            }
        }
    }

    fn half(&mut self, gl: &glow::Context, size: (i32, i32)) -> [Half; 2] {
        if self.half.as_ref().is_some_and(|h| h.0 == size) {
            return self.half.as_ref().unwrap().1;
        }
        self.drop_half(gl);
        let make = || unsafe {
            let t = super::gl::texture(gl, false);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R8 as i32, size.0, size.1, 0, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            let f = gl.create_framebuffer().expect("GL framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(t), 0);
            (t, f)
        };
        let h = [make(), make()];
        self.half = Some((size, h));
        h
    }

    /// The passes over the 3D picture: `depth` the attachment (multisampled or not), `target`
    /// its size, `out` a framebuffer with only the picture's colour, `clip` the scissor box
    /// (x, y from below, w, h) the result is limited to; `ao` whether the AO is wanted, `sun`
    /// toward the sun in view space for its shadow.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, gl: &glow::Context, depth: glow::Texture, ms: bool, target: (u32, u32), out: glow::Framebuffer, clip: [i32; 4], ao: bool, sun: Option<[f32; 3]>) {
        let (w, h) = (target.0 as i32, target.1 as i32);
        let hs = ((w + 1) / 2, (h + 1) / 2);
        let half = self.half(gl, hs);
        let focal = 4096.0 * h as f32 / 1920.0;
        let v = ms as usize;
        unsafe {
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(if ms { glow::TEXTURE_2D_MULTISAMPLE } else { glow::TEXTURE_2D }, Some(depth));
            let common = |p: glow::Program| {
                gl.use_program(Some(p));
                let u = move |n: &str| gl.get_uniform_location(p, n);
                gl.uniform_1_i32(u("gdepth").as_ref(), 0);
                gl.uniform_1_i32(u("ao").as_ref(), 1);
                gl.uniform_2_i32(u("size").as_ref(), w, h);
                gl.uniform_1_f32(u("focal").as_ref(), focal);
                u
            };
            // the AO at half resolution
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(half[0].1));
            gl.viewport(0, 0, hs.0, hs.1);
            let u = common(self.progs[0][v]);
            gl.uniform_1_f32(u("radius").as_ref(), RADIUS);
            gl.uniform_1_f32(u("max_px").as_ref(), h as f32 / 4.0);
            gl.uniform_1_i32(u("ao_on").as_ref(), ao as i32);
            gl.uniform_1_i32(u("sun_on").as_ref(), sun.is_some() as i32);
            let s = sun.unwrap_or([0.0, 1.0, 0.0]);
            gl.uniform_3_f32(u("sun").as_ref(), s[0], s[1], s[2]);
            gl.uniform_1_f32(u("sun_len").as_ref(), super::shadows::SUN_LENGTH);
            gl.uniform_1_f32(u("sun_strength").as_ref(), super::shadows::SUN_STRENGTH);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            // blurred across, then down
            gl.active_texture(glow::TEXTURE1);
            for (from, to, dir) in [(0, 1, (1, 0)), (1, 0, (0, 1))] {
                gl.bind_texture(glow::TEXTURE_2D, Some(half[from].0));
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(half[to].1));
                let u = common(self.progs[1][v]);
                gl.uniform_2_i32(u("dir").as_ref(), dir.0, dir.1);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            // the picture times the AO
            gl.bind_texture(glow::TEXTURE_2D, Some(half[0].0));
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out));
            gl.viewport(0, 0, w, h);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(clip[0], clip[1], clip[2], clip[3]);
            let show = match debug_view() {
                4 => 2,
                v => v,
            };
            if show == 0 {
                gl.enable(glow::BLEND);
                gl.blend_func(glow::DST_COLOR, glow::ZERO);
            }
            let u = common(self.progs[2][v]);
            gl.uniform_1_f32(u("strength").as_ref(), STRENGTH);
            gl.uniform_1_i32(u("show").as_ref(), show);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }
}
