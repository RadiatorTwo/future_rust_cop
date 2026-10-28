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
      be bound (joy_capture hooked). Open: menus and dialogs by pad (now mouse/keyboard only)
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
      all 27 files decode like ffmpeg; all videos checked by the user (2026-10-27)
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
- [ ] Replacement textures: a switch in Preferences; real HD textures (an AI upscale of the
      dumps, or remade by hand)

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
