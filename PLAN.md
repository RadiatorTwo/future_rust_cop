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
- [ ] DirectInput (now reported as absent; only used for joysticks)
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
- [ ] Other missions
- [ ] Later: a native Glide backend (a 64-bit glide2x.dll such as dgVoodoo2's), behind the
      same Glide layer
- [ ] Videos (MPEG-1 from the CD's Video folder; DirectShow is reported as absent now)
- [ ] Frame limit fields (0x406a7f, 0x406b30, 0x4071aa, 0x407278, 0x407295) as code fields

### 2. The user's goals after that

- [ ] Quit option and preferences menu (controls, graphics, resolution): our own settings
      go into the game's dialogs through `dialog/extend.rs` (renderer, MSAA, resolution, ...)
- [ ] High resolution through the OpenGL renderer (widescreen: done for Glide)
- [ ] Linux, later Android
- [ ] Texture quality (options in Preferences through `dialog/extend.rs`). The source has
      no more colour than Glide gets: mission files hold each 256x256 page as a `Cbmp`
      resource with `PX16` (16-bit PlayStation 1555 pixels), a 256-entry 16-bit palette
      `PLUT` and a lookup table `LkUp`. Ideas, most useful first:
  - [ ] Mipmaps and anisotropic filtering (distant floors and walls shimmer): built per
        sub-texture, since a page is an atlas and must not bleed
  - [ ] Debanding: a little noise when the 5-bit gradients are widened to 8 bits
  - [ ] An upscaling filter such as xBRZ (sharper edges than bilinear; a matter of taste)
- [ ] Remastered assets (later): replace textures recognised by their content. Replacements
      must not be limited to the game's formats: full colour (8 bits per channel and more,
      e.g. PNG), real alpha, higher resolution than the 256x256 page; the renderer keeps
      them as their own textures instead of converting them to 1555

### 3. Smaller open points

- [ ] FPU exception bits (status word bits 0-5) are not modelled (0x47c070, 0x469f60)
- [ ] Difftest: Linux support (demand paging uses a Windows exception handler)
- [ ] Difftest: start from a memory snapshot of the running game, so the 189 untested
      functions get tested
- [ ] Difftest: trace mode that finds the first diverging instruction (the old project had one)
- [ ] Performance: compile functions in parallel, cache compiled code; later x87 TOP
      known at compile time, fewer flag/state stores
