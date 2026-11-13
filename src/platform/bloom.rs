//! Bloom (PLAN.md 2a step 9): a soft glow around bright things, most around the glowing
//! effects (explosions, muzzle flashes, police lights, lamps).
//!
//! Applied at the HUD's start after the other passes (`glide::post_3d`), so the HUD is not
//! bloomed. The view's picture is copied (resolved with MSAA); a first pass takes its bright
//! part at half resolution: a soft threshold on the brightness, and lower where the depth
//! attachment's mask says an additive effect was drawn (`ao.rs`: G). Then a chain of smaller
//! levels (13-tap filter down), back up (tent filter, each level adding the one below), and
//! the largest level added to the picture at low strength.
//!
//! No HDR target: the picture is 8 bits, so the threshold is below white.
//!
//! REG_DWORD `Bloom` (default off), `FCOP_BLOOM=0/1`; in the Advanced Graphics dialog.
//! `FCOP_GBUFFER=bloom` shows the glow alone.

use super::ao;
use super::gl::compile;
use super::light::Setting;
use glow::HasContext;

static SETTING: Setting = Setting::with_default("Bloom", "FCOP_BLOOM", false);

pub fn enabled() -> bool {
    SETTING.get()
}

/// The Advanced Graphics dialog's OK: saved, and used from the next frame on.
pub fn set_enabled(on: bool) {
    SETTING.set(on);
}

/// Levels below the half-resolution one.
const LEVELS: usize = 6;
/// The brightness where the glow begins, and the knee's width around it.
const THRESHOLD: f32 = 0.72;
const KNEE: f32 = 0.2;
/// What an additive effect's mask lowers the threshold to.
const EFFECT_THRESHOLD: f32 = 0.5;
/// The glow's strength when added (its whole light, as a share of the bright part's).
const STRENGTH: f32 = 0.35;
/// Each larger level counts this much more going up: the glow reaches farther around a
/// bright thing instead of staying on it.
const SPREAD: f32 = 1.45;

const PRE_FS: &str = r#"
uniform sampler2D picture;
uniform float threshold;
uniform float knee;
uniform float effect_threshold;
out vec4 o;
float bright(vec3 c, float t) {
    float b = max(c.r, max(c.g, c.b));
    float s = clamp(b - t + knee, 0.0, 2.0 * knee);
    s = s * s / (4.0 * knee + 1e-4);
    return max(s, b - t) / max(b, 1e-4);
}
void main() {
    // one half-resolution pixel: 2x2 full-resolution pixels, each by its own threshold
    ivec2 q = ivec2(gl_FragCoord.xy) * 2;
    vec3 sum = vec3(0.0);
    for (int i = 0; i < 4; i++) {
        ivec2 p = q + ivec2(i & 1, i >> 1);
        vec3 c = texelFetch(picture, clamp(p, ivec2(0), size - 1), 0).rgb;
        float m = clamp(G(p).g, 0.0, 1.0);
        float t = mix(threshold, effect_threshold, m);
        // weighted by 1 / (1 + brightness): one bright pixel does not flicker as a blob
        float w = 1.0 / (1.0 + max(c.r, max(c.g, c.b)));
        sum += c * bright(c, t) * w;
    }
    o = vec4(sum * 0.25 * 2.0, 1.0);
}
"#;

/// 13 taps (Jimenez 2014): the level above, filtered down to this one.
const DOWN_FS: &str = r#"
uniform sampler2D src;
uniform vec2 texel;     // the source's texel size
out vec4 o;
void main() {
    vec2 uv = gl_FragCoord.xy / vec2(textureSize(src, 0)) * 2.0;
    vec2 t = texel;
    vec3 a = texture(src, uv + t * vec2(-2.0, 2.0)).rgb;
    vec3 b = texture(src, uv + t * vec2(0.0, 2.0)).rgb;
    vec3 c = texture(src, uv + t * vec2(2.0, 2.0)).rgb;
    vec3 d = texture(src, uv + t * vec2(-2.0, 0.0)).rgb;
    vec3 e = texture(src, uv).rgb;
    vec3 f = texture(src, uv + t * vec2(2.0, 0.0)).rgb;
    vec3 g = texture(src, uv + t * vec2(-2.0, -2.0)).rgb;
    vec3 h = texture(src, uv + t * vec2(0.0, -2.0)).rgb;
    vec3 i = texture(src, uv + t * vec2(2.0, -2.0)).rgb;
    vec3 j = texture(src, uv + t * vec2(-1.0, 1.0)).rgb;
    vec3 k = texture(src, uv + t * vec2(1.0, 1.0)).rgb;
    vec3 l = texture(src, uv + t * vec2(-1.0, -1.0)).rgb;
    vec3 m = texture(src, uv + t * vec2(1.0, -1.0)).rgb;
    vec3 r = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    o = vec4(r, 1.0);
}
"#;

/// A 3x3 tent of the level below, added to this one (blending ONE, ONE); `strength` scales.
const UP_FS: &str = r#"
uniform sampler2D src;
uniform vec2 texel;     // the source's texel size
uniform vec2 dst_size;  // this level's size
uniform float strength;
out vec4 o;
void main() {
    vec2 uv = gl_FragCoord.xy / dst_size;
    vec2 t = texel;
    vec3 r = texture(src, uv).rgb * 4.0;
    r += (texture(src, uv + vec2(t.x, 0.0)).rgb + texture(src, uv - vec2(t.x, 0.0)).rgb
        + texture(src, uv + vec2(0.0, t.y)).rgb + texture(src, uv - vec2(0.0, t.y)).rgb) * 2.0;
    r += texture(src, uv + t).rgb + texture(src, uv - t).rgb
        + texture(src, uv + vec2(t.x, -t.y)).rgb + texture(src, uv + vec2(-t.x, t.y)).rgb;
    o = vec4(r / 16.0 * strength, 1.0);
}
"#;

/// The glow alone (`FCOP_GBUFFER=bloom`): the picture replaced.
fn showing() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("FCOP_GBUFFER").as_deref() == Ok("bloom"))
}

/// The passes' programs and textures.
pub struct Bloom {
    /// the first pass, single-sampled and multisampled depth
    pre: [glow::Program; 2],
    down: glow::Program,
    up: glow::Program,
    vao: glow::VertexArray,
    /// for the target's size: the picture's copy, and the levels (half resolution first) with one
    /// framebuffer they are attached to in turn (a framebuffer of their own each: every switch
    /// cost the driver some 7 µs of the game thread)
    copy: Option<((u32, u32), glow::Texture, glow::Framebuffer, Vec<(glow::Texture, (i32, i32))>, glow::Framebuffer)>,
}

impl Bloom {
    pub fn new(gl: &glow::Context) -> Bloom {
        let pre = |ms: bool| compile(gl, ao::VS, &format!("{}{}{PRE_FS}", if ms { "#define MS\n" } else { "" }, ao::COMMON));
        let vao = unsafe { gl.create_vertex_array().expect("GL vertex array") };
        Bloom { pre: [pre(false), pre(true)], down: compile(gl, ao::VS, DOWN_FS), up: compile(gl, ao::VS, UP_FS), vao, copy: None }
    }

    fn drop_targets(&mut self, gl: &glow::Context) {
        if let Some((_, t, f, levels, chain)) = self.copy.take() {
            unsafe {
                gl.delete_framebuffer(f);
                gl.delete_texture(t);
                gl.delete_framebuffer(chain);
                for (t, _) in levels {
                    gl.delete_texture(t);
                }
            }
        }
    }

    pub fn delete(&mut self, gl: &glow::Context) {
        self.drop_targets(gl);
        unsafe {
            for p in self.pre.into_iter().chain([self.down, self.up]) {
                super::gl::delete_program(gl, p);
            }
            gl.delete_vertex_array(self.vao);
        }
    }

    fn targets(&mut self, gl: &glow::Context, target: (u32, u32)) {
        if self.copy.as_ref().is_some_and(|c| c.0 == target) {
            return;
        }
        self.drop_targets(gl);
        unsafe {
            let make = |w: i32, h: i32, format: u32, kind: u32| {
                let t = super::gl::texture(gl, true);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, format as i32, w, h, 0, glow::RGBA, kind, glow::PixelUnpackData::Slice(None));
                let f = gl.create_framebuffer().expect("GL framebuffer");
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(t), 0);
                (t, f)
            };
            let (t, f) = make(target.0 as i32, target.1 as i32, glow::RGBA8, glow::UNSIGNED_BYTE);
            let mut levels = Vec::new();
            let (mut w, mut h) = (target.0 as i32, target.1 as i32);
            for _ in 0..=LEVELS {
                w = (w / 2).max(1);
                h = (h / 2).max(1);
                let (lt, lf) = make(w, h, glow::RGBA16F, glow::HALF_FLOAT);
                gl.delete_framebuffer(lf);
                levels.push((lt, (w, h)));
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            let chain = gl.create_framebuffer().expect("GL framebuffer");
            self.copy = Some((target, t, f, levels, chain));
        }
    }

    /// The glow over the 3D picture: `depth` the attachment, `target` its size, `out` a
    /// framebuffer with only the picture's colour, `clip` the scissor box.
    pub fn run(&mut self, gl: &glow::Context, depth: glow::Texture, ms: bool, target: (u32, u32), out: glow::Framebuffer, clip: [i32; 4]) {
        if ao::debug_view() != 0 {
            return;
        }
        self.targets(gl, target);
        let Some((_, copy, copy_fbo, levels, chain)) = &self.copy else { return };
        // a level as the chain's target
        let target_level = |t: glow::Texture| unsafe {
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(t), 0);
        };
        let (w, h) = (target.0 as i32, target.1 as i32);
        let [x, y, cw, ch] = clip;
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vao));
            // the view's part of the picture, resolved
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(out));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(*copy_fbo));
            gl.blit_framebuffer(x, y, x + cw, y + ch, x, y, x + cw, y + ch, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            // the bright part at half resolution, only inside the view
            let (l0, (w0, h0)) = levels[0];
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(*chain));
            target_level(l0);
            gl.viewport(0, 0, w0, h0);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x / 2, y / 2, cw / 2, ch / 2);
            let p = self.pre[ms as usize];
            gl.use_program(Some(p));
            let u = |p: glow::Program, n: &'static str| super::gl::uniform(gl, p, n);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(if ms { glow::TEXTURE_2D_MULTISAMPLE } else { glow::TEXTURE_2D }, Some(depth));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(*copy));
            gl.uniform_1_i32(u(p, "gdepth").as_ref(), 0);
            gl.uniform_1_i32(u(p, "picture").as_ref(), 1);
            gl.uniform_2_i32(u(p, "size").as_ref(), w, h);
            gl.uniform_1_f32(u(p, "threshold").as_ref(), THRESHOLD);
            gl.uniform_1_f32(u(p, "knee").as_ref(), KNEE);
            gl.uniform_1_f32(u(p, "effect_threshold").as_ref(), EFFECT_THRESHOLD);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.disable(glow::SCISSOR_TEST);
            // down the chain
            gl.use_program(Some(self.down));
            gl.uniform_1_i32(u(self.down, "src").as_ref(), 1);
            for i in 1..levels.len() {
                let (src, (sw, sh)) = levels[i - 1];
                let (dst, (dw, dh)) = levels[i];
                gl.bind_texture(glow::TEXTURE_2D, Some(src));
                target_level(dst);
                gl.viewport(0, 0, dw, dh);
                gl.uniform_2_f32(u(self.down, "texel").as_ref(), 1.0 / sw as f32, 1.0 / sh as f32);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            // back up, each level adding the one below
            gl.use_program(Some(self.up));
            gl.uniform_1_i32(u(self.up, "src").as_ref(), 1);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE);
            for i in (0..levels.len() - 1).rev() {
                let (src, (sw, sh)) = levels[i + 1];
                let (dst, (dw, dh)) = levels[i];
                gl.bind_texture(glow::TEXTURE_2D, Some(src));
                target_level(dst);
                gl.viewport(0, 0, dw, dh);
                gl.uniform_2_f32(u(self.up, "texel").as_ref(), 1.0 / sw as f32, 1.0 / sh as f32);
                gl.uniform_2_f32(u(self.up, "dst_size").as_ref(), dw as f32, dh as f32);
                gl.uniform_1_f32(u(self.up, "strength").as_ref(), SPREAD);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            // onto the picture, inside the view
            gl.bind_texture(glow::TEXTURE_2D, Some(l0));
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out));
            gl.viewport(0, 0, w, h);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x, y, cw, ch);
            if showing() {
                gl.disable(glow::BLEND);
            }
            gl.uniform_2_f32(u(self.up, "texel").as_ref(), 1.0 / w0 as f32, 1.0 / h0 as f32);
            gl.uniform_2_f32(u(self.up, "dst_size").as_ref(), w as f32, h as f32);
            // the levels' sum (weights SPREAD^k) brought back to one, times the strength
            let sum: f32 = (0..=LEVELS).map(|k| SPREAD.powi(k as i32)).sum();
            let k = (LEVELS + 1) as f32 / sum;
            gl.uniform_1_f32(u(self.up, "strength").as_ref(), if showing() { k } else { STRENGTH * k });
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(if ms { glow::TEXTURE_2D_MULTISAMPLE } else { glow::TEXTURE_2D }, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }
}
