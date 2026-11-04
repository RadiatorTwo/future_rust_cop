# Plan — what is done, what is open

Updated 2026-10-26. Details on how things work: CLAUDE.md.

## Done

- [x] Finding the code: 2176 functions incl. 147 extra entry points (`src/discover.rs`)
- [x] x86 -> Cranelift for every instruction in the EXE (`src/lift.rs`)
- [x] x87 FPU incl. rounding modes, 24-bit precision (renderer), fild/fistp qword shadow
- [x] MMX moves, port I/O (gameport: nothing connected), push/pop es
- [x] Self-modifying code of the renderer (`src/smc.rs`)
- [x] Events (imports, traps) with a clean stop out of guest code
- [x] Tests without game files (`cargo test --release`)
- [x] Differential test against Unicorn: 1978 of 2176 functions agree, 6 expected
      differences, 189 untested (fault in Unicorn from random states)
- [x] Platform layer, first stage (`src/platform/`): the game starts, runs its C runtime
      start-up, reads the registry, passes the CD check, loads its menu graphics and shows
      the main menu (2026-10-26)

## Open

### 1. Platform layer (in progress)

The game calls Windows and DirectX through its import table (200 imports from 10 DLLs,
`examples/imports.rs`); each has a Rust implementation in `src/platform/`.

Done:
- [x] Process start: EXE loaded, TIB/FS, main stack, guest heap, imports bound to thunks
- [x] kernel32 (C runtime start-up, heap, files, events/mutexes/threads, TLS, time)
- [x] Files and the CD check (volume label FCOP_CD on D:, paths `D:\FCopLAPD\...`)
- [x] Threads take turns (one guest thread at a time, handed over at every import call)
- [x] Registry in `registry.txt` (readable text); default renderer `Group` = Software
- [x] Window, mouse and keyboard (winit), frames shown with softbuffer, scaled with aspect
- [x] DirectDraw 2D: surfaces, Blt/BltFast/Flip/Lock, colour keys, 555 pixels; the menu
      layer (always 640x480 16-bit) works through this
- [x] Main menu shown (`FCOP_SHOT=file.png` writes the shown frame)
- [x] Scripted input `FCOP_INPUT` (clicks, keys at fixed times); START in the menu starts
      the first mission, which runs with the game's software renderer, keypad controls work
- [x] Spin-waits: generated code checks `rt::POLL` at loop heads and hands the turn over
- [x] Sound: DirectSound buffers and PlaySoundA (WAVE resources) mixed on cpal; positions
      advance in real time (the music is streamed); music and effects confirmed by ear
- [x] Dialogs: the game's RT_DIALOG templates run by its own dialog procedures, drawn by us
      as an overlay in the menu's style (Preferences, Audio, Controller Configuration incl.
      rebinding keys through the game's keyboard hook, Load/Save, the pause bar, the quit
      box); `dialog/extend.rs` adds our own settings (first: Renderer in Preferences)

Next:
- [x] Gamepads (`dinput.rs`, 2026-10-27): DirectInput 5 on gilrs, up to 2 pads (player 1/2).
      The game's joystick profile 1 is a modern layout through our button order (RT gun, LT
      heavy, A jump, B special, X action, Y target, LB/RB jink, Back map, R3 camera, Start
      pause), D-pad as buttons 11...14. Checked by the user with an Xbox controller. Fixed after
      that: rebinding needed long presses (presses are kept until read), L3 and higher could not
      be bound (joy_capture hooked). Each assigned stick direction now scales movement, turning,
      and jinking after its dead zone and blends with simultaneous movement (single-player and
      split screen; network input has no magnitude field).
      Right-stick directions can be assigned too and persist in controls.txt.
      Menus and dialogs by pad (issue #8, 2026-11-01, `menupad.rs`): D-pad / left stick go
      from item to item with a gold frame (the cursor hides until the mouse moves), A clicks,
      skips videos; in dialogs left/right move sliders, A opens combo boxes, B is Esc, a
      Controller dialog box takes a pad button only after a click on it (mouse or A), a pad
      save to an unused slot is named "Save <slot>". Checked by the user (menu and dialogs;
      fixed after the first test: OK/Cancel reachable, combo boxes no longer keep left/right,
      the waiting box no longer takes the first move).
      Part 2 (2026-11-01, `gamepad.rs`): pads plugged in while the game runs work (the game
      always sees two pads, the real ones are chosen at every read); the pad has its own
      bindings besides controls.txt (gamepad.txt, default Xbox profile; pad 1 player 1, pad 2
      player 2, a single pad only player 1), set with our "Gamepad" buttons in Preferences and
      the pause bar (the Controller dialog on them, keys not taken). Checked by script (pad
      forward analog and A jump with keyboard bindings; gamepad.txt saved, controls.txt kept).
      Checked by the user. After that the Controller dialog is called "Keyboard": no pad
      input and only the Keyboard profile there (the pad profiles in Gamepad).
- [x] First mission played 90 s by script (walking, turning, all weapons, jump, pause bar):
      no crash; the game keeps running when the window loses the focus
- [x] Presentation through OpenGL (glutin + glow): 2D frames, the dialog overlay, 3D scenes
- [x] Glide 2.x on OpenGL (`glide.rs`, the default renderer; Software stays selectable):
      combine units, blending, chroma key with soft edges when filtered, textures in TMU
      memory, clip window, gamma. THRASH is left out (the game gives it ARGB4444 textures)
- [x] Hooks: a game function can be replaced by a Rust function that calls the original
      (`jit::compile(.., hooks)`, `rt::call_original`, `platform::HOOKS`)
- [x] Glide precise mode (`walker.rs`): vertices in 2560x1920 units instead of rounded
      pixels, frames 4:3 (no 5:4 stretch at 1280x960), HUD text next to its icons
- [x] Widescreen (Hor+) in Glide missions (`wide.rs`, `walker.rs`): the frame takes the
      window's aspect, more world at the sides, HUD groups at the left/centre/right edge,
      target markers in place, dialogs stay 4:3. Tested forced 16:9 and 21:9
      (`FCOP_WIDE=<aspect>`, `FCOP_WIDE=0` off)
- [x] Alt+Enter: borderless fullscreen (checked by the user on a real display, widescreen
      included, 2026-10-26)
- [x] "Widescreen" box in Preferences (3DFX Options, `dialog/extend.rs`): REG_DWORD
      `Widescreen` in the game's key (default on), applies at OK
- [x] Widescreen checked by the user: split screen, fullscreen, fast movement (2026-10-26)
- [ ] PlayStation look in Glide (`light.rs`, 2026-11-04; to be checked by the user):
  - Texture Coloring: light above the texture's own brightness (byte 0x80 = 1.0, up to
    2.0: lamps, flashes) as the software renderer and the PlayStation show it; the game's
    Glide walker (and its THRASH/D3D one, 0x456270) clamped it at 1.0. The lamp's warm pool
    on the wall at the first mission's start comes back; flat RGB (0x30) and untextured
    records left as they are (the game's flat untextured colours wrap above 0x7f)
  - PlayStation Colors (instead of a lowered gamma, which made it darker than the PS1;
    compared with the user's PS1 screenshot and disc): only M2C's textures were recoloured
    for the PC; a fitted colour polynomial (cubic; a matrix dulled saturated colours) gives them the PS1's colours (mean 65, 64, 47 against
    the PS1's 62, 60, 47 at M2C's start; the floor still a little brighter than the PS1's)
  - PlayStation Shading: the software renderer's contrast (the user: it looks right, 3Dfx
    "not remotely"). The game's Glide gamma starts at 1.25; shown 0.25 lower, Glide's blocks
    are the software renderer's brightness in dark and bright places alike. Left: texture
    filtering (block contrast 8.7 against 10.2, 9.8 with the game's Filtering box off). The
    robot's shadow (solid in software, a soft blob in Glide) was not it (user)
- [x] Split screen (Crime War): both players share one health bar and die together: the
      original's design (Precinct Assault has separate bars; checked by the user)
- [ ] Widescreen with the software renderer (not planned yet: Glide is the default)
- [ ] Glide: other resolutions, filtering off, split screen checked with precise mode
- [x] Default keys for player 1 without a keypad (`user32::single_player_keys`): arrows,
      A/D jink, X action, S change target; player 2 keeps the game's keypad profile
- [x] Other missions: all played by the user, they work (2026-10-27)
- [ ] Later: a native Glide backend (a 64-bit glide2x.dll such as dgVoodoo2's), behind the
      same Glide layer
- [x] Videos (`dshow.rs`, `src/mpeg/`): EA logo, intro, briefing play with sound, Space skips;
      all 27 files decode like ffmpeg; all videos checked by the user (2026-10-27); shown
      centred and as large as the window allows with their aspect, not where the game placed
      them (briefings 512x288 high up in 640x480) (2026-10-31)
- [x] Performance at high frame rates: Glide batches across combine/constant/texture
      changes (texture array), the frame wait sleeps; 300 fps hold in the first mission
- [x] High frame rates by interpolated pictures (`fps.rs`): logic at 8 ticks, the render
      block drawn again in between with actors and camera interpolated (one frame late);
      the per-system fixes (weapon, robot physics, hover, camera turn, effects) and the
      frame limit fields are gone. Measured 144 fps, same kills as the original, camera
      moves evenly. Not yet: software renderer
- [x] Frame rate in split screen (2-player co-op on one computer, 2026-10-28): pictures draw
      both views of 0x4076d0, both cameras interpolated. Measured at 144: ~141 fps (one view
      151), both cameras move evenly (128 units a tick in frames and pictures alike).
      Checked by the user: works
- [x] Frame rate: checked by the user by eye, all smooth (2026-10-27)

### 2. The user's goals after that

- [ ] Quit option and preferences menu (controls, graphics, resolution): our own settings
      go into the game's dialogs through `dialog/extend.rs` (renderer, MSAA, resolution, ...)
- [x] High resolution (Glide): missions are drawn as tall as the window (fullscreen: the
      display's resolution), widescreen as wide as its aspect. Tested 1904x984
- [x] Resolution list in Preferences (Glide): "Desktop" and the display's modes instead of
      the game's list (`display.rs`, REG_DWORD `RenderSize`); a window takes the size,
      fullscreen scales it to the display
- [x] "Fullscreen" box in Preferences (Glide; the game hides its own): REG_DWORD
      `Fullscreen`, applies at OK and at start; Alt+Enter saves it too
- [x] Graphics settings during a mission (2026-10-31): "Display" in the pause bar opens
      Preferences without what needs a restart (Renderer) or belongs to the pause bar
      already; all of it applies at once, gamma too (the game set it only at Glide's start)
- [x] High resolution: lines (the laser sight) grow with the resolution (Glide: quads, a
      pixel per 720 lines, 2026-10-27)
- [ ] The software renderer stays at the game's resolution
- [x] MSAA (Glide, 2026-10-27): drawn into a multisampled framebuffer, resolved at
      grBufferSwap; "Anti-alias" in Preferences (Off, 2x, 4x, 8x; REG_DWORD `Msaa`,
      default 4x), `FCOP_MSAA=<n>`. No measurable cost at 8x (~500-650 fps)
- [x] Camera close behind the robot (user, 2026-10-27; `camera.rs`), after the game's
      seven on the camera key; the laser line to the target is left out, the target markers
      stay. Looks good (user). `FCOP_CAMERA=behind`
- [ ] First-person camera: built (`FCOP_CAMERA=first`, robot hidden) but off the camera key:
      "lots of problems" (user), and the game crashed with it twice (2026-10-27): "guest
      memory read at 0x376 in the function at 0x452de0" (the terrain visibility pass) and
      "read at 0x4140a2a in 0x457340 project_points". Probably looking ahead makes the
      visibility pass (0x452880 ff.) reach cells beyond what it was made for (the map's edge,
      far distances). Also to look at: the near plane, the draw distance, eye height
- [x] Cameras: own names in the HUD ("Chase Camera", "First Person Camera"; 2026-10-27)
- [x] Linux (Android later). Runs (2026-10-27, CachyOS VM, Wayland and X11): menu and
      missions fine. Done: SIGSEGV/SIGBUS report like Windows' (`catch_faults`), files not
      needed deleted (no recycle bin), the window kept opaque (0RGB frames have alpha 0, which
      Wayland blended with the desktop). Sound checked. CI builds and releases it
      (0.2.0). Start window: a second process
      `fcop --splash` (checked in the VM). Gamepads checked (Xbox controller,
      wireless dongle with CachyOS' xone-dkms). Network play between Linux and Windows works
      both ways (user, 2026-10-28)
- [ ] Android
- [ ] Texture quality (options in Preferences through `dialog/extend.rs`). The source has
      no more colour than Glide gets: mission files hold each 256x256 page as a `Cbmp`
      resource with `PX16` (16-bit PlayStation 1555 pixels), a 256-entry 16-bit palette
      `PLUT` and a lookup table `LkUp`. Ideas, most useful first:
  - [x] Mipmaps and anisotropic filtering (2026-10-27): textures premultiplied by their
        coverage, filtered by the GPU (trilinear, anisotropic up to 16x); a primitive samples
        inside its texture rectangle, at most at the level where that spans 4 texels (pages
        are atlases). "Textures" in Preferences (REG_DWORD `TextureFilter`). Checked by the
        user in motion (2026-10-27)
  - [x] Debanding: tried and dropped (2026-10-27). A flash3kyuu-style filter (4 samples a
        few texels away, their mean where all are within ~1 step) wiped out the textures'
        fine 1-step grain (grass, ramps) and found no real bands: the game's textures are
        grainy by design, not smooth gradients. Only worth another look where banding is seen
  - [ ] An upscaling filter such as xBRZ (sharper edges than bilinear; a matter of taste)
- [x] Replacement textures (HD textures, 2026-10-28; `hd.rs`): every download (a whole page,
      or a band of rows the game changes later) is known by a hash of its texels; a PNG named
      by it in `game_files/Textures` replaces it: RGBA8, straight alpha as coverage (soft
      edges), up to 4x (all scaled to the largest's scale). Kept in their own texture array
      (32 layers of up to 1024x1024, mipmaps), the rest of a page that has replacements is the
      original scaled up on the GPU; the texel alpha the game blends with stays the original's.
      `FCOP_DUMP_TEXTURES=1` writes the originals to `Textures/dump`, `FCOP_HD=0` off.
      Measured in the first mission: 15 pages at start (all downloaded twice), then only the
      HUD's message icon (rows 208-255 of the page at TMU 0x1a0000, composed anew ~10 times a
      second, rarely the same: replaced only where a band repeats). Tested with all pages
      4x (lanczos, and tinted to see them used). Checked by the user (the robot painted pink)
- [x] Making replacement textures (2026-10-28; `export.rs`, `mission.rs`): the pictures of a
      page are not in the page; edge detection cut signs apart, the rectangles drawn at run
      time are only parts. The mission files tell what uses which part: models (`Cobj`, freely
      mapped sheets: the robot's upper body, M2C model 018, is one picture) and terrain
      (`Ctil`: its tiles take windows anywhere in large painted areas; joining windows that
      share 30% chained over whole pages, 90% gave thousands of windows, so close parts are
      one picture, mostly most of a page). `fcop export` cuts every page into these pictures
      (`pieces/<mission>_page<n>_<hash>/<what>_x_y_wxh.png`, `layout.png`), `fcop import`
      puts changed ones back (any size up to 4x; only changed ones, pictures overlap) into
      `<mission>_page<n>_<hash>.png`; the hash comes from the mission file. Checked: the robot's
      picture 4x and recoloured, imported: recoloured in the game; put back, import removed the
      page. A first try filtered skewed texture coordinates: it dropped 202 of 471 faces of
      the robot's page (user: "the robot's front has no rectangles"); only terrain is filtered
      now (garbage coordinates on tiles drawn in a colour). Upscaling alone is not enough
      (user): the textures are to be made anew
- [x] `<piece>.json` beside each piece (2026-10-28): its faces' corners as texture coordinates
      in the piece (0...1) and, for models, their positions on the model, for an AI to make the
      texture anew (user). Model y is up (the robot's legs, M2C model 019, lie below the origin;
      the world's y is up); a face once, a face a line
- [x] `tools/texview` (2026-10-28): navigator for the pieces' folders (user: hard to find
      one's way in them; the layout stays). Its own crate (egui, own Cargo.lock, not linked to
      fcop): folders | thumbnails | piece with UV grid, original beside it, place on the page,
      JSON; changed = differs from page.png as `fcop import` decides; files polled every second.
      Later perhaps: a 3D view of a model with its pieces, a button for `fcop import`
- [x] Sound export (2026-10-30; `sounds.rs`, `mission::streams`): `fcop export-sounds` writes
      the mission files' sounds to `game_files/Sounds/`, each different one once (1226 in all,
      644 different: every mission carries its own copy, often under another number; named
      `<first mission>_<name>.wav`, `sounds.txt` lists who uses each): effects (`Cwav`, already
      WAV files, 16-bit mono 22050 Hz, `wav<n>.wav`), spoken messages (`snds` after a `SWVR`
      naming the stream: u32 sample count, 8-bit mono 22050 Hz, `1_01_04.wav`) and the music
      (`MSIC` chunks outside the SHOCs: u16 block count, u16 block, u32 frames, 8-bit stereo
      interleaved, 14260 Hz: the game's buffer format, logged). LAX1, M4A1, OV, Un have no
      voices or music. Existing files are kept. No import yet
- [ ] Export: sprites, effects, the HUD (not in models or terrain), the other vertex lists of
      animated models
- [ ] Replacement textures: a switch in Preferences

- [x] German version (2026-11-02): recognised by SHA-256, US addresses moved by
      `data/map-de.txt` (`src/ver.rs`, `examples/vermap.rs`); UDF CD images (`src/iso.rs`).
      Plays like the US one (menu, missions, graphics, sound, input)
- [ ] German version: check our rows in Preferences / pause bar with the longer German
      texts; save games and network play between German and US untested (network likely
      incompatible)

### 2a. Modern graphics effects (Glide only, planned 2026-11-04)

The user's wish: ambient occlusion, dynamic lights, dynamic shadows, more to follow. Each is
a switch in our Advanced Graphics dialog (`extend::template`, button in 3DFX Options; REG_DWORD, an
`FCOP_...` override), off keeps the PlayStation look. The software renderer gets none.

**What the renderer has to work with.** The display list records (`walker.rs`) hold only
screen x/y (s16, 2560x1920 units) and colours; the walker gives every Glide vertex oow = 1.0
(0x439456: textures are affine, as on the PS1). There is no depth per vertex, no normal, no
world position, no depth buffer: the list is sorted in depth buckets, farthest first. Known:
the projection `sx = 1280 + x * 4096 / z` (times k for widescreen, `wide.rs`), its three
functions 0x453740 terrain vertex, 0x457340 object points (in place: {x, y, z, _} s16, z
probably left), 0x457140 sprites (all hooked already), a record's depth key at +0 until the
sort, the camera [0x511f48] (`fps.rs`, up to +0x88), the actors (0x50e428).

#### Step 0: depth and normals (needed by all three)

- [x] Camera-space z per vertex (2026-11-04, `ao.rs`): the simpler way, a table screen (x, y)
      -> z filled by the projection hooks, looked up per Glide vertex (~97% found; clipped
      vertices take the primitive's others; sprites none). Checked with `FCOP_GBUFFER=depth`.
      Original idea: find the code that copies projected points into records
      (callers of 0x457340 / 0x453740, writes at record +0x28...) and keep z beside each
      record: a parallel array indexed by (record - [0x499b0c]) / 0x38, 4 z values. Simpler
      first try, if that is too spread out: the projection hooks fill a table screen (x, y)
      -> z, and the Glide draw looks each vertex up (precise mode passes the record's units
      unchanged); vertices made by clipping or not found take the record's depth key.
      Check: a picture of the depth (`FCOP_GBUFFER=depth` shows it instead of the frame,
      read back with FCOP_SHOT).
- [x] Depth into a second attachment (RG32F: depth, mask; multisampled with MSAA), written by
      opaque and alpha-blended 3D batches (the game's solid geometry is SRC_ALPHA blended with
      alpha 1), the mask by additive / blended ones. No normals stored: the AO takes them from
      the depth. Planned at first: z as a vertex
      attribute; second and third colour attachments of the target (and of the MSAA
      buffer): view-space depth (R32F) and a normal (RG16F, octahedral), written only by
      opaque 3D records (`glColorMaski`, `glEnablei`/`glDisablei(BLEND, i)`, GL 3.0), not by
      blended ones or the HUD. Normals: per triangle on the CPU in `add` (three view-space
      points: x = (sx - 1280) * z / 4096 / k, y likewise), a flat attribute. A fourth
      attachment: the unlit texture colour (albedo, for step 2) and a mask where additive /
      transparent things were drawn (they should not be darkened or lit afterwards).
- [x] The hook point (`glide::post_3d` after the walker's 3D pass, per view in its clip
      window). Planned: the effects are applied to the 3D picture when the HUD starts (the
      walker reaches bucket 0, `walker.rs` draws it in its own calls), once per view (split
      screen: each half by its clip window), in `fps.rs`'s pictures too. A
      `glide::post_3d()` runs the passes on the target, the HUD is drawn over the result.
- [ ] Bonus that comes almost free: with z, perspective-correct textures (an option; the
      affine warping is part of the PS1 look, so off by default)

#### Step 1: ambient occlusion (SSAO)

- [x] Done 2026-11-04 (`ao.rs`): half resolution, 12 samples on a disc (radius 400 object
      units, at most a quarter of the height in pixels), occlusion by how far each rises above
      the surface's plane, depth-aware blur and upsampling, full strength (made stronger after the user saw it: 2026-11-04). Planned: a
      full-screen pass at half resolution: per pixel ~12-16 samples in a hemisphere
      around its normal, compared with the depth texture (radius in world units, ~0.5 of a
      robot's height; fades with distance), then a depth-aware blur (two passes), then
      colour *= mix(1, ao, strength * (1 - mask)) on the full-resolution target.
- [x] Setting: Ambient Occlusion box in Advanced Graphics (REG_DWORD `AmbientOcclusion`,
      default off; later Low / High); `FCOP_SSAO=0/1`; `FCOP_GBUFFER=ao` shows it alone.
- [ ] Check at the M2C start and in a city mission with FCOP_SHOT (on/off pairs), at
      1920x1080 and 4K; cost with FCOP_FPS (aim: well under 1 ms). Done for M2C at 1920x1080:
      contact shadows at walls, under people and robots; ~0.4 ms (965 -> 690 fps unlimited).
      Open: a city mission, 4K, split screen, the user's look (strength, radius); foliage
      (keyed trees) is noisy
- Risks: z wrong where vertices were clipped at the near plane (holes, halos at the screen's
  edge); terrain tiles with the game's baked shading get darker on top of it (strength).

#### Step 2: dynamic lights

- [x] Done 2026-11-04 (`lights.rs`), as planned below; depth of glows that are sprites from
      the sprite projection 0x457140. The robots' police lights now colour the ground, walls and
      robot red/blue, muzzle flashes and rocket trails orange. Planned: sources without reverse
      engineering: every additive Glide primitive of the 3D picture
      (blend ONE, ONE or SRC_ALPHA, ONE: muzzle flashes, explosions, shots, glows) becomes a
      point light at its centre and depth, its colour = average texture colour times vertex
      colour, its strength and radius by its area. Merged when close, the strongest 32-64
      kept. They come and go with the game's effects, fps.rs's pictures included.
- [x] The pass after the AO; no albedo buffer: the picture itself stands in for it (times 1 +
      1.6 L), plus 0.12 L for dark surfaces; no tiles (32 lights at most). Planned: deferred
      lighting pass after SSAO: per pixel colour += albedo * Σ light * atten *
      max(N·L, 0), not where the mask is set; lights sorted into screen tiles if there are
      many. Strength setting.
- [ ] Later, with reverse engineering: lights the game knows itself (the robot's lights in
      its data +0x1c4 per `fps.rs`, street lamps, vehicles' headlights as cones), and a
      flicker for fires.
- [x] Setting: Dynamic Lights box in Advanced Graphics (REG_DWORD `DynamicLights`, default off);
      `FCOP_LIGHTS=0/1`; `FCOP_GBUFFER=lights`. Police lights flash red and blue (the game
      draws only red glows; blue added, own 0.25 s rhythm). Open: the user's look (strength, reach),
      explosions and other missions checked, split screen.

#### Step 3: dynamic shadows

- [x] Done 2026-11-04 (`shadows.rs`): sun shadows in the AO's half-resolution pass (16 steps,
      900 units, thickness max(220, 2% of z), 45% darker, blurred and applied with the AO), the
      strongest 8 dynamic lights blocked in the lights' pass (16 steps, fully, not faded toward the light). Sun toward (0.45, 1,
      -0.3) in the world, turned by the camera's matrix +0x58 (rows right / up / forward,
      16.16), noted per view by the projection hooks. One box "Shadows" (REG_DWORD `Shadows`,
      default off, `FCOP_SHADOWS=0/1`, `FCOP_GBUFFER=shadows`). Checked at the M2C start: the
      robot's shadow, the ramp's and walls' on the ground; 300 fps held at 1920x1080 with AO,
      lights and shadows. Open: the user's look (direction, length, strength), a sun per
      mission, the keyed trees are speckled (as with the AO), split screen.
      Planned: screen-space shadows. Per pixel a short ray march in view space toward the
      light through the depth texture (16 steps, a thickness assumed); shadowed: colour *=
      1 - strength. For a sun (a direction per mission, a table with a default from above;
      the camera's matrix turns it into view space) and for the 2-4 strongest dynamic
      lights of step 2 (explosions throw shadows). Contact shadows under robots, at walls and
      buildings. Limits: no shadows from things off screen or hidden behind others.
- [ ] Later, real shadow maps, two ways to try:
  - the game draws the scene a second time from a camera above the player along the sun
    (as `fps.rs` already reruns the render block, with the same care for random generators,
    visibility leftovers and the display list), its depth is the shadow map; per pixel the
    view-space position goes to world space (the camera's inverse) and into the sun's view.
    Costs a second game render per frame; the visibility pass culls by that camera
  - or our own: terrain (`Ctil`) and models (`Cobj`, placed by the actors' matrix +0x2c and
    position +0x50) drawn by us into a shadow map; independent of the game's renderer, but
    animated models (several vertex lists) and every kind of object have to be right
- [x] The game's own round shadows under objects (blend ZERO, 1 - SRC_ALPHA, one texture)
      are left out of the 3D picture while Shadows is on (`glide::add`, 2026-11-04).
- [ ] Setting: Shadows Off / Contact / Full; `FCOP_SHADOWS=0/1/2` (now a box: Off / Contact).

#### Step 4: knowing the effects (needed by steps 5-9)

- [ ] Which primitive is which effect: smoke, fire, explosion, rocket trail, muzzle flash,
      police light, spark. By the texture's hash (as `hd.rs` hashes every download; the
      pieces seen with `FCOP_DUMP_TEXTURES=1` / `fcop export`), sorted by hand into a small
      table in the code (only hashes, nothing from the EXE), with the blend mode as a
      fallback (additive = glowing, alpha-blended grey = smoke). A flat vertex attribute
      `effect` to the shader.
- [ ] Reverse engineering, as far as needed: the game's particle/effect system (who adds the
      sprite records, 0x457140's callers; the emitter's place, speed, age). Needed for
      step 5's extra particles and for rockets' direction in step 7.
- [ ] `FCOP_GBUFFER=effects` colours each kind, to check the table.

#### Step 5: better particles, soft particles, smoke

- [ ] Soft particles: blended sprites fade where they come near the scene behind them
      (alpha *= clamp((scene z - sprite z) / d, 0, 1), the depth texture of step 0 read in
      the fragment shader; the sprite's own z from 0x457140's hook). No more hard lines where
      smoke and explosions cut into the ground and walls.
- [ ] Smoke: the game's smoke sprites drawn softer and fuller: a noise texture of our own
      (generated, no asset) scrolled over them, the edge from it, a slow turn and growth by
      age (needs step 4's age, else by the sprite's size).
- [ ] More particles: our own extra smoke and sparks at the game's emitters (explosions,
      burning wrecks, rocket trails), simulated in Rust per game frame (`fps.rs` timing:
      by game time, so pictures in between move them evenly), drawn as sprites with the
      depth test of step 0. Count and lifetime by a setting (Off / Low / High).
- Order: sprites are blended in the game's depth order, ours have to go in by their z
  (drawn into the list's buckets, or sorted with the game's blended records at the HUD's
  start).

#### Step 6: lit smoke

- [ ] Smoke and other alpha-blended (not additive) effects take the lights of step 2 in
      their own fragment shader (the light list as a uniform buffer), with no normal:
      light by distance only, plus a little light from behind (forward scattering) so a
      fire or explosion behind a cloud makes its edge glow. Explosions colour it orange,
      police lights blue/red.
- [ ] Police lights as sources: the cars' flashing lights are found by step 4 (texture or
      colour); if the game draws them only as small sprites, their light gets a larger
      radius and a flicker that follows the sprite's own blink.
- [ ] Strongest in dark missions: a light's strength against the picture's brightness
      (the scene's mean, measured on a small mip of the target).

#### Step 7: heat shimmer / distortion

- [ ] A distortion buffer (RG16F, screen offsets) written by the sources: behind rockets
      (along their trail, from step 4; direction from consecutive positions), above fire
      (rising, slow), short rings at explosions (a shock ring growing for ~0.3 s, then a
      fading shimmer). Offsets from a scrolling noise, weaker with distance, zero on the HUD.
- [ ] Pass at the HUD's start: the 3D picture copied, read back at pixel + offset (only
      from things behind the source: depth of step 0 compared, else foreground objects
      bleed into the shimmer).
- [ ] Setting: Heat Distortion Off / On; `FCOP_DISTORT=0/1`.

#### Step 8: height fog

- [ ] Per pixel the world height: the view-space position (step 0) through the camera's
      inverse ([0x511f48]: matrix and position; y is up). Fog density grows below a height
      (per mission: from the terrain's height range, `Ctil`, so low streets and trenches get
      haze and rooftops stay clear), falls off exponentially above, times the distance the
      view ray runs through it (the closed formula for exponential height fog, no ray
      march). Colour per mission (a table; default from the sky / clear colour).
- [ ] In the drawing fragment shader itself (every fragment has its z from step 0), opaque
      and blended alike, not as a post pass: blended things are drawn between the opaque
      ones, a post pass would fog a sprite by the wall behind it. Subtle by default.
- [ ] Setting: Height Fog Off / On (strength later); `FCOP_FOG=0/1`; per-mission table
      checked in every mission with FCOP_SHOT.

#### Step 9: subtle bloom

- [ ] Brightness first: the target in RGBA16F (HDR, Texture Coloring's up to 2x not clipped
      before the end), else bloom has nothing above white to work with.
- [ ] Source: the bright part of the 3D picture (soft threshold) plus the glowing effects
      (step 4's mask, so explosions and police lights bloom, bright walls barely). A mip
      chain down (5-6 levels, 13-tap filter) and back up (tent filter), added at low
      strength; HUD not bloomed. Then the shown frame's gamma as before.
- [ ] Setting: Bloom Off / On; `FCOP_BLOOM=0/1`.

#### Order of the post passes (at the HUD's start)

While drawing (per fragment): soft particles, lit smoke, height fog. At the HUD's start, over
the finished 3D picture (blended effects included, so these use step 0's mask to leave them
alone): SSAO -> dynamic lights -> shadows -> distortion -> bloom. Then the HUD, then the
gamma at the frame shown.

#### Ideas after these (the user: "more to come")

Tone mapping (with step 9's HDR target), FXAA/SMAA as an alternative to MSAA, colour grading,
motion blur from the interpolated actors, sharper sprites (perspective-correct, replacement
textures), wet streets / reflections in dark missions.

### 3. Smaller open points

- [x] Debug cheats (`cheats.rs`, 2026-10-27): `win`, `lose`, `health`, `ammo` by name in
      FCOP_INPUT and on Shift+F1...F4 with `FCOP_CHEATS=1`. Ideas: invulnerable, all missions
      open, mission choice

- [x] Bug (user, 2026-10-27): after a won level the menu was not drawn again (the mission's
      loading bar stayed on it; seen by the user as the pause menu's purple background too).
      The game redraws its menu at WM_PAINT after the reward video ends; we never sent
      WM_PAINT. Now `user32` keeps a dirty flag per window (fixed 2026-10-27)
- [x] Bug (user, 2026-10-27): saves had no name and showed as -EMPTY-: the slot combo box
      (CBS_DROPDOWN) is where the name is typed, and ours could not be typed into. Fixed
      2026-10-27; saving and loading checked by script (progress comes back)
- [x] Bug (issue #6, 2026-11-01): Precinct Assault's score screen in split screen with
      widescreen was cut to player 1's half. It is drawn in game state 0xd by 0x4080a0 and one
      0x40ca10(1) after the views; `walker.rs` gave its HUD the left half's view. In state 0xd
      the HUD now gets the centred 4:3 view. Reproduced with `poke@511fb8=d` in a 2-player
      Precinct Assault (menu: 5000 click 506,47 / 7000 click 506,77 / 9000 click 484,303)
- [x] Bug (user, 2026-11-03): with much text on screen in play the widescreen HUD went 4:3
      (centred): `hud_sides` kept everything centred from 64 glyphs on, meant for the score
      screen. Now only game state 0xd (the score screen) keeps the HUD centred
- [x] Network play (user, 2026-10-27): Host Game and Join Game. `dplay.rs`: IDirectPlay4A
      from CoCreateInstance, sessions found by UDP (broadcast, this computer, or the address
      typed into our field in the Join dialog), messages over TCP through the host. Checked
      2026-10-27 with two instances on one computer: hosting, finding, joining (also by typed
      address), the shared menu (Precinct Assault, 2 players), the mission on both, the host
      back at its dialog when the other leaves
- [x] Network play between two computers: works (user, 2026-10-27). Windows asks to let
      the host through its firewall (TCP and UDP port 47624) the first time it listens
- [x] Network play follows the Frame Rate setting (2026-10-27): `fps.rs` also draws in
      the network loop's waits (0x406b20, the joiner's wait for the host's frame too) and
      knows its two-view frame 0x4076d0; two instances unlimited: ~950 / ~1600 fps instead of 75.
      Pictures are placed by game time (the network game's frames come in pairs, 8+0 / 7+1
      ticks; between frames the client's pictures stood still and felt like 37 fps, user):
      measured, the robot's speed per tick is now even on both sides. A pair's frames do not
      move things in step with their ticks (a 1-tick frame hardly moves them), which showed
      as ghosting on the host, worse at 300 fps (user): only frames 8 ticks after the last
      kept are kept. Measured at 300 fps: robot and camera speed within 1-2% of the
      neighbouring pictures, host and client
- [x] Walls and terrain shook on the host (user, 2026-10-27: static things only, robots and
      vehicles fine): the two-view frame 0x4076d0 runs the visibility pass 0x452880 before the
      render block, with the real camera, then draws with the shown (interpolated) one; its own
      picture now runs the pass again (what it leaves for the logic put back at 0x4114c0)
- [x] The client looked like a low frame rate, blurred (user): in a network game each computer
      shows only its own player's view ([0x511fa0] & 0xf, 0x40ca10 skips the other's display
      list); the client's is view 1. Its pictures drew view 0 (no triangles at all, the last
      frame stayed), and its own frames' view 1 was not interpolated. Now the local view
      (checked by reading the rendered frame back: the client's picture moves every frame)
- [x] A dialog list without items crashed on the arrow keys (clamp(0, -1))
- [x] Network play: "Lost Net Synchronization" (user, 2026-10-27, with the higher frame
      rate). The game compares the random generators' indices; pictures changed the
      generators' tables and the views' centres (0x4a39b8, from the visibility pass), which the
      next frame's logic uses to wake objects (0x42e570), so it depended on when pictures were
      drawn. Both kept now: two instances with movement and screenshots stayed identical for
      2779 frames (actors compared every frame). Before, the joining computer's picture
      alternated between two views after the loss

- [x] Effects volume 0 was still audible (user, 2026-10-27): the game's curve 0x4318e0 went
      only down to -16.7 dB. Hooked: amplitude in proportion, 0 silent (checked by ear)

- [ ] FPU exception bits (status word bits 0-5) are not modelled (0x47c070, 0x469f60)
- [ ] Difftest: Linux support (demand paging uses a Windows exception handler)
- [ ] Difftest: start from a memory snapshot of the running game, so the 189 untested
      functions get tested
- [ ] Difftest: trace mode that finds the first diverging instruction (the old project had one)
- [x] Start time (2026-10-28): functions compiled on every core (5.5 s -> 1.2 s), the machine
      code kept in `game_files/FCopLAPD/compiled.bin` (later starts ~0.13 s)
- [ ] Performance: x87 TOP known at compile time, fewer flag/state stores
