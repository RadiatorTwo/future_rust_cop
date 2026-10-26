//! The game's 3dfx Glide 2.x renderer on OpenGL. The game loads glide2x.dll itself and takes
//! its functions with GetProcAddress (`proc_address`): every name gets a thunk, the decorated
//! name (`_grDrawTriangle@12`) gives the stdcall argument size.
//!
//! The game draws its depth-sorted triangles with Glide's colour and alpha combine units, alpha
//! blending and a chroma key (black texels are not drawn), textures 256x256 ARGB1555 in TMU
//! memory, no depth buffer. We draw into a texture of the Glide resolution (`grSstWinOpen`)
//! and show it at `grBufferSwap`. With filtering on, the chroma key leaves the keyed texels
//! out of the bilinear filter, and their share becomes a soft alpha edge.

use super::gl::{compile, texture};
use super::{arg, log, mem, thunk, thunk_not_implemented};
use crate::rt::{Cpu, EDX};
use glow::HasContext;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// The module handle LoadLibraryA gives for glide2x.dll.
pub const MODULE: u32 = 0x1000_0000;

/// TMU memory reported: 4 MB.
const TMU_RAM: u32 = 4 << 20;

#[derive(Clone, Copy, PartialEq, Debug)]
struct State {
    /// function, factor, local, other, invert
    color: [u32; 5],
    alpha: [u32; 5],
    /// ARGB
    constant: u32,
    /// rgb source and destination factor
    blend: (u32, u32),
    /// TMU start address of the texture, if texturing
    tex: Option<u32>,
    chroma: bool,
    key: u32,
    linear: bool,
    clamp: (bool, bool),
    clip: [i32; 4],
}

struct Texture {
    tex: glow::Texture,
    w: u32,
    h: u32,
}

struct Glide {
    res: (u32, u32),
    fbo: glow::Framebuffer,
    color: glow::Texture,
    prog: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    textures: BTreeMap<u32, Texture>,
    state: State,
    /// vertices waiting to be drawn with `state`: x y r g b a s t
    batch: Vec<f32>,
    batch_mode: u32,
    gamma: f32,
}

thread_local! {
    static GLIDE: RefCell<Option<Glide>> = const { RefCell::new(None) };
}

const VS: &str = r#"
uniform vec2 screen;
layout(location = 0) in vec2 pos;
layout(location = 1) in vec4 col;
layout(location = 2) in vec2 tc;
out vec4 vcol;
noperspective out vec2 st;
void main() {
    vcol = col / 255.0;
    st = tc;
    gl_Position = vec4(pos.x / screen.x * 2.0 - 1.0, 1.0 - pos.y / screen.y * 2.0, 0.0, 1.0);
}
"#;

const FS: &str = r#"
uniform sampler2D tex;
uniform ivec2 tex_size;
uniform ivec2 clamp_st;
uniform int use_tex;
uniform ivec4 cc;      // colour combine: function, factor, local, other
uniform int cc_inv;
uniform ivec4 ac;      // alpha combine
uniform int ac_inv;
uniform vec4 konst;
uniform int chroma;
uniform vec3 key;
uniform int linear_filter;
uniform int edge;      // how the chroma key's soft edge is applied (see `flush`)
in vec4 vcol;
noperspective in vec2 st;
out vec4 color;

vec4 texel(ivec2 p) {
    ivec2 s = tex_size;
    p = ivec2(clamp_st.x != 0 ? clamp(p.x, 0, s.x - 1) : ((p.x % s.x) + s.x) % s.x,
              clamp_st.y != 0 ? clamp(p.y, 0, s.y - 1) : ((p.y % s.y) + s.y) % s.y);
    return texelFetch(tex, p, 0);
}

bool keyed(vec3 c) { return chroma != 0 && all(lessThan(abs(c - key), vec3(0.5 / 255.0))); }

// the texture colour and how much of it is not keyed out
vec4 sample_tex(out float cover) {
    if (linear_filter == 0) {
        vec4 t = texel(ivec2(floor(st)));
        cover = keyed(t.rgb) ? 0.0 : 1.0;
        return t;
    }
    vec2 p = st - 0.5;
    ivec2 i = ivec2(floor(p));
    vec2 f = p - floor(p);
    vec4 sum = vec4(0.0);
    float wsum = 0.0;
    for (int k = 0; k < 4; k++) {
        ivec2 o = ivec2(k & 1, k >> 1);
        vec4 t = texel(i + o);
        float w = (o.x == 1 ? f.x : 1.0 - f.x) * (o.y == 1 ? f.y : 1.0 - f.y);
        if (keyed(t.rgb)) w = 0.0;
        sum += t * w;
        wsum += w;
    }
    cover = wsum;
    return wsum > 0.0 ? sum / wsum : vec4(0.0);
}

vec3 factor3(int f, vec3 local, float local_a, float other_a, float tex_a) {
    if (f == 0) return vec3(0.0);
    if (f == 1) return local;
    if (f == 2) return vec3(other_a);
    if (f == 3) return vec3(local_a);
    if (f == 4) return vec3(tex_a);
    if (f == 8) return vec3(1.0);
    if (f == 9) return 1.0 - local;
    if (f == 10) return vec3(1.0 - other_a);
    if (f == 11) return vec3(1.0 - local_a);
    if (f == 12) return vec3(1.0 - tex_a);
    return vec3(1.0);
}

vec3 combine3(int fn_, vec3 f, vec3 local, float local_a, vec3 other) {
    if (fn_ == 0) return vec3(0.0);
    if (fn_ == 1) return local;
    if (fn_ == 2) return vec3(local_a);
    if (fn_ == 3) return f * other;
    if (fn_ == 4) return f * other + local;
    if (fn_ == 5) return f * other + local_a;
    if (fn_ == 6) return f * (other - local);
    if (fn_ == 7) return f * (other - local) + local;
    if (fn_ == 8) return f * (other - local) + local_a;
    if (fn_ == 9) return -f * local + local;
    if (fn_ == 16) return -f * local + local_a;
    return local;
}

void main() {
    float cover = 1.0;
    vec4 t = use_tex != 0 ? sample_tex(cover) : vec4(1.0);
    // alpha first: colour factors may use it
    float a_local = ac.z == 0 ? vcol.a : konst.a;
    float a_other = ac.w == 0 ? vcol.a : ac.w == 1 ? t.a : konst.a;
    float af = factor3(ac.y, vec3(a_local), a_local, a_other, t.a).x;
    float a = clamp(combine3(ac.x, vec3(af), vec3(a_local), a_local, vec3(a_other)).x, 0.0, 1.0);
    if (ac_inv != 0) a = 1.0 - a;
    vec3 c_local = cc.z == 0 ? vcol.rgb : konst.rgb;
    vec3 c_other = cc.w == 0 ? vcol.rgb : cc.w == 1 ? t.rgb : konst.rgb;
    // the chroma key compares the colour unit's "other" input
    if (cc.w != 1 && keyed(c_other)) discard;
    if (cc.w != 1) cover = 1.0;
    vec3 cf = factor3(cc.y, c_local, a_local, a_other, t.a);
    vec3 c = clamp(combine3(cc.x, cf, c_local, a_local, c_other), 0.0, 1.0);
    if (cc_inv != 0) c = 1.0 - c;
    if (edge == 0 && cover < 0.5) discard;
    if (edge == 1) a = cover;
    if (edge == 2) a *= cover;
    if (edge == 3) c *= cover;
    if (cover <= 0.0) discard;
    color = vec4(c, a);
}
"#;

fn with<R>(f: impl FnOnce(&mut Glide, &glow::Context) -> R) -> Option<R> {
    GLIDE.with(|g| {
        let mut g = g.borrow_mut();
        let g = g.as_mut()?;
        super::window::with_gl(|gl| f(g, &gl.gl))
    })
}

fn f32_arg(c: &Cpu, n: u32) -> f32 {
    f32::from_bits(arg(c, n))
}

/// Size of a texture level: LOD 0 is 256, 8 is 1; aspect 0..6 is 8:1 .. 1:8.
fn lod_size(lod: u32, aspect: u32) -> (u32, u32) {
    let l = 256 >> lod.min(8);
    match aspect {
        0..=3 => (l, (l >> (3 - aspect)).max(1)),
        _ => ((l >> (aspect - 3)).max(1), l),
    }
}

/// Texels of a format as RGBA bytes (alpha 255 where the format has none).
fn convert(format: u32, data: u32, n: usize) -> Vec<u8> {
    let m = mem();
    let e5 = |v: u32| ((v << 3) | (v >> 2)) as u8;
    let e6 = |v: u32| ((v << 2) | (v >> 4)) as u8;
    let e4 = |v: u32| (v * 17) as u8;
    let mut out = Vec::with_capacity(n * 4);
    for k in 0..n as u32 {
        let px: [u8; 4] = match format {
            0xb => {
                let v = m.rd16(data + 2 * k) as u32;
                [e5(v >> 10 & 31), e5(v >> 5 & 31), e5(v & 31), if v & 0x8000 != 0 { 255 } else { 0 }]
            }
            0xa => {
                let v = m.rd16(data + 2 * k) as u32;
                [e5(v >> 11), e6(v >> 5 & 63), e5(v & 31), 255]
            }
            0xc => {
                let v = m.rd16(data + 2 * k) as u32;
                [e4(v >> 8 & 15), e4(v >> 4 & 15), e4(v & 15), e4(v >> 12)]
            }
            0xd => {
                let v = m.rd16(data + 2 * k);
                let i = v as u8;
                [i, i, i, (v >> 8) as u8]
            }
            0 => {
                let v = m.rd8(data + k) as u32;
                [(v >> 5) as u8 * 36, (v >> 2 & 7) as u8 * 36, (v & 3) as u8 * 85, 255]
            }
            2 => [255, 255, 255, m.rd8(data + k)],
            3 => {
                let i = m.rd8(data + k);
                [i, i, i, 255]
            }
            _ => [255, 0, 255, 255],
        };
        out.extend(px);
    }
    out
}

fn bytes_per_texel(format: u32) -> u32 {
    if format >= 8 { 2 } else { 1 }
}

impl Glide {
    fn open(gl: &glow::Context, w: u32, h: u32) -> Glide {
        unsafe {
            let color = texture(gl, true);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            let fbo = gl.create_framebuffer().expect("GL framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(color), 0);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            let prog = compile(gl, VS, FS);
            let vao = gl.create_vertex_array().expect("GL vertex array");
            let vbo = gl.create_buffer().expect("GL buffer");
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            for (loc, n, off) in [(0, 2, 0), (1, 4, 8), (2, 2, 24)] {
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_pointer_f32(loc, n, glow::FLOAT, false, 32, off);
            }
            Glide {
                res: (w, h),
                fbo,
                color,
                prog,
                vao,
                vbo,
                textures: BTreeMap::new(),
                state: State {
                    color: [1, 0, 0, 0, 0],
                    alpha: [1, 0, 0, 0, 0],
                    constant: 0xffff_ffff,
                    blend: (4, 0),
                    tex: None,
                    chroma: false,
                    key: 0,
                    linear: false,
                    clamp: (true, true),
                    clip: [0, 0, w as i32, h as i32],
                },
                batch: Vec::new(),
                batch_mode: glow::TRIANGLES,
                gamma: 1.0,
            }
        }
    }

    /// Draw what is batched with the current state.
    fn flush(&mut self, gl: &glow::Context) {
        if self.batch.is_empty() {
            return;
        }
        let s = self.state;
        let (w, h) = self.res;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, w as i32, h as i32);
            gl.enable(glow::SCISSOR_TEST);
            let c = s.clip;
            gl.scissor(c[0], h as i32 - c[3], (c[2] - c[0]).max(0), (c[3] - c[1]).max(0));
            gl.use_program(Some(self.prog));
            gl.bind_vertex_array(Some(self.vao));
            let u = |n: &str| gl.get_uniform_location(self.prog, n);
            gl.uniform_2_f32(u("screen").as_ref(), w as f32, h as f32);
            let tex = s.tex.and_then(|a| self.textures.get(&a));
            gl.uniform_1_i32(u("use_tex").as_ref(), tex.is_some() as i32);
            gl.active_texture(glow::TEXTURE0);
            gl.uniform_1_i32(u("tex").as_ref(), 0);
            if let Some(t) = tex {
                gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
                gl.uniform_2_i32(u("tex_size").as_ref(), t.w as i32, t.h as i32);
            }
            gl.uniform_2_i32(u("clamp_st").as_ref(), s.clamp.0 as i32, s.clamp.1 as i32);
            gl.uniform_4_i32(u("cc").as_ref(), s.color[0] as i32, s.color[1] as i32, s.color[2] as i32, s.color[3] as i32);
            gl.uniform_1_i32(u("cc_inv").as_ref(), s.color[4] as i32);
            gl.uniform_4_i32(u("ac").as_ref(), s.alpha[0] as i32, s.alpha[1] as i32, s.alpha[2] as i32, s.alpha[3] as i32);
            gl.uniform_1_i32(u("ac_inv").as_ref(), s.alpha[4] as i32);
            let k = s.constant;
            let ch = |sh: u32| (k >> sh & 0xff) as f32 / 255.0;
            gl.uniform_4_f32(u("konst").as_ref(), ch(16), ch(8), ch(0), ch(24));
            gl.uniform_1_i32(u("chroma").as_ref(), s.chroma as i32);
            let key = s.key;
            gl.uniform_3_f32(u("key").as_ref(), (key >> 16 & 0xff) as f32 / 255.0, (key >> 8 & 0xff) as f32 / 255.0, (key & 0xff) as f32 / 255.0);
            gl.uniform_1_i32(u("linear_filter").as_ref(), s.linear as i32);
            // blending: Glide factors 0 zero, 1 src alpha, 2 src/dst colour, 3 dst alpha, 4 one,
            // 5 1 - src alpha, 6 1 - src/dst colour, 7 1 - dst alpha
            let factor = |f: u32, dst: bool| match f {
                0 => glow::ZERO,
                1 => glow::SRC_ALPHA,
                2 => if dst { glow::SRC_COLOR } else { glow::DST_COLOR },
                3 => glow::DST_ALPHA,
                5 => glow::ONE_MINUS_SRC_ALPHA,
                6 => if dst { glow::ONE_MINUS_SRC_COLOR } else { glow::ONE_MINUS_DST_COLOR },
                7 => glow::ONE_MINUS_DST_ALPHA,
                _ => glow::ONE,
            };
            let (src, dst) = (factor(s.blend.0, false), factor(s.blend.1, true));
            let opaque = src == glow::ONE && dst == glow::ZERO;
            let soft = s.chroma && s.linear && tex.is_some();
            // the soft chroma key edge: 0 cut at half, 1 alpha = cover (blending turned on for
            // it), 2 alpha *= cover, 3 rgb *= cover (additive)
            let edge = if !soft {
                0
            } else if opaque {
                1
            } else if src == glow::SRC_ALPHA || dst == glow::ONE_MINUS_SRC_ALPHA {
                2
            } else if src == glow::ONE && dst == glow::ONE {
                3
            } else {
                0
            };
            gl.uniform_1_i32(u("edge").as_ref(), edge);
            if opaque && edge != 1 {
                gl.disable(glow::BLEND);
            } else {
                gl.enable(glow::BLEND);
                if edge == 1 {
                    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                } else {
                    gl.blend_func(src, dst);
                }
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            let bytes = std::slice::from_raw_parts(self.batch.as_ptr() as *const u8, self.batch.len() * 4);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            gl.draw_arrays(self.batch_mode, 0, (self.batch.len() / 8) as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
        }
        self.batch.clear();
    }

    /// Change the state, drawing what was batched with the old one first.
    fn set(&mut self, gl: &glow::Context, f: impl FnOnce(&mut State)) {
        let mut s = self.state;
        f(&mut s);
        if s != self.state {
            self.flush(gl);
            self.state = s;
        }
    }

    /// Add vertices (GrVertex at guest addresses) as primitives of `mode`.
    fn add(&mut self, gl: &glow::Context, mode: u32, verts: &[u32]) {
        if mode != self.batch_mode {
            self.flush(gl);
            self.batch_mode = mode;
        }
        let m = mem();
        let f = |a: u32| f32::from_bits(m.rd32(a));
        for &v in verts {
            // x y z r g b ooz a oow, then tmu 0: sow tow oow
            let oow = f(v + 0x2c);
            let oow = if oow != 0.0 { oow } else { 1.0 };
            self.batch.extend([f(v), f(v + 4), f(v + 0xc), f(v + 0x10), f(v + 0x14), f(v + 0x1c), f(v + 0x24) / oow, f(v + 0x28) / oow]);
        }
    }

    /// Texels into the texture at TMU address `addr` (rows `rows` of its top level, or all).
    fn download(&mut self, gl: &glow::Context, addr: u32, lod: u32, aspect: u32, format: u32, data: u32, rows: Option<(u32, u32)>) {
        self.flush(gl);
        let (w, h) = lod_size(lod, aspect);
        let t = self.textures.entry(addr).or_insert_with(|| Texture { tex: texture(gl, false), w: 0, h: 0 });
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
            if (t.w, t.h) != (w, h) {
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
                (t.w, t.h) = (w, h);
            }
            let (r0, r1) = rows.unwrap_or((0, h - 1));
            let r1 = r1.min(h - 1);
            if r0 > r1 {
                return;
            }
            let n = ((r1 - r0 + 1) * w) as usize;
            let px = convert(format, data, n);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                r0 as i32,
                w as i32,
                (r1 - r0 + 1) as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&px)),
            );
        }
    }
}

// ---- the Glide functions ---------------------------------------------------------------------

fn ret1(_: &mut Cpu) -> u32 {
    1
}
fn ret0(_: &mut Cpu) -> u32 {
    0
}

fn sst_query_boards(c: &mut Cpu) -> u32 {
    mem().wr32(arg(c, 0), 1);
    1
}

fn sst_query_hardware(c: &mut Cpu) -> u32 {
    let p = arg(c, 0);
    let m = mem();
    m.slice(p, 0x40).fill(0);
    // one Voodoo with 6 MB frame buffer memory (the game offers up to 1280x960 for it)
    for (off, v) in [(0, 1), (4, 0), (8, 6), (0xc, 2), (0x10, 1), (0x14, 0), (0x18, 1), (0x1c, TMU_RAM >> 20)] {
        m.wr32(p + off, v);
    }
    1
}

fn sst_win_open(c: &mut Cpu) -> u32 {
    let res = arg(c, 1);
    let (w, h) = match res {
        0 => (320, 200),
        1 => (320, 240),
        2 => (400, 256),
        3 => (512, 384),
        4 => (640, 200),
        5 => (640, 350),
        6 => (640, 400),
        7 => (640, 480),
        8 => (800, 600),
        9 => (960, 720),
        0xa => (856, 480),
        0xb => (512, 256),
        0xc => (1024, 768),
        0xd => (1280, 1024),
        0xe => (1600, 1200),
        _ => (640, 480),
    };
    log!("glide: grSstWinOpen {w}x{h}");
    let g = super::window::with_gl(|gl| Glide::open(&gl.gl, w, h));
    let ok = g.is_some();
    GLIDE.with(|s| *s.borrow_mut() = g);
    ok as u32
}

fn sst_win_close(_: &mut Cpu) -> u32 {
    with(|g, gl| unsafe {
        gl.delete_framebuffer(g.fbo);
        gl.delete_texture(g.color);
        for t in g.textures.values() {
            gl.delete_texture(t.tex);
        }
    });
    GLIDE.with(|s| *s.borrow_mut() = None);
    1
}

fn buffer_clear(c: &mut Cpu) -> u32 {
    let color = arg(c, 0);
    with(|g, gl| {
        g.flush(gl);
        let (w, h) = g.res;
        let k = g.state.clip;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(g.fbo));
            gl.viewport(0, 0, w as i32, h as i32);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(k[0], h as i32 - k[3], (k[2] - k[0]).max(0), (k[3] - k[1]).max(0));
            let ch = |sh: u32| (color >> sh & 0xff) as f32 / 255.0;
            gl.clear_color(ch(16), ch(8), ch(0), 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.disable(glow::SCISSOR_TEST);
        }
    });
    0
}

fn buffer_swap(_: &mut Cpu) -> u32 {
    let shown = with(|g, gl| {
        g.flush(gl);
        (g.color, g.res, g.gamma)
    });
    if let Some((tex, (w, h), gamma)) = shown {
        super::window::show_texture(tex, w, h, gamma);
    }
    0
}

fn draw_triangle(c: &mut Cpu) -> u32 {
    let v = [arg(c, 0), arg(c, 1), arg(c, 2)];
    with(|g, gl| g.add(gl, glow::TRIANGLES, &v));
    0
}

fn draw_line(c: &mut Cpu) -> u32 {
    let v = [arg(c, 0), arg(c, 1)];
    with(|g, gl| g.add(gl, glow::LINES, &v));
    0
}

fn draw_point(c: &mut Cpu) -> u32 {
    let v = [arg(c, 0)];
    with(|g, gl| g.add(gl, glow::POINTS, &v));
    0
}

fn clip_window(c: &mut Cpu) -> u32 {
    let r = [arg(c, 0) as i32, arg(c, 1) as i32, arg(c, 2) as i32, arg(c, 3) as i32];
    with(|g, gl| g.set(gl, |s| s.clip = r));
    0
}

fn combine(c: &Cpu) -> [u32; 5] {
    [arg(c, 0), arg(c, 1), arg(c, 2), arg(c, 3), arg(c, 4)]
}

fn color_combine(c: &mut Cpu) -> u32 {
    let v = combine(c);
    with(|g, gl| g.set(gl, |s| s.color = v));
    0
}

fn alpha_combine(c: &mut Cpu) -> u32 {
    let v = combine(c);
    with(|g, gl| g.set(gl, |s| s.alpha = v));
    0
}

fn tex_combine(c: &mut Cpu) -> u32 {
    // grTexCombine(tmu, rgb function, factor, alpha function, factor, rgb invert, alpha invert):
    // the game passes the texture through (LOCAL, NONE)
    let (rgb, alpha) = (arg(c, 1), arg(c, 3));
    if rgb != 1 || alpha != 1 {
        log!("warn: grTexCombine({rgb}, {alpha}) taken as LOCAL");
    }
    0
}

fn constant_color(c: &mut Cpu) -> u32 {
    let v = arg(c, 0);
    with(|g, gl| g.set(gl, |s| s.constant = v));
    0
}

fn alpha_blend(c: &mut Cpu) -> u32 {
    let b = (arg(c, 0), arg(c, 1));
    with(|g, gl| g.set(gl, |s| s.blend = b));
    0
}

fn chromakey_mode(c: &mut Cpu) -> u32 {
    let on = arg(c, 0) != 0;
    with(|g, gl| g.set(gl, |s| s.chroma = on));
    0
}

fn chromakey_value(c: &mut Cpu) -> u32 {
    let k = arg(c, 0);
    with(|g, gl| g.set(gl, |s| s.key = k));
    0
}

fn tex_filter_mode(c: &mut Cpu) -> u32 {
    let linear = arg(c, 2) != 0;
    with(|g, gl| g.set(gl, |s| s.linear = linear));
    0
}

fn tex_clamp_mode(c: &mut Cpu) -> u32 {
    let cl = (arg(c, 1) != 0, arg(c, 2) != 0);
    with(|g, gl| g.set(gl, |s| s.clamp = cl));
    0
}

fn gamma_value(c: &mut Cpu) -> u32 {
    let v = f32_arg(c, 0);
    with(|g, _| g.gamma = if v > 0.0 { v } else { 1.0 });
    0
}

fn tex_min_address(_: &mut Cpu) -> u32 {
    0
}

fn tex_max_address(_: &mut Cpu) -> u32 {
    // the last start address a 256x256 16-bit texture fits at
    TMU_RAM - 0x2_0000
}

/// GrTexInfo: smallLod, largeLod, aspect, format, data
fn tex_info(p: u32) -> (u32, u32, u32, u32, u32) {
    let m = mem();
    (m.rd32(p), m.rd32(p + 4), m.rd32(p + 8), m.rd32(p + 12), m.rd32(p + 16))
}

fn tex_mem_required(info: (u32, u32, u32, u32, u32)) -> u32 {
    let (small, large, aspect, format, _) = info;
    (large..=small.max(large)).map(|l| {
        let (w, h) = lod_size(l, aspect);
        w * h * bytes_per_texel(format)
    }).sum::<u32>().next_multiple_of(8)
}

fn tex_texture_mem_required(c: &mut Cpu) -> u32 {
    tex_mem_required(tex_info(arg(c, 1)))
}

fn tex_calc_mem_required(c: &mut Cpu) -> u32 {
    tex_mem_required((arg(c, 0), arg(c, 1), arg(c, 2), arg(c, 3), 0))
}

fn tex_download_mipmap(c: &mut Cpu) -> u32 {
    let (addr, info) = (arg(c, 1), tex_info(arg(c, 3)));
    let (_, large, aspect, format, data) = info;
    with(|g, gl| g.download(gl, addr, large, aspect, format, data, None));
    0
}

fn tex_download_mipmap_level(c: &mut Cpu) -> u32 {
    let (addr, this, large, aspect, format, data) = (arg(c, 1), arg(c, 2), arg(c, 3), arg(c, 4), arg(c, 5), arg(c, 7));
    if this == large {
        with(|g, gl| g.download(gl, addr, this, aspect, format, data, None));
    }
    0
}

fn tex_download_mipmap_level_partial(c: &mut Cpu) -> u32 {
    let (addr, this, large, aspect, format, data, r0, r1) = (arg(c, 1), arg(c, 2), arg(c, 3), arg(c, 4), arg(c, 5), arg(c, 7), arg(c, 8), arg(c, 9));
    if this == large {
        // `data` points at the first row downloaded
        with(|g, gl| g.download(gl, addr, this, aspect, format, data, Some((r0, r1))));
    }
    0
}

fn tex_source(c: &mut Cpu) -> u32 {
    let addr = arg(c, 1);
    with(|g, gl| g.set(gl, |s| s.tex = Some(addr)));
    0
}

fn sst_screen_width(c: &mut Cpu) -> u32 {
    let w = with(|g, _| g.res.0).unwrap_or(640);
    c.r[EDX] = 0;
    w
}

fn sst_screen_height(_: &mut Cpu) -> u32 {
    with(|g, _| g.res.1).unwrap_or(480)
}

/// The Glide functions we implement; anything else the game asks for gets `not_implemented`.
const FUNCTIONS: &[(&str, fn(&mut Cpu) -> u32)] = &[
    ("grGlideInit", ret0),
    ("grGlideShutdown", ret0),
    ("grSstQueryBoards", sst_query_boards),
    ("grSstQueryHardware", sst_query_hardware),
    ("grSstSelect", ret0),
    ("grSstWinOpen", sst_win_open),
    ("grSstWinClose", sst_win_close),
    ("grSstIdle", ret0),
    ("grSstControl", ret1),
    ("grSstScreenWidth", sst_screen_width),
    ("grSstScreenHeight", sst_screen_height),
    ("grBufferClear", buffer_clear),
    ("grBufferSwap", buffer_swap),
    ("grDrawTriangle", draw_triangle),
    ("guDrawTriangleWithClip", draw_triangle),
    ("grDrawLine", draw_line),
    ("grDrawPoint", draw_point),
    ("grClipWindow", clip_window),
    ("grColorCombine", color_combine),
    ("grAlphaCombine", alpha_combine),
    ("grTexCombine", tex_combine),
    ("grConstantColorValue", constant_color),
    ("grAlphaBlendFunction", alpha_blend),
    ("grChromakeyMode", chromakey_mode),
    ("grChromakeyValue", chromakey_value),
    ("grTexFilterMode", tex_filter_mode),
    ("grTexClampMode", tex_clamp_mode),
    ("grTexMipMapMode", ret0),
    ("grDepthBufferMode", ret0),
    ("grDitherMode", ret0),
    ("grGammaCorrectionValue", gamma_value),
    ("grTexMinAddress", tex_min_address),
    ("grTexMaxAddress", tex_max_address),
    ("grTexTextureMemRequired", tex_texture_mem_required),
    ("grTexCalcMemRequired", tex_calc_mem_required),
    ("grTexDownloadMipMap", tex_download_mipmap),
    ("grTexDownloadMipMapLevel", tex_download_mipmap_level),
    ("grTexDownloadMipMapLevelPartial", tex_download_mipmap_level_partial),
    ("grTexSource", tex_source),
];

/// GetProcAddress on glide2x.dll: a thunk for the name (`_grName@argbytes`).
pub fn proc_address(name: &str) -> u32 {
    let plain = name.trim_start_matches('_');
    let (base, bytes) = plain.split_once('@').map_or((plain, 0), |(b, n)| (b, n.parse().unwrap_or(0)));
    let name = format!("glide2x!{base}");
    match FUNCTIONS.iter().find(|e| e.0 == base) {
        Some(&(_, f)) => thunk(&name, (bytes / 4) as i32, f),
        None => thunk_not_implemented(&name, (bytes / 4) as i32),
    }
}
