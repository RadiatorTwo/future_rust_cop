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
- `src/lift/x87.rs`: the FPU. ST registers are doubles in `Cpu::st`, TOP is a Cranelift
  variable (ST(i) = slot `(top+i)&7`, computed at run time); fistp/frndint honour the control
  word's rounding mode; `fild qword` remembers the exact integer (`st_int`/`st_int_of`) for
  the memcpy idiom. Transcendentals and 80-bit operands go to `rt_x87`.
- `src/smc.rs`: self-modifying code. Fields = targets of `mov [code address], x` plus the
  patch table at 0x48ea20 (555/565). Finds 177 immediates/branch offsets (incl. shift
  counts) and 67 displacements, which the lifter reads from guest memory (`Lifter::field`),
  and 8 rewritten instructions (cmp/jb <-> test/je), lifted in every version and chosen by
  comparing the current bytes. Rewritten jumps/calls go through `jump_dyn`/`rt_lookup`.
- `src/jit.rs`: declares all functions, lifts and compiles them in one JITModule, plus a
  `foreign_stub` (calls to non-recompiled addresses -> `rt_foreign`) and an `enter`
  trampoline (host calling convention -> tail convention).
- `src/rt.rs`: `Cpu` (repr(C), offsets used by the lifter), the 4 GB guest address space
  (guest address a = base + a; game pointers stay 32-bit), helpers (`rt_string` does the
  string instructions).
- `src/main.rs`: prototype driver; tests strchr/strncpy/strstr/strcspn/strncmp/strpbrk
  (0x4780d0 ...), __aulldiv/__aullrem and _ftol (0x478090) against Rust on random inputs.
  Pass the EXE path (the old project's copy: ../FUTURE-COP-RECOMP/game_files/FCopLAPD/).
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
  `examples/smc.rs <exe>`: lists the self-modifying code fields.
- Odd instructions: MMX is only movq/movd copies (`Cpu::mm`); `in` reads 0xff (gameport,
  nothing connected), `out` does nothing; `push/pop es` only occur in data decoded as code.

## Known facts about the game (from the earlier project)

- Image 0x400000-0x522000, code 0x401000-0x483AE0; the original used a main stack at
  0x100000-0x400000, TIB at 0x10000, heap 0x01000000-0x40000000.
- The software renderer (0x45b000-0x461000) patches immediates, displacements and branch
  offsets in its own code; FUN_0045b1c0 rewrites code for 555/565 pixel formats from a
  table at 0x48ea20 (handled by `src/smc.rs`). The old project also rewrote the frame
  limit (the 8 ticks = 37.5 fps immediates at 0x406a7f, 0x406b30, 0x4071aa, 0x407278,
  0x407295) from its platform layer; add them as fields when the platform layer does that.
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

Every instruction in all 2176 functions is supported and the difftest passes for every
function Unicorn can run from random states, apart from the expected differences.

1. The platform layer in Rust (Win32/DirectDraw/DirectSound/Glide on SDL2 + OpenGL), then
   running the game.
2. Compile functions in parallel; cache compiled code.
