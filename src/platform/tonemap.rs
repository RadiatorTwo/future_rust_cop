//! Tone mapping (Glide): with a curve chosen, the 3D picture is drawn into a floating point
//! target (values above 1.0 kept: Texture Coloring's light up to 2x, additive effects) and
//! brought back into 0..1 by the curve before the HUD is drawn. The output stays an ordinary
//! 8-bit picture: any monitor shows it. Off (the default) is the path before: an 8-bit target,
//! no pass. Texture Coloring, PlayStation Colors and PlayStation Shading stay as set (the
//! user, 2026-11-14: PlayStation Colors with tone mapping too).
//!
//! REG_DWORD `Tonemap` in the game's key: 0 Off, 1 Soft (today's picture up to `KNEE`, a soft
//! shoulder above), 2 AgX, 3 ACES; `FCOP_TONEMAP=0..3` overrides it. Chosen in Advanced Graphics.

use super::advapi32;
use glow::HasContext;
use std::sync::atomic::{AtomicU32, Ordering};

/// A tone mapping curve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Curve {
    Off = 0,
    Soft = 1,
    Agx = 2,
    Aces = 3,
}

impl Curve {
    /// The stored value's curve (unknown values: Off).
    pub fn from_value(v: u32) -> Curve {
        match v {
            1 => Curve::Soft,
            2 => Curve::Agx,
            3 => Curve::Aces,
            _ => Curve::Off,
        }
    }
}

/// The chosen curve's value; `u32::MAX` until read.
static CURVE: AtomicU32 = AtomicU32::new(u32::MAX);

pub fn curve() -> Curve {
    let v = CURVE.load(Ordering::Relaxed);
    if v != u32::MAX {
        return Curve::from_value(v);
    }
    let env = std::env::var("FCOP_TONEMAP").ok().and_then(|v| v.parse().ok());
    let stored = || advapi32::value("", "Tonemap").and_then(|(_, d)| Some(u32::from_le_bytes(d.get(..4)?.try_into().ok()?)));
    let c = Curve::from_value(env.or_else(stored).unwrap_or(0));
    CURVE.store(c as u32, Ordering::Relaxed);
    c
}

/// The Advanced Graphics dialog's OK: saved, and used from the next frame on.
pub fn set_curve(c: Curve) {
    CURVE.store(c as u32, Ordering::Relaxed);
    advapi32::set_value("", "Tonemap", 4, &(c as u32).to_le_bytes());
}

/// A curve is chosen: the picture is drawn in HDR.
pub fn hdr() -> bool {
    curve() != Curve::Off
}

/// The curves as Advanced Graphics lists them.
pub const CHOICES: [(Curve, &str); 4] = [(Curve::Off, "Off"), (Curve::Soft, "Soft"), (Curve::Agx, "AgX"), (Curve::Aces, "ACES")];

/// Soft: unchanged up to here.
pub const KNEE: f32 = 0.7;
/// Soft: where the shoulder is (effectively) white: Texture Coloring's brightest.
pub const WHITE: f32 = 2.0;

/// Soft on one value.
pub fn soft(x: f32) -> f32 {
    // an exponential shoulder: slope 1 at the knee, towards 1.0 (0.996 at WHITE), never above
    if x <= KNEE { x } else { KNEE + (1.0 - KNEE) * (1.0 - (-(x - KNEE) / (1.0 - KNEE)).exp()) }
}

/// The pass: the picture's values as the game's colours (what the shown gamma is applied to).
/// Soft works on them directly (unchanged below the knee); AgX and ACES take them as display
/// values, linear by a gamma of 2.2, and give display values back, so mid-tones stay near
/// today's.
const FS: &str = r#"
uniform sampler2D picture;
uniform vec2 size;
uniform int curve;
uniform float knee;
out vec4 o;

vec3 soft(vec3 c) {
    // on the largest channel, the others scaled with it: the hue is kept
    float m = max(max(c.r, c.g), c.b);
    if (m <= knee) return c;
    float s = knee + (1.0 - knee) * (1.0 - exp(-(m - knee) / (1.0 - knee)));
    return c * (s / m);
}

// AgX's base transform (no look): inset, log2 encoding, sigmoid, outset
vec3 agx(vec3 c) {
    const mat3 inset = mat3(0.842479062253094, 0.0423282422610123, 0.0423756549057051,
                            0.0784335999999992, 0.878468636469772, 0.0784336,
                            0.0792237451477643, 0.0791661274605434, 0.879142973793104);
    const mat3 outset = mat3(1.19687900512017, -0.0528968517574562, -0.0529716355144438,
                             -0.0980208811401368, 1.15190312990417, -0.0980434501171241,
                             -0.0990297440797205, -0.0989611768448433, 1.15107367264116);
    const float lo = -12.47393;
    const float hi = 4.026069;
    vec3 x = inset * pow(max(c, 0.0), vec3(2.2));
    x = (clamp(log2(max(x, 1e-10)), lo, hi) - lo) / (hi - lo);
    vec3 x2 = x * x;
    vec3 x4 = x2 * x2;
    x = 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
    return clamp(outset * x, 0.0, 1.0);
}

// ACES, Narkowicz's fit (with his exposure of 0.6)
vec3 aces(vec3 c) {
    vec3 x = pow(max(c, 0.0), vec3(2.2)) * 0.6;
    x = clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
    return pow(x, vec3(1.0 / 2.2));
}

void main() {
    vec4 c = texture(picture, gl_FragCoord.xy / size);
    vec3 r = curve == 1 ? soft(c.rgb) : curve == 2 ? agx(c.rgb) : aces(c.rgb);
    o = vec4(r, c.a);
}
"#;

/// The pass's program and its copy of the picture.
pub struct Pass {
    prog: glow::Program,
    vao: glow::VertexArray,
    /// the copy (size and format, texture, framebuffer)
    copy: Option<(((u32, u32), u32), glow::Texture, glow::Framebuffer)>,
}

impl Pass {
    pub fn new(gl: &glow::Context) -> Pass {
        let vao = unsafe { gl.create_vertex_array().expect("GL vertex array") };
        Pass { prog: super::gl::compile(gl, super::ao::VS, FS), vao, copy: None }
    }

    fn drop_copy(&mut self, gl: &glow::Context) {
        if let Some((_, t, f)) = self.copy.take() {
            unsafe {
                gl.delete_framebuffer(f);
                gl.delete_texture(t);
            }
        }
    }

    pub fn delete(&mut self, gl: &glow::Context) {
        self.drop_copy(gl);
        unsafe {
            super::gl::delete_program(gl, self.prog);
            gl.delete_vertex_array(self.vao);
        }
    }

    /// The view's part (`clip`) of the picture in `out` (`target` its size) through `curve`:
    /// copied (resolved when multisampled), then drawn back mapped.
    pub fn run(&mut self, gl: &glow::Context, target: (u32, u32), out: glow::Framebuffer, clip: [i32; 4], curve: Curve) {
        let (format, kind) = super::glide::colour_format();
        if self.copy.as_ref().is_none_or(|c| c.0 != (target, format)) {
            self.drop_copy(gl);
            unsafe {
                let t = super::gl::texture(gl, false);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, format as i32, target.0 as i32, target.1 as i32, 0, glow::RGBA, kind, glow::PixelUnpackData::Slice(None));
                let f = gl.create_framebuffer().expect("GL framebuffer");
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(t), 0);
                self.copy = Some(((target, format), t, f));
            }
        }
        let Some((_, copy, copy_fbo)) = self.copy else { return };
        let [x, y, w, h] = clip;
        let u = |n: &'static str| super::gl::uniform(gl, self.prog, n);
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(out));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(copy_fbo));
            gl.blit_framebuffer(x, y, x + w, y + h, x, y, x + w, y + h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out));
            gl.viewport(0, 0, target.0 as i32, target.1 as i32);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x, y, w, h);
            gl.use_program(Some(self.prog));
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(copy));
            gl.uniform_1_i32(u("picture").as_ref(), 0);
            gl.uniform_2_f32(u("size").as_ref(), target.0 as f32, target.1 as f32);
            gl.uniform_1_i32(u("curve").as_ref(), curve as i32);
            gl.uniform_1_f32(u("knee").as_ref(), KNEE);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_identity_below_knee() {
        for x in [0.0, 0.25, 0.5, KNEE] {
            assert_eq!(soft(x), x);
        }
    }

    #[test]
    fn soft_white_point() {
        let y = soft(WHITE);
        assert!(y > 0.99 && y <= 1.0, "{y}");
    }

    #[test]
    fn soft_bounded() {
        let mut last = 0.0;
        for k in 0..10_000 {
            let y = soft(k as f32 * 0.01);
            assert!(y >= last && y <= 1.0, "{k}: {y}");
            last = y;
        }
    }

    #[test]
    fn soft_smooth_at_knee() {
        let d = (soft(KNEE + 1e-3) - soft(KNEE)) / 1e-3;
        assert!((d - 1.0).abs() < 0.01, "{d}");
    }

    #[test]
    fn curve_from_value() {
        assert_eq!(Curve::from_value(2), Curve::Agx);
        assert_eq!(Curve::from_value(9), Curve::Off);
    }
}
