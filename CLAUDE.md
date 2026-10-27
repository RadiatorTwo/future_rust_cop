# CLAUDE.md — Future Cop in Rust

Hand-over for future sessions. What is done and what is open: **PLAN.md** (keep it current).

## Goal (the user's)

A modern native port of Future Cop: L.A.P.D. (1998), now entirely in Rust (decided
2026-10-26; the earlier C project is not continued here and none of its code belongs in this
repository). Wanted: the game playable natively, a quit option and preferences menu
(controls, graphics, resolution), high resolution and widescreen through an OpenGL renderer,
later remastered assets, Windows/Linux/Android.

Only code is ever shared; players supply their own game files. **Never commit anything
derived from the EXE** (translated code, extracted data). `game_files/` is git-ignored.
`data/functions.json` (function boundaries from a Ghidra export) is fine to keep.

## The user

- New to reverse engineering; explain in plain words, give a recommendation, not a survey.
- Speaks German; works in short sessions and watches usage limits.
- Confirm before anything outward-facing or destructive (pushing, deleting).
- Prefers clean Rust over generated C; performance comes later ("ordentlicher Rust Code,
  optimieren wir hinterher").

## How it works

- `data/symbols.txt`: what we know about the game's functions and data, by address (name,
  description; "?" marks guesses). Keep it current when something new is understood.
- `src/image.rs`: the EXE's sections and imports; `data/functions.json` loader.
- `src/discover.rs`: finds the code (recursive descent from each function, jump tables,
  extra entry points where other code jumps into the middle of a function or where data
  holds code addresses). Finds 2176 functions incl. 147 extra entries, matching the old
  project exactly.
- `src/lift.rs`: x86 -> Cranelift IR. Registers and flags are Cranelift variables, loaded
  from `rt::Cpu` on entry and stored around calls/returns/exits. One block per instruction;
  jumps into other functions are tail calls (`return_call`, `CallConv::Tail`), indirect
  targets go through `rt_lookup`. `unsupported()` checks an instruction before emitting
  anything; such instructions call `rt_unimpl`.
- `src/lift/x87.rs`: the FPU. ST registers are doubles in `Cpu::st`, TOP is a Cranelift
  variable (ST(i) = slot `(top+i)&7`, computed at run time); fistp/frndint honour the control
  word's rounding mode; `fild qword` remembers the exact integer (`st_int`/`st_int_of`) for
  the memcpy idiom. Transcendentals and 80-bit operands go to `rt_x87`.
- `src/smc.rs`: self-modifying code. Fields = targets of `mov [code address], x` plus the
  patch table at 0x48ea20 (555/565). Finds 177 immediates/branch offsets (incl. shift
  counts) and 67
  displacements, which the lifter reads from guest memory (`Lifter::field`), and 8 rewritten
  instructions (cmp/jb <-> test/je), lifted in every version and chosen by comparing the
  current bytes. Rewritten jumps/calls go through `jump_dyn`/`rt_lookup`.
- `src/jit.rs`: declares all functions, lifts and compiles them in one JITModule, plus a
  `foreign_stub` (calls to non-recompiled addresses -> `rt_foreign`) and an `enter`
  trampoline (host calling convention -> tail convention).
- `src/rt.rs`: `Cpu` (repr(C), offsets used by the lifter), the 4 GB guest address space
  (guest address a = base + a; game pointers stay 32-bit), helpers (`rt_string` does the
  string instructions).
- `src/mpeg/`: MPEG-1 movies, a Rust port of pl_mpeg (MIT, credited in `mod.rs`): `demux`
  splits the program stream, `video.rs` decodes MPEG-1 video (I/P/B) to YCbCr frames
  (`to_rgb`), `mp2.rs` Layer II audio; `tables.rs` holds pl_mpeg's tables (VLC trees as
  (next index, value) pairs). Matches ffmpeg (frames within IDCT rounding, sound within float
  noise); `examples/movie.rs <file.mpg> [dir]` decodes one and writes raw frames/samples.
- `src/main.rs`: compiles the EXE and runs the game (`fcop [exe]`; default: the first
  FCopLAPD.exe under `game_files/`, a copy of the old project's game folder without its
  registry.txt/controls.txt). `examples/crt.rs` tests strchr/strncpy/strstr/strcspn/
  strncmp/strpbrk (0x4780d0 ...), __aulldiv/__aullrem and _ftol (0x478090) against Rust.
- `src/platform/`: the Windows/DirectX layer. `mod.rs`: guest memory layout (TIB 0x10000,
  stack 0x100000-0x400000, heap 0x01000000-0x40000000, thunks 0x7f000000), loading the EXE,
  binding imports to thunks (`Import` = name, stdcall argument count or -1, `fn(&mut Cpu)
  -> u32`), the event handler (runs the import, sets eax, pops return address and
  arguments), `guest_call` (host -> guest: window procedures, threads, callbacks), COM
  objects (`com_new`: one guest vtable of thunks per method table), the turn lock (one guest
  thread runs at a time; handed over around every import), `host_path` (case-insensitive,
  `D:\FCopLAPD\` = game folder, falls back to its parent for the CD's Video folder),
  `printf`. One file per DLL: `kernel32.rs` (kernel objects in one table with one
  condition variable), `user32.rs` (windows, message queue, timers, key state; desktop
  1600x1200), `gdi32.rs`, `advapi32.rs` (registry in `registry.txt`), `ddraw.rs`,
  `dsound.rs` (DirectSound and PlaySoundA: voices in guest memory mixed on cpal's audio
  thread, play cursors advance in real time; without a device a timer thread advances them),
  `misc.rs` (winmm timers, ole32, comctl32; DirectInput reports "absent"), `dshow.rs`
  (DirectShow for the videos, 0x408800: CoCreateInstance gives the filter graph with
  IMediaControl/IMediaEventEx/IVideoWindow/IBasicAudio; RenderFile demuxes the MPEG file, Run
  plays it on a thread: frames to `window::set_video` (a layer over the frame, in the video
  window's place in the 4:3 picture), sound to the mixer's stream (`dsound::stream_*`), which
  is also the clock; at the end EC_COMPLETE and the notify message WM_USER. The game skips with
  Space or a click itself (0x408ab0 calls Stop). It never releases IBasicAudio, so the graph
  is replaced at the next CoCreateInstance),
  `window.rs` (winit window and input; main thread only, other threads' frames are shown
  at the next pump), `gl.rs` (the OpenGL 3.3 context: shows a 2D frame or a rendered texture,
  letterboxed, with the dialog overlay on top; FCOP_SHOT reads back the window), `glide.rs`
  (Glide 2.x on OpenGL: LoadLibraryA("glide2x.dll") gives `glide::MODULE`, GetProcAddress a
  thunk per decorated name `_grX@N`; unknown names get `thunk_not_implemented`, logged at
  their first call. Triangles are batched into a texture of the grSstWinOpen size; only
  blending, chroma key, filtering, clamping, clip window and primitive type end a batch (one
  draw call): the colour/alpha combine units, the constant colour and the texture are vertex
  attributes, and every texture is a layer of one 256x256 texture array (64 layers, by TMU
  address; a download frees the textures it overwrites in TMU memory). Before that the game's
  per-triangle state changes made ~240 draw calls a frame and capped 300 fps at ~200. Frames are
  shown at grBufferSwap with the game's gamma; the fragment shader does Glide's colour and
  alpha combine units and the chroma key, with bilinear filtering of the unkeyed texels and
  their coverage as a soft edge). Registry `Group` = `3Dfx` (default) or `Software`.
  Frames are drawn as tall as the window (high resolution; the Glide screen, 1280x1024 for
  the game's 1280x960, only gives the coordinates), 4:3 or as wide as the window's aspect;
  `display.rs`: the resolution chosen in Preferences (our list replaces the game's Display
  Mode list 1021 with Glide; REG_DWORD `RenderSize` = w << 16 | h, 0 = the window's size)
  is the size drawn at, and the window's starting size. Fullscreen (borderless) is REG_DWORD `Fullscreen`:
  our box where the game's FullScreen box (1012, hidden with Glide) was, and Alt+Enter.
- Hooks: `platform::HOOKS` lists game functions replaced by Rust functions; `jit.rs` compiles
  the original as `G_x` and makes `F_x` call `rt_hook`; the hook runs the original with
  `rt::call_original` (return address on the guest stack, its `ret` pops it).
- `walker.rs`: hook of the Glide display list walker 0x4393e0 (display list layout in its
  header). Precise mode points the resolution's jump table entry (0x43a168) at the plain
  path 0x4397cd, so vertices arrive in 2560x1920 units; text (type 6, game pixels) is drawn in
  its own calls; `glide::set_space` tells the renderer which space. `FCOP_PRECISE=0` off.
- `wide.rs`: widescreen (Hor+). k = (4/3) / aspect scales camera-space x before the
  projections (0x453740 terrain vertex, 0x457340 object points, 0x457140 sprites) and the
  sphere view tests (0x452fd0 terrain cells, 0x4572a0 objects); target markers (0x435330,
  HUD level) and full-height HUD records are moved back to 3D positions (`to_screen`). The
  aspect comes from the window at every grBufferSwap (`set_aspect`; the Glide target keeps
  its height, its width follows). `walker.rs` draws the HUD with 4:3 views
  (`glide::View`: left / centre / right edge, split screen one per half). Needs precise
  mode. Players switch it in Preferences ("Widescreen" in 3DFX Options, REG_DWORD
  `Widescreen` in the game's key, default on). `FCOP_WIDE=0` off, `FCOP_WIDE=<aspect>` forces one. Dialog overlays stay 4:3 in the
  middle of a wide frame (`window::overlay_span`). Alt+Enter toggles borderless fullscreen (and saves it).
- `fps.rs`: the mission frame rate. The game's logic stays at its 8 ticks a frame (clock:
  1/300 s ticks from QPC, 0x4130c0; mission loop 0x406820, frame 0x407360): it computes
  (rate * ticks) >> 8 in many places, and shorter frames broke hits, effects, hovering...
  (an earlier attempt wrote the loop's tick limit 0x406a7f etc.). Instead pictures are drawn
  in between: the frame's render block (0x426eb0 ... 0x4114c0, see the module) runs again
  while the loop waits for its next frame (clock read from 0x406a56), shown with 0x4153a0,
  with every actor (list at 0x50e428, count [0x50e420]; matrix +0x2c, position +0x50; the
  player robot's side weapons and lights are model records in its data [+0xac] at +0x178,
  +0x1c4, +0x210, placed at +4, drawn by 0x447ba0: interpolated too, else they jerked) and the
  camera ([0x511f48], up to +0x88) put between their places in the last two frames; the
  frame's own render shows the start of the interval, so pictures lag one frame. 0x44fd20
  in the render block is logic (the actor grid 0x4a31b0 for hits, per-frame lists): it runs
  only in the game's frame, with the real places (in pictures it multiplied enemy damage
  and made temporary things flicker). Otherwise drawing only changes the random seed
  0x50e370 and the display list (records from [0x499b0c] to [0x511dc0]; 0x40ca10 links them
  by depth, resets the end, draws), both kept: the loop adds records after the frame's
  render (0x407d70: messages, indicator 0x4357f0) for the next frame, and a picture would
  use them up (they flickered), so each picture puts them back. Only with Glide, one view, playing ([0x511fb8] = 4), not paused. Choices 60 ... 300 and unlimited (REG_DWORD
  `FrameRate`, 0 = original 37.5, 0xffffffff unlimited: ~950 fps in the first mission; our combo below the 3DFX Options group in Preferences).
  Between frames it sleeps (turn handed over) until 2 ms before the next picture or frame.
  `FCOP_FPS=1` logs the frames shown per second (Glide).
- Imports not implemented yet are listed at start and stop the game when called.
  Switches: `FCOP_TRACE_API=1` (every import call with its caller), `FCOP_TRACE_MSG=1`
  (window messages), `FCOP_TRACE_REG=1`, `FCOP_DD_LOG=1` (surfaces and blits),
  `FCOP_SHOT=file.png` (the frame shown, every half second; `{t}` in the name: ms since start,
  every picture kept), `FCOP_INPUT="6000:click@484,303
  14000:Up*5000"` (scripted input, ms after start; that one starts the first mission and
  walks forward with the user's bindings in `controls.txt`: player 1 on the arrow keys, the
  keypad belongs to player 2; times count from the game's start, after the ~5.5 s compile:
  give `timeout` enough on top; the intro videos now come first: "3000:Space 5000:Space
  8000:Space 11000:click@484,303 20000:Space" skips them and the briefing and starts the
  mission; `poke@addr=value` writes a dword (hex); `text@Name` types characters (WM_CHAR,
  `_` a space); cheat names run a cheat, below),
  `FCOP_WATCH=1` (last import call and turn state every 2 s), `FCOP_PROF=1` (time in
  each import, every 2 s), `FCOP_COUNT=<file>` (calls per function and second: "second
  address calls [name]"; counting is compiled in only then), `FCOP_VIDEO_LOG=1` (videos, their window and end), `FCOP_WINDOW=1920x1080`
  (the window's starting size in pixels; Windows limits it to the screen). An access
  violation in guest memory is reported with the recompiled function it happened in.
- Key bindings: `controls.txt` in the game folder (14 lines per player: forward, back,
  turn right, turn left, gun, action, special, jump, map, camera, jink, jink, heavy weapon,
  change target), written by the game's Controller dialog (and when it is missing: back it
  up before tests). Without it 0x417aa0 copies profile [0x4a01b0] of the tables at 0x48afb8
  (player 1) / 0x48b120 (player 2): 3 profiles of 0x78 bytes, (device, key) pairs, device 0
  keyboard with virtual keys, 1 joystick. Both keyboard profiles are the keypad;
  `user32::single_player_keys` rewrites player 1's at start (arrows, A/D jink).
- Cheats (`cheats.rs`): by name in FCOP_INPUT ("30000:win") and, with `FCOP_CHEATS=1`, on
  Shift+F1... (Ctrl is player 1's special weapon; the game does not see these keys). They
  run in the mission loop's frame wait (hook of 0x406a30), only while playing. `win`: progress
  +0x40 = goal +0x44 of the game record [0x511f80], state 0xb, as the mission script does (the
  reward video follows); `lose`: progress != goal, state 0xb; `health`: as the game makes a
  robot whole (0x447700): health (actor +0x1c, 16 bits; maximum +0x1e) of the robot and its
  upper part (actor +0xdc) set, then the damage look 0x42bab0(actor, old health) for both
  (else the sparks below a quarter stayed; the robot data's +0x58 is only the HUD's copy);
  `ammo`: data +0x84... as at the mission's start; `hurt`: the game's damage 0x446720(actor,
  max / 4). Game functions are called with `guest_call`. New ones go into `CHEATS`.
- WM_PAINT (`user32.rs`): windows have a dirty flag (InvalidateRect, first ShowWindow, the
  video window hidden with put_Visible 0); GetMessage/PeekMessage return WM_PAINT when the
  queue is empty, until ValidateRect/BeginPaint. The game's window procedure (0x45a340) redraws
  the whole menu (0x40b250) at WM_PAINT; that is how the menu comes back after a video that
  played to its end (a skipped one redraws it itself; without WM_PAINT the mission's loading
  bar stayed on the menu after a won level).
- Focus: the game pauses itself on WM_ACTIVATEAPP 0; we never send it, so it keeps running
  in the background, and keys held when the focus goes are released (WM_KEYUP).
- Spin-waits: `rt::POLL` is nonzero while a guest thread waits for its turn; generated code
  loads it at every loop head (target of a backward branch) and calls `rt_yield`.
- `resource(type, id)` finds resources in the loaded EXE (`image::list_resources` works on
  the file too; `examples/dialogs.rs` lists every dialog template and control).
- `platform/dialog/`: the game's dialogs. `template.rs` parses DLGTEMPLATE(EX); `mod.rs` builds
  controls (button, check, radio, group, static, edit, list, combo, trackbar), handles the
  game's messages to them, runs the modal loop (input goes to the topmost dialog, other
  messages are dispatched as usual) and implements DialogBoxParamA, EndDialog, GetDlgItem...,
  MessageBoxA; user32 routes window functions on dialog handles (0x00DAxxyy) here. `draw.rs`
  draws them into a 640x480 ARGB overlay (`window::set_overlay`, stretched over the frame) with
  `font.rs` (public-domain 8x8 font, set proportionally). `extend.rs` is where our own
  settings go: change a template before it is built (`adjust`), fill our controls after the
  game's WM_INITDIALOG (`init`), handle them (`command`), save them on OK (`ok`); our
  controls have ids from 0x7E00 and the game's procedure never sees them. Keyboard hooks
  (SetWindowsHookExA WH_KEYBOARD) get key messages as they are taken from the queue: the
  controller dialog reads rebound keys that way, and marks the waiting box with InvertRect.
  Combo boxes with CBS_DROPDOWN can be typed into (click on the text; a selection puts the
  item's text there, selected as a whole): the Load/Save dialog (163) takes the save's name
  from it (GetDlgItemTextA; "-EMPTY-" is refused), else saves had no name and showed empty.
  `FCOP_DLG_LOG=1` logs dialogs and the messages sent to their procedures.
- DDBLTFX: dwFillColor is at 0x50 (the old project read 0x48 and drew the pause screen's
  dark purple 0x1c2a as black).
- The game draws in two separate layers: menus are 640x480 16-bit frames through DirectDraw
  2D (blits to the primary surface; windowed, the primary is the whole desktop and the main
  window's rectangle is shown); missions use the 3D renderer (software, Glide or THRASH).
  Blits to the primary only mark it changed; it is shown when the game polls its messages,
  at a Flip, or 50 ms after the first unshown change (showing every blit made the menu
  flicker while it built up).
- `tests/`: x86 snippets assembled with iced's code_asm, recompiled and checked against Rust
  (`cargo test --release`, harness in `tests/common/`); needs no game files. `x87.rs` for
  the FPU, `misc.rs` for MMX moves, ports, push/pop es, `smc.rs` for self-modifying code.
- `src/bin/difftest.rs`: differential test against Unicorn (optional feature `difftest`,
  Unicorn is GPL-2.0 and never linked into the game). Every function from N random states
  (`-n`, default 6) on both sides; registers, flags, x87 stack and memory compared; an
  import call ends a run on both sides. Worker processes (`-j`), restarted after a crash or
  a hang of the recompiled side. `-v` prints details, incl. why untested functions fault.
  Build on Windows inside a VS developer environment (bindgen needs the MSVC headers) with
  CMAKE pointing at VS's cmake.exe, e.g. a .cmd: `call "...\VC\Auxiliary\Build\vcvars64.bat"`,
  `set CMAKE=...\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe`,
  `cargo build --release --features difftest --bin difftest`. Windows only for now (demand
  paging uses a vectored exception handler).
  Expected differences: 0x45b1c0 cpuid, 0x4742e0 rdtsc, 0x4764d0 `in` (we return 0xff),
  0x47c070 / 0x469f60 FPU exception bits (not modelled), 0x478ac0 a bogus Ghidra entry
  (decodes as `lock push`), 0x470470 (push/pop es in data), 0x478fe0 (calls a random
  scratch address, Unicorn runs the bytes), 0x457550 (an access at 0xffffffff: Unicorn
  wraps to 0, we use the extra page behind 4 GB). Flag-only differences at the end are
  undefined flags. Page 0 is mapped on demand too (null pointers in uninitialised globals),
  which makes about 1990 of 2176 functions testable; -n 20 takes about 25 minutes.
- `src/rt.rs` `Event`/`set_handler`: imports, traps and unsupported instructions go to a
  handler; setting `Cpu::stop` unwinds out of guest code (checked after every call).
- `examples/missing.rs <exe>`: lists every instruction the lifter does not support yet;
  `examples/smc.rs <exe>`: lists the self-modifying code fields;
  `examples/dis.rs <exe> <addr>`: disassembles a function as discovery sees it (`all`: every
  one; names from `data/symbols.txt` on targets); crash reports name the function too;
  `examples/at.rs <exe> <addr>...`: decodes single instructions.
- Odd instructions: MMX is only movq/movd copies (`Cpu::mm`); `in` reads 0xff (gameport,
  nothing connected), `out` does nothing; `push/pop es` only occur in data decoded as code.

## Known facts about the game (from the earlier project)

- Image 0x400000-0x522000, code 0x401000-0x483AE0; the original used a main stack at
  0x100000-0x400000, TIB at 0x10000, heap 0x01000000-0x40000000.
- The software renderer (0x45b000-0x461000) patches immediates, displacements and branch
  offsets in its own code; FUN_0045b1c0 rewrites code for 555/565 pixel formats from a
  table at 0x48ea20 (handled by `src/smc.rs`). The mission frame limit (8 ticks = 37.5
  fps) is in immediates at 0x406a7f, 0x406b30, 0x4071aa, 0x407278, 0x407295; the game's logic
  needs it (see `fps.rs`).
- The game's threads raced when run truly in parallel (async file reader 0x415f30 and its
  callback 0x416bf0 share a one-slot mailbox): guest threads must take turns.
- x87 runs at 53-bit precision (the CRT sets it), but the renderer loads control word
  0xc7f (24 bits, truncating) from 0x48ce04: arithmetic under any control word other than
  53/64 bits to nearest goes through `rt_fpu_round` (exact rounding via TwoSum/FMA error
  signs). Exact s64 shadow for the fild/fistp qword memcpy idiom.
- CD check: volume label FCOP_CD on drive D:, paths like D:\FCopLAPD\Missions\...; the CD's
  root MISSIONS folder is the Mac version (big-endian). Videos are MPEG-1 in the CD's Video
  folder. The supported EXE has SHA-256 2dcdd1e7...7e5e.

## Next steps

See PLAN.md. The main menu shows; next: clicking through the menu, dialogs, sound,
then missions. The old C project's platform layer (../FUTURE-COP-RECOMP/recomp/platform/,
its CLAUDE.md "Lessons") is a good source of facts about the game's behaviour; its code is
not copied.
