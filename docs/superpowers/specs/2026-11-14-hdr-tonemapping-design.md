# HDR rendering and tone mapping (Glide) – design

Agreed with the user on 2026-11-14.

## Intent

Light above white must not burn out any more. Texture Coloring (light up to 2x a texture's own
brightness: lamps, muzzle flashes), additive effects (explosions, glows) and later lighting
keep their colour and gradient instead of being clipped at 1.0 when written. Bloom finally gets
real brightness to work with (PLAN step 9: "Brightness first"). The user wants to compare
several looks in the game.

## Goals

- The 3D picture rendered into a floating point target (RGBA16F), values above 1.0 kept.
- A tone mapping pass that brings it back into 0..1 before the HUD is drawn.
- Several tone mapping curves, chosen in Advanced Graphics.
- Works on every monitor: the output stays an ordinary 8-bit (SDR) picture.

## Non-goals

- HDR output for HDR monitors (HDR10 / scRGB swap chains). The user: "careful with monitors
  without HDR" – the window's surface and its 8-bit output stay as they are.
- Eye adaptation (auto exposure), an exposure slider.
- "Real lighting" beyond Texture Coloring's 2x (the user: "maybe, with HDR, later").
- The software renderer (Glide only).

## Anti-goals

- With tone mapping Off, any change to the picture or a cost: Off must be exactly today's
  path (RGBA8 target, no extra pass).
- A tone mapped HUD: the HUD is drawn as today, after the pass.
- A picture that only looks right on HDR monitors.

## Decisions (the user's)

- Selection box "Tonemapping" in Advanced Graphics: Off (default), Soft, AgX, ACES.
- With any curve but Off, **PlayStation Colors** is off (it only recolours the first level's
  textures; the curve takes over the colours). **Texture Coloring** stays as set (it is
  lighting, not a PlayStation thing, and fills the range above white). **PlayStation
  Shading** (darker, more contrast through a lower gamma) stays as set, unless it turns out to
  disturb.

## Approaches considered

1. Only a soft shoulder (today's picture up to ~0.7, rolled off to white above): keeps the
   look matched to the PS1, HDR works only where information is lost today.
2. AgX: natural, very bright colours bleach gently; mid-tones slightly flatter.
3. ACES (filmic fit): strong contrast, shifts hues and darkens; with PlayStation Shading
   likely too dark.

The user chose all three as options in one selection box, to compare in the game.

## Design

- **Target:** while the curve is not Off, Glide's colour target (`glide.rs`: the resolve
  texture and the multisampled renderbuffer, today RGBA8) is RGBA16F. Made anew when the
  setting changes, like a change of size or MSAA. Everything that reads or writes it (the
  post passes in `glide::post_3d`: SSAO, lights, heat shimmer, bloom; their copies) must
  accept a float target; bloom's copy of the picture follows the target's format.
- **Pass:** a new last post pass at the end of `glide::post_3d` (after bloom, before the HUD):
  reads the view's part of the picture, applies the curve, writes 0..1 back into the target
  inside the view's clip box (split screen: per view). The HUD is then drawn over it as today;
  the gamma at the frame shown (`window`/`gl.rs` blit) stays.
- **Curves** (in linear terms of what the target holds today, i.e. before the shown gamma):
  - Soft: identity up to a knee (~0.7), a smooth shoulder reaching 1.0 at a white point
    (~2.0, Texture Coloring's maximum); hue kept (applied to the largest channel, the others
    scaled with it).
  - AgX: the common AgX base transform (inset matrix, log2 encoding, sigmoid, outset), no look.
  - ACES: the Narkowicz fit.
  The exact constants are the builder's to tune with the user's screenshots.
- **Setting:** REG_DWORD `Tonemap` (0 Off, 1 Soft, 2 AgX, 3 ACES) in the game's key, read at
  start and applied at the dialog's OK from the next frame on; `FCOP_TONEMAP=0..3` overrides it.
  `light.rs`'s PlayStation Colors is treated as off while it is not 0 (the stored setting
  itself is not changed, so Off brings it back).

## Checks

- Off: same picture as before (screenshot pair, byte-identical where the scene is the same).
- Screenshot pairs (Off and each curve) at M2C's start (the wall lamp) and at an explosion.
- Cost: the pass and the float target timed in the code (`FCOP_PHASES` or a timer), at
  1920x1080 with all effects.
- The user looks at it in motion.

## Left to the builder

The curves' constants, where exactly the format switch lives, the shader code, the dialog row's
place in Advanced Graphics, PLAN.md and CLAUDE.md entries.
