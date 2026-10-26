# future_rust_cop

A native port of **Future Cop: L.A.P.D.** (1998) in Rust. The game's own program is
recompiled: the player's `FCopLAPD.exe` is decoded with [iced-x86], every function is
lifted to [Cranelift] IR and compiled to native code. No game code or data is part of this
repository; players use their own copy of the game.

Status: prototype. The whole EXE (2176 functions, 174k instructions) is found and compiled,
every instruction supported (x87 included, and the renderer's self-modifying code). A
differential test against Unicorn agrees for every function it can run. The Windows/DirectX
layer is being written in Rust: the game starts and shows its main menu (no sound, videos or
3D yet); see PLAN.md.

```
cargo run --release -- path/to/FCopLAPD.exe
```

Without a path it uses the first FCopLAPD.exe under `game_files/`. Settings go to
`registry.txt` in the game folder. `cargo run --release --example crt` checks the game's C
runtime functions (string functions, 64-bit division) against Rust on random inputs.

[iced-x86]: https://github.com/icedland/iced
[Cranelift]: https://cranelift.dev
