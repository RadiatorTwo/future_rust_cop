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

Next:
- [ ] Click through the menu (mouse input is wired up but not yet tried); scripted input
      for tests (the old project had `FCOP_INPUT`)
- [ ] Spin-waits: when a thread waits in a loop without calling an import, the other thread
      never gets its turn (the old project yielded at loop heads; add when it shows up)
- [ ] Dialogs: Preferences etc. are the game's RT_DIALOG resources (DialogBoxParamA and
      friends are still missing); draw them in the game's style
- [ ] Sound: DirectSound (now reported as absent) on cpal
- [ ] DirectInput (now reported as absent; only used for joysticks)
- [ ] Missions: the software renderer through DirectDraw, then the 3D layer on OpenGL
      (Glide/THRASH), separate from the menu layer
- [ ] Videos (MPEG-1 from the CD's Video folder; DirectShow is reported as absent now)
- [ ] Frame limit fields (0x406a7f, 0x406b30, 0x4071aa, 0x407278, 0x407295) as code fields

### 2. The user's goals after that

- [ ] Quit option and preferences menu (controls, graphics, resolution)
- [ ] High resolution and widescreen through the OpenGL renderer
- [ ] Linux, later Android
- [ ] Remastered assets (later)

### 3. Smaller open points

- [ ] FPU exception bits (status word bits 0-5) are not modelled (0x47c070, 0x469f60)
- [ ] Difftest: Linux support (demand paging uses a Windows exception handler)
- [ ] Difftest: start from a memory snapshot of the running game, so the 189 untested
      functions get tested
- [ ] Difftest: trace mode that finds the first diverging instruction (the old project had one)
- [ ] Performance: compile functions in parallel, cache compiled code; later x87 TOP
      known at compile time, fewer flag/state stores
