# CLAUDE.md — Future Cop in Rust

Hand-over for future sessions.

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
- `src/jit.rs`: declares all functions, lifts and compiles them in one JITModule, plus a
  `foreign_stub` (calls to non-recompiled addresses -> `rt_foreign`) and an `enter`
  trampoline (host calling convention -> tail convention).
- `src/rt.rs`: `Cpu` (repr(C), offsets used by the lifter), the 4 GB guest address space
  (guest address a = base + a; game pointers stay 32-bit), helpers (`rt_string` does the
  string instructions).
- `src/main.rs`: prototype driver; tests strchr/strncpy/strstr/strcspn/strncmp/strpbrk
  (0x4780d0 ...) and __aulldiv/__aullrem against Rust on random inputs.

## Known facts about the game (from the earlier project)

- Image 0x400000-0x522000, code 0x401000-0x483AE0; the original used a main stack at
  0x100000-0x400000, TIB at 0x10000, heap 0x01000000-0x40000000.
- The software renderer (0x45b000-0x461000) patches immediates, displacements and branch
  offsets in its own code; FUN_0045b1c0 rewrites code for 555/565 pixel formats from a
  table at 0x48ea20. The lifter must read those fields from guest memory (not done yet).
- The game's threads raced when run truly in parallel (async file reader 0x415f30 and its
  callback 0x416bf0 share a one-slot mailbox): guest threads must take turns.
- x87 runs at 53-bit precision (the CRT sets it): doubles are enough, with an exact s64
  shadow for the fild/fistp qword memcpy idiom.
- CD check: volume label FCOP_CD on drive D:, paths like D:\FCopLAPD\Missions\...; the CD's
  root MISSIONS folder is the Mac version (big-endian). Videos are MPEG-1 in the CD's Video
  folder. The supported EXE has SHA-256 2dcdd1e7...7e5e.

## Next steps

1. x87 (most of the 6% of functions that are still incomplete).
2. Self-modifying code fields (renderer), then a differential test against a reference
   emulator for broad coverage.
3. The platform layer in Rust (Win32/DirectDraw/DirectSound/Glide on SDL2 + OpenGL), then
   running the game.
4. Compile functions in parallel; cache compiled code.
