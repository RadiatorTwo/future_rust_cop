# future_rust_cop

A native port of **Future Cop: L.A.P.D.** (1998) in Rust. The game's own program is
recompiled: the player's `FCopLAPD.exe` is decoded with [iced-x86], every function is
lifted to [Cranelift] IR and compiled to native code. No game code or data is part of this
repository; players use their own copy of the game.

Status: prototype. The whole EXE (2176 functions, 174k instructions) is found and compiled;
x87 floating point, self-modifying code and the Windows/DirectX layer are next.

```
cargo run --release -- path/to/FCopLAPD.exe
```

This recompiles the EXE and checks the game's C runtime functions (string functions, 64-bit
division) against Rust on random inputs.

[iced-x86]: https://github.com/icedland/iced
[Cranelift]: https://cranelift.dev
