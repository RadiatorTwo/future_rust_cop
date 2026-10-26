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

## Open

### 1. Platform layer (next, the big one)

The game calls Windows and DirectX through its import table; each import needs a Rust
implementation behind `rt::set_handler` / `Event::Foreign`.

- [ ] List the imports the game uses and group them (kernel32, user32, ddraw, dsound,
      dinput?, glide, winmm, ...)
- [ ] Process start: TIB/FS segment (SEH uses `fs:[0]`), main stack, heap, command line
- [ ] Files and the CD check (volume label FCOP_CD, paths `D:\FCopLAPD\...`)
- [ ] Threads that take turns (the game's threads race when truly parallel)
- [ ] Window, input, timing (SDL2)
- [ ] Graphics: DirectDraw (software renderer) and Glide on OpenGL
- [ ] Sound: DirectSound on SDL2
- [ ] Videos (MPEG-1 from the CD's Video folder)
- [ ] Frame limit fields (0x406a7f, 0x406b30, 0x4071aa, 0x407278, 0x407295) as code fields
- [ ] First goal: reach the main menu

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
