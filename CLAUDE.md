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
- `src/image.rs`: the EXE's sections and imports; `data/functions.json` and `data/symbols.txt`
  are built into the program (`include_str!`: fcop.exe needs no files of its own).
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
  trampoline (host calling convention -> tail convention). Lifting runs on one thread,
  Cranelift's compiling on every core (`compile_all`: 5.5 s -> ~1.2 s with 16 threads); the
  machine code and its relocations go to `compiled.bin` beside FCopLAPD.exe (`compile_cached`;
  key: SHA-256 of fcop's own file and of the EXE, so any new build compiles again; not with
  FCOP_COUNT), and later starts only link it (~130 ms). Generated code must not hold host
  addresses as constants (statics move from run to run): `rt::POLL` is the data symbol
  `rt_poll`.
- `src/rt.rs`: `Cpu` (repr(C), offsets used by the lifter), the 4 GB guest address space
  (guest address a = base + a; game pointers stay 32-bit), helpers (`rt_string` does the
  string instructions).
- `src/mpeg/`: MPEG-1 movies, a Rust port of pl_mpeg (MIT, credited in `mod.rs`): `demux`
  splits the program stream, `video.rs` decodes MPEG-1 video (I/P/B) to YCbCr frames
  (`to_rgb`), `mp2.rs` Layer II audio; `tables.rs` holds pl_mpeg's tables (VLC trees as
  (next index, value) pairs). Matches ffmpeg (frames within IDCT rounding, sound within float
  noise); `examples/movie.rs <file.mpg> [dir]` decodes one and writes raw frames/samples.
- `src/setup.rs`: the game files in `game_files` (next to fcop.exe, else in the current
  folder): finds FCopLAPD.exe (6 folders deep), checks its SHA-256, moves FCopLAPD.exe,
  febmp.bin, missions (and registry.txt, controls.txt, save.cfg, compiled.bin) into `game_files/FCopLAPD`
  and the folder with intro.mpg into `game_files/Video` (no FCopLAPD.exe but a CD image, .iso
  or raw .bin/.img, in game_files (then trashed) or next to fcop.exe (kept; game_files is made
  there): `src/iso.rs` (ISO 9660, Joliet names when there are) extracts those from it,
  checked first; `examples/iso.rs` lists an image), clears read-only (CD copies), and
  moves everything else in both folders to the recycle bin (elsewhere deleted), at every start, without asking
  (kept beside them: `game_files/Textures`, the player's replacement textures).
  `src/splash.rs`: a small Win32 window on its own thread shows what happens until the game's
  window opens (looking, copying from a CD image in %, compiling) or why it cannot start (with
  a Close button); none with `FCOP_BATCH=1` (tests). Closing it while starting quits. On Linux
  it is a second process, `fcop --splash` (winit allows one event loop a process), drawn with
  the dialogs' font through `gl.rs`, its texts sent line by line on its stdin (end of input:
  it closes; after an error it waits for Close).
  The release ships `dist/game_files/README.txt` (kept there).
- `src/main.rs`: compiles the EXE and runs the game (a release build has no console window:
  messages go to the console it was started from or a redirection, else to fcop.log next to
  fcop.exe; debug builds keep the console) (`fcop [exe]`, the EXE checked too;
  default: `setup`; here `game_files/`, a copy of the old project's game folder without its
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
  thread, play cursors advance in real time; without a device a timer thread advances them;
  the effects' volume 0x4318e0 is hooked: amplitude in proportion, 0 silent (the game's own
  curve ended at -16.7 dB)),
  `misc.rs` (winmm timers, ole32, comctl32),
  `dinput.rs` (DirectInput 5, only for joysticks: gamepads through gilrs, at most 2 reported
  (the game has room for 2); player i uses pad i, one pad is shared. The game reads lX/lY by
  their sign (0 inside its 25% dead zone), but hooks on robot physics and its hover/walk movement
  vector helpers restore each assigned axis independently for proportional movement, turning,
  and jinking (not in network games: their packets have no magnitude), and
  buttons 0...16. Our button order makes the
  game's own joystick profile 1 a modern layout: 0 RT gun, 1 LT heavy, 2 B special, 3 A jump,
  4 X action, 5 Y change target, 6/7 LB/RB jink, 8 Back map, 9 R3 camera, 10 L3, 11...14 D-pad
  (free to bind); the left stick moves; Start is the Escape key (pause). Right-stick directions
  are extended binding keys 0x16...0x19 (joy_key, joy_capture and controls.txt parse/format are
  hooked). Mouse left/right/middle and Back/Forward (X1/X2) are keyboard-device bindings using
  their Windows virtual-key codes; the controller keyboard hook captures right/middle/side buttons
  directly and left on a repeated click of the selected action, and the parse/format hooks persist
  their compact names. The Controller
  dialog reads every 250 ms and takes only buttons 0...9: presses are
  kept until the game reads the pad (a short press counts), and joy_capture 0x412c10 is hooked
  for 10...16. gilrs starts
  before compiling (Windows finds pads ~75 ms later, the game looks once at its start), is
  polled at every `window::pump` and when the game reads a pad on the main thread, never dropped. `FCOP_PAD_LOG=1` logs; `pad@A*500`, `pad@LX=-1*1000` in
  FCOP_INPUT drive a scripted pad (pad 0)), `dshow.rs`
  (DirectShow for the videos, 0x408800: CoCreateInstance gives the filter graph with
  IMediaControl/IMediaEventEx/IVideoWindow/IBasicAudio; RenderFile demuxes the MPEG file, Run
  plays it on a thread: frames to `window::set_video` (a layer over the frame, in the video
  window's place in the 4:3 picture), sound to the mixer's stream (`dsound::stream_*`), which
  is also the clock; at the end EC_COMPLETE and the notify message WM_USER. The game skips with
  Space or a click itself (0x408ab0 calls Stop). It never releases IBasicAudio, so the graph
  is replaced at the next CoCreateInstance),
  `dplay.rs` (DirectPlay for network play: CoCreateInstance(CLSID_DirectPlay) gives an
  IDirectPlay4A; one connection, "TCP/IP"; the host listens on TCP and UDP port 47624
  (`FCOP_NET_PORT`, `FCOP_NET_BIND` the address, e.g. 127.0.0.1 for tests without the firewall
  prompt), EnumSessions asks by UDP (broadcast and 127.0.0.1, or the address typed into our
  "Host Address" field in the Join dialog, kept as REG_SZ `JoinAddress`), Open(JOIN) connects by
  TCP; frames: length, kind, contents; the host passes messages on. A message for a local
  player sets its event; Receive builds system messages (player created/destroyed, session lost)
  with their names behind them. `FCOP_NET_LOG=1` logs. Tested with two instances: host started
  with FCOP_INPUT "4000:click@160,162 5000:click@317,120 6000:click@300,146 7000:click@232,386",
  the other "6000:click@160,192 7000:click@317,73 8000:click@300,99 10000:click@320,125
  11000:click@232,435", then START (484,303) on both),
  `window.rs` (winit window and input; main thread only, other threads' frames are shown
  at the next pump), `gl.rs` (the OpenGL 3.3 context: shows a 2D frame or a rendered texture,
  letterboxed, with the dialog overlay on top; alpha is not written to the window, as Wayland
  blends windows by it; FCOP_SHOT reads back the window), `glide.rs`
  (Glide 2.x on OpenGL: LoadLibraryA("glide2x.dll") gives `glide::MODULE`, GetProcAddress a
  thunk per decorated name `_grX@N`; unknown names get `thunk_not_implemented`, logged at
  their first call. Triangles are batched into a texture of the grSstWinOpen size; only
  blending, chroma key, filtering, clamping, clip window and primitive type end a batch (one
  draw call; lines become quads a pixel wide per 720 lines of the target, so the laser sight
  grows with the resolution): the colour/alpha combine units, the constant colour and the texture are vertex
  attributes, and every texture is a layer of one 256x256 texture array (64 layers, by TMU
  address; a download frees the textures it overwrites in TMU memory). Before that the game's
  per-triangle state changes made ~240 draw calls a frame and capped 300 fps at ~200. Frames are
  shown at grBufferSwap with the game's gamma; the fragment shader does Glide's colour and
  alpha combine units and the chroma key, with bilinear filtering of the unkeyed texels and
  their coverage as a soft edge). Textures are stored premultiplied by coverage (key black,
  as the game always sets it; texels' alpha in a second RG8 array), so the GPU's filtering is
  that keyed filter: mipmaps (`glGenerateMipmap` before the next draw after a download) and
  anisotropic filtering, a primitive sampling inside its texture rectangle (flat vertex
  attribute) at most at the level where that spans 4 texels (pages are atlases). REG_DWORD
  `TextureFilter` 0 original, 1 trilinear, 2...16 anisotropic (default 16; "Textures" in
  Preferences, below Frame Rate; OK/Cancel moved down a row); `FCOP_FILTER=<n>` overrides.
  MSAA: drawn into a multisampled renderbuffer (`Glide::ms`, REG_DWORD `Msaa` 0/2/4/8,
  default 4, "Anti-alias" in Preferences below Textures; `FCOP_MSAA=<n>`), resolved into the
  shown texture at grBufferSwap.
  Replacement textures (`hd.rs`): each download (whole page or band of rows) hashed (FNV-1a of
  format, size, texels); `game_files/Textures/**/<...><16 hex>.png` replaces it (RGBA, alpha =
  coverage, up to 4x; all scaled to the largest's scale). `Hd`: a second array (32 layers,
  mipmaps; `Texture::hd` its layer, vertex mode bits 16... layer + 1); a band without a
  replacement on a page with one is the original blitted up (`Hd::scale_up`); the shader
  takes colour and coverage from it, the texel alpha from the original. `FCOP_DUMP_TEXTURES=1`
  writes every new download to `Textures/dump/<hash>.png`; `FCOP_HD=0` off.
  Replacements equal to the original (exported pages nobody changed) are left out.
- `src/mission.rs`: mission files (`missions/M2C`..., the first mission M2C; `GlblData` has no
  models): chunks with reversed tags ("COHS" = SHOC); SHOC holds SHDR (kind at +24, id +28) or
  SDAT (data from +20). Resources `Cbmp` (pages: `PX16` 256x256 16-bit, colour as Glide's 1555;
  the game sets the alpha bit where a texel is not black and its own top bit (PS1 STP) is
  clear: that gives the download's hash), `Cobj` (models: `4DVL` vertices (word, count, x y z
  pad i16), `3DQL` faces (word, count, 12 bytes: flags, corners in the low 3 bits of byte 1
  (3/4; 7, 8 are points/lines), u16 byte offset into `3DTL`'s entries, 4 vertex, 4 normal
  indices), `3DTL` (word, 16 bytes each: kind, colour 80 80 80, 4 u v bytes in page texels,
  page = Cbmp id - 1), also `4DNL`, `3DRF`, `3DRL`, `3DBB`, `3DHY`/`3DMI`/`3DHS`/`AnmD`
  (animated: several vertex lists)), `Ctil` (`Sect`: u16 at 2 = texel coordinate pairs, 16x16
  cells at 0x3c4 (first tile << 6 | count), tile count << 6 at 0x5c4, tiles from 0x5c8 (u32:
  bits 1-10 byte offset of its pairs, 15-21 shape (triangles: < 60, 71...102), 22-31
  texture), then the pairs, then the textures (u16: bits 8-10 page, 0x2000 four pairs, 13-15
  how drawn); read at 0x4537f0/0x453930), `Cact`, `Cwav`... Model y is up, as the world's.
  `src/export.rs`: `fcop export` cuts pages into pictures by what uses them (models' sheets,
  terrain), `fcop import` puts changed ones back into replacement pages. `tools/texview`:
  a separate egui program (own crate and Cargo.lock) to browse those pieces (UV grid,
  original, place on the page, JSON; reloads what an editor saves).
  Registry `Group` = `3Dfx` (default) or `Software`.
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
  while the loop waits for its next frame (clock read from 0x406a56; network game: 0x4071e4,
  0x407282 in 0x406b20, frames in pairs of 8+0 ticks on the host, 7+1 on the joining computer,
  which waits for the host's frame at 0x406cc6; its frame is the two-view
  0x4076d0, and 0x40ca10 draws only this computer's view, [0x511fa0] & 0xf: the host 0, the
  other 1 (its player [0x511fd0 + 4 * view]); pictures draw that one with its camera 0x4348e0
  selected; it runs the visibility pass
  0x452880 before 0x426eb0 with the real camera, so its own picture runs it again with the shown
  one, else walls and terrain shook), shown with 0x4153a0,
  with every actor (list at 0x50e428, count [0x50e420]; matrix +0x2c, position +0x50; the
  player robot's side weapons and lights are model records in its data [+0xac] at +0x178,
  +0x1c4, +0x210, placed at +4, drawn by 0x447ba0: interpolated too, else they jerked) and the
  camera ([0x511f48], up to +0x88) put where they were at a game time ([0x511ee4] of each
  frame, last 4 kept; only a frame 8 ticks after the last kept, as a pair of the network
  game's frames moves things evenly, its single frames do not) 10 ticks behind the newest frame, as far as the clock went on since (the
  clock's lead over the game time averaged): by game time, so uneven frames do not matter;
  the frame's own render too. 0x44fd20
  in the render block is logic (the actor grid 0x4a31b0 for hits, per-frame lists): it runs
  only in the game's frame, with the real places (in pictures it multiplied enemy damage
  and made temporary things flicker). Otherwise drawing only changes the random generators
  (0x50e370/0x4a2b08, 0x50e36c/0x4a2ab8: index and table), the views' centres and cells the
  visibility pass leaves for the logic (0x4a39b8, 0x4a3b00: 0x42e570 wakes objects near them)
  and the display list (records from [0x499b0c] to [0x511dc0]; 0x40ca10 links them
  by depth, resets the end, draws), both kept: the loop adds records after the frame's
  render (0x407d70: messages, indicator 0x4357f0) for the next frame, and a picture would
  use them up (they flickered), so each picture puts them back. Split screen ([0x511f54], two views in 0x4076d0 from the
  one-view loop): pictures draw both views, each with its player's camera (both cameras
  interpolated, found by number as 0x434900 does; [0x499b24] picks the half and every 0x4114c0
  moves it on), places put back after the second view's 0x4114c0; not while a player's view
  shows 0x405cb0 instead of the world. Only with Glide, playing ([0x511fb8] = 4), not paused. Choices 60 ... 300 and unlimited (REG_DWORD
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
  keypad belongs to player 2; times count from the game's start, after the compile (~1.2 s, ~0.13 s from compiled.bin):
  give `timeout` enough on top; the intro videos now come first: "3000:Space 5000:Space
  8000:Space 11000:click@484,303 20000:Space" skips them and the briefing and starts the
  mission; `poke@addr=value` writes a dword (hex); `text@Name` types characters (WM_CHAR,
  `_` a space); cheat names run a cheat, below),
  `FCOP_WATCH=1` (last import call and turn state every 2 s), `FCOP_PROF=1` (time in
  each import, every 2 s), `FCOP_COUNT=<file>` (calls per function and second: "second
  address calls [name]"; counting is compiled in only then), `FCOP_VIDEO_LOG=1` (videos, their window and end), `FCOP_NO_VIDEO=1` (videos end at
  once, as played to their end: `FCOP_INPUT="8000:click@484,303"` alone starts the first
  mission), `FCOP_WINDOW=1920x1080`
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
- Cameras (`camera.rs`): robot_camera 0x4456a0 (hooked) places the player's camera each
  frame; the camera key counts the robot data's mode +0xa4 (0...6, a table at 0x48c468 /
  presets at 0x48c2a8) and sets a timer +0xae to 900 (a press = that timer going up). Modes 5
  and 6 are skipped (back to 0 at once) where their view is blocked, so our mode (close
  behind; first person only with FCOP_CAMERA=first, it crashed the visibility pass, see
  PLAN.md) follows a press that brings the game back to 0; leaving it sets it to 0 again. The original runs in mode 0, then the camera ([0x434900](number at
  data +0xac)) gets our angles via 0x434760(yaw, pitch, roll) and position via
  0x4346d0(&pos), both on the camera in [0x511f48]. Positions 20.12, y up; yaw (actor
  +0xb0, 4096 a turn) faces (sin, 0, cos); pitch > 0 looks down. A press counts once in 250
  ms (it counted twice while held). Names: render_hud 0x434c90 (hooked) shows the name from
  the table [0x48b7fc] by the game's mode while the key timer runs; for a player in ours the
  pointer goes to our table for that call ("Chase Camera", "First Person Camera"). First person skips robot_draw 0x447ba0 in the robot's
  own view (a skipping hook pops the return address itself); the HUD's target markers
  0x435330 end with the laser line (type 0x3a), which `drop_laser` takes off the list end.
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
  item's text there, selected as a whole, shown highlighted; texts with the focus have a caret
  blinking at Windows' 530 ms, restarted by keys and clicks): the Load/Save dialog (163) takes the save's name
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

## Releases

`.github/workflows/build.yml`: on windows-latest and ubuntu-22.04 (older glibc, runs on newer
distributions; ALSA and libudev headers from apt), tests and builds at a tag `v*` (or started by
hand: `gh workflow run build.yml`), not at every push; a tag also makes a GitHub release with `fcop-<tag>-windows-x64.zip` and
`fcop-<tag>-linux-x64.tar.gz` (the program, README, LICENSE, `game_files/README.txt`).
Version in Cargo.toml. Linux tested in a CachyOS VM (VMware, NAT): the host serves the
source by HTTP to it (copy and paste there is text only).

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
