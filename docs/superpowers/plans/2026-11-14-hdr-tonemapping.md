# HDR and Tone Mapping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Glide's 3D picture rendered into an RGBA16F target and tone mapped (Off / Soft / AgX / ACES, chosen in Advanced Graphics) before the HUD, output unchanged 8-bit.

**Architecture:** A new module `src/platform/tonemap.rs` holds the setting, a CPU reference of the Soft curve (tested) and the GL pass. `glide.rs` picks the colour target's format from the setting and calls the pass last in `post_3d`. The post passes' own copies follow the target's format. The dialog gets one combo row.

**Tech Stack:** Rust, glow (OpenGL 3.3 core), GLSL 330.

**Spec:** `docs/superpowers/specs/2026-11-14-hdr-tonemapping-design.md`

## Global Constraints

- Registry: REG_DWORD `Tonemap` in the game's key, 0 Off (default), 1 Soft, 2 AgX, 3 ACES; switch `FCOP_TONEMAP=0..3` overrides it.
- Off = today's path exactly: RGBA8 target, no extra pass, no extra copy.
- While not Off: PlayStation Colors counts as off (`light::ps_colors()` false); the stored `PlayStationColors` value is not changed. Texture Coloring and PlayStation Shading unchanged.
- The HUD is not tone mapped; the shown frame's gamma (`light::gamma`, the blit in `gl.rs`) unchanged.
- Output stays SDR 8-bit (window surface untouched). Glide only.
- Every fcop.exe run with `FCOP_STATS=1`. Never run cargo fmt. No AI attribution in commits.

## Review Focus

- Off after having been on (Advanced Graphics during a mission, pause bar "Display"): target must go back to RGBA8 and the picture be today's. Test: Task 2 step (toggle run).
- MSAA Off (samples 0): the target is the texture itself, no renderbuffer; the pass must still read a copy, never the texture it writes. Test: Task 3 run with `FCOP_MSAA=0`.
- Split screen: `post_3d` runs per view; the pass only touches its view's clip box, so the other half is not mapped twice. Test: Task 3 note, checked by the user (split screen is not scriptable here).
- Window resize / resolution change while HDR is on: the target is made anew in the chosen format. Test: Task 2 run resizing via `FCOP_INPUT` is not possible; covered by the format being chosen in the same function that makes the target (code review).
- Additive effects far above 2.0 (explosions stacking): Soft must stay ≤ 1.0 and monotonic for any input. Test: Task 1 unit test `soft_bounded`.

---

### Task 1: Setting and the Soft curve

**Files:**
- Create: `src/platform/tonemap.rs`
- Modify: `src/platform/mod.rs` (`pub mod tonemap;`), `src/platform/light.rs:150` (`ps_colors`)

**Interfaces:**
- Produces: `#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Curve { Off = 0, Soft = 1, Agx = 2, Aces = 3 }`; `pub fn curve() -> Curve`; `pub fn set_curve(c: Curve)` (saves REG_DWORD `Tonemap`, applies from the next frame); `pub fn hdr() -> bool` (`curve() != Curve::Off`); `pub fn soft(x: f32) -> f32`; `pub const KNEE: f32 = 0.7; pub const WHITE: f32 = 2.0;`; `pub const CHOICES: [(Curve, &str); 4] = [(Off,"Off"), (Soft,"Soft"), (Agx,"AgX"), (Aces,"ACES")]`.

- [ ] **Step 1: Write the failing tests** (`#[cfg(test)] mod tests` in `tonemap.rs`)

```rust
#[test] fn soft_identity_below_knee() { for x in [0.0, 0.25, 0.5, KNEE] { assert_eq!(soft(x), x); } }
#[test] fn soft_white_point() { let y = soft(WHITE); assert!(y > 0.99 && y <= 1.0); }
#[test] fn soft_bounded() { let mut last = 0.0; for k in 0..10_000 { let y = soft(k as f32 * 0.01); assert!(y >= last && y <= 1.0); last = y; } }
#[test] fn soft_smooth_at_knee() { let d = (soft(KNEE + 1e-3) - soft(KNEE)) / 1e-3; assert!((d - 1.0).abs() < 0.01); }
#[test] fn curve_from_value() { assert_eq!(Curve::from_value(2), Curve::Agx); assert_eq!(Curve::from_value(9), Curve::Off); }
```

- [ ] **Step 2: Run** `cargo test --release --lib tonemap` — expected: FAIL (module missing).

- [ ] **Step 3: Implement** `Curve::from_value(v: u32) -> Curve` (unknown → Off), `curve`/`set_curve`/`hdr` (an `AtomicU32` loaded once from `advapi32::value("", "Tonemap")`, `FCOP_TONEMAP` first; `set_curve` stores and calls `advapi32::set_value("", "Tonemap", 4, ..)`), and `soft`: identity up to `KNEE`; above it an exponential shoulder with slope 1 at the knee, approaching 1.0 (≈ 0.996 at `WHITE`), never above it:

```rust
pub fn soft(x: f32) -> f32 {
    if x <= KNEE { x } else { KNEE + (1.0 - KNEE) * (1.0 - (-(x - KNEE) / (1.0 - KNEE)).exp()) }
}
```
(`WHITE` is then where the shoulder is effectively white; the spec's "reaching 1.0 at ~2.0".) Then `light::ps_colors()` returns `PS_COLORS.get() && !super::tonemap::hdr()`.

- [ ] **Step 4: Run** `cargo test --release --lib tonemap` — expected: 5 passed. `cargo build --release` without warnings.

- [ ] **Step 5: Commit** `git add src/platform/tonemap.rs src/platform/mod.rs src/platform/light.rs` / "Tone mapping setting (Tonemap, FCOP_TONEMAP) and the Soft curve; PlayStation Colors off while it is on"

### Task 2: Float colour target

**Files:**
- Modify: `src/platform/glide.rs` (target texture ~line 845 `tex_image_2d(.. RGBA8 ..)`, `multisample` ~line 875 `renderbuffer_storage_multisample(.. RGBA8 ..)`, the size check before them), `src/platform/heat.rs:177` (its copy), `src/platform/bloom.rs` (`targets`: the picture's copy `make(.., RGBA8, UNSIGNED_BYTE)`)

**Interfaces:**
- Consumes: `tonemap::hdr()`.
- Produces: `pub fn colour_format() -> (u32, u32)` in `glide.rs`: `(glow::RGBA16F, glow::HALF_FLOAT)` when `tonemap::hdr()`, else `(glow::RGBA8, glow::UNSIGNED_BYTE)`. The `Glide` struct remembers the format its target was made with (`target_hdr: bool`) and remakes target and MSAA buffer when it differs (as it does for a new size); heat's and bloom's copies are made with `colour_format()` and remade when it changes (their size check also compares the format).

- [ ] **Step 1: Implement** the above; Off produces the exact calls of today.
- [ ] **Step 2: Verify Off is unchanged:** two runs `FCOP_TONEMAP=0` vs the build before the task (`git stash` build copied to `fcop_old.exe`), `FCOP_STATS=1 FCOP_NO_VIDEO=1 FCOP_SHOT=<scratch>/off_{t}.png FCOP_INPUT="8000:click@484,303 20000:Space" timeout 26` — the shots at the same `{t}` near 25000 compared; expected: same picture (scripted, standing; if timing shifts frames, compare visually and by mean brightness within 0.5).
- [ ] **Step 3: Verify HDR runs:** `FCOP_TONEMAP=1` (no pass yet: values above 1 are clipped only at the 8-bit window, picture as before), with `FCOP_MSAA=0` and default MSAA; expected: no GL errors or panics in the log, mission reached.
- [ ] **Step 4: Commit** "Glide's colour target RGBA16F while tone mapping is on (copies of the post passes follow)"

### Task 3: The tone mapping pass

**Files:**
- Modify: `src/platform/tonemap.rs` (pass), `src/platform/glide.rs` (`post_3d`: after bloom; `Glide` holds `tonemap: Option<tonemap::Pass>` deleted with the others)

**Interfaces:**
- Consumes: `curve()`, `KNEE`, `WHITE` (uniforms), `super::ao::VS` (full-screen triangle, as bloom), `super::gl::{compile, uniform, texture}`.
- Produces: `pub struct Pass`; `Pass::new(gl) -> Pass`; `Pass::run(&mut self, gl, target: (u32, u32), out: glow::Framebuffer, clip: [i32; 4], curve: Curve)`; `Pass::delete(&mut self, gl)`.

- [ ] **Step 1: Implement** `run` as bloom's first step: blit the view's clip box of `out` into a copy texture of `colour_format()` (resolves MSAA), then draw onto `out` inside the clip with scissor, blending off, a fragment shader with `uniform int curve` — 1 Soft (the CPU formula, applied to the largest channel and the others scaled by the same factor, so hue is kept), 2 AgX (base transform: inset matrix, log2 encoding over [-12.47393, 4.026069] EV, the 6th-order sigmoid polynomial, outset matrix, back to linear), 3 ACES (Narkowicz: `clamp((x*(2.51x+0.03))/(x*(2.43x+0.59)+0.14), 0, 1)`). Alpha written unchanged. In `post_3d`: `if super::tonemap::hdr() { g.tonemap.get_or_insert_with(..).run(gl, target, colour, clip, super::tonemap::curve()) }` after the bloom call.
- [ ] **Step 2: Verify** with `FCOP_SHOT` pairs at M2C's start (the wall lamp, ~25000 ms) for `FCOP_TONEMAP=0,1,2,3`, and once with `FCOP_MSAA=0`: expected — 0 today's picture, 1 nearly today's with the lamp's pool no longer flat white, 2/3 visibly different curves, HUD identical in all four.
- [ ] **Step 3: Measure** the pass with a temporary timer around `run` (as done for bloom, removed afterwards): expected well under 50 µs a picture at 1920x1080; note the value for PLAN.md.
- [ ] **Step 4: Commit** "Tone mapping pass (Soft, AgX, ACES) at the end of the 3D picture, before the HUD"

### Task 4: Advanced Graphics row, docs

**Files:**
- Modify: `src/platform/dialog/extend.rs` (`template` for `ADVANCED`: a label "Tonemapping" and a combo below Perspective at y 94, OK/Cancel and the dialog's height moved down 14; `init`: items from `tonemap::CHOICES`, sel = `curve()`; `ok`: `tonemap::set_curve`), `CLAUDE.md` (the effects list: `tonemap.rs`, `FCOP_TONEMAP`), `PLAN.md` (2a step 9 "Brightness first" done, an entry for tone mapping with the measured cost)

**Interfaces:**
- Consumes: `tonemap::{CHOICES, curve, set_curve, Curve}`; ids `FIRST_OURS + 31` (label) and `+ 32` (combo) — check they are free in `extend.rs` and take the next free ones otherwise.

- [ ] **Step 1: Implement** the row like the MSAA combo (`init` ~line 466, `ok` ~line 558).
- [ ] **Step 2: Verify** with a script: open Preferences → Advanced Graphics (`FCOP_SHOT` of the dialog), the combo shows the stored choice; choose ACES and OK, restart: registry.txt holds `Tonemap` = 3 and the picture is ACES. Then set it back to Off by `FCOP_TONEMAP=0`-free run with the dialog. Expected as described; `cargo test --release` all passed.
- [ ] **Step 3: Commit** "Tonemapping in Advanced Graphics (Off, Soft, AgX, ACES); CLAUDE.md, PLAN"
