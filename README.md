# future_rust_cop

A native port of **Future Cop: L.A.P.D.** (1998) in Rust. The game's own program is
recompiled: the player's `FCopLAPD.exe` is decoded with [iced-x86], every function is
lifted to [Cranelift] IR and compiled to native code when the game starts, and the
Windows/DirectX functions it calls are implemented in Rust (OpenGL for the 3D renderer, its
videos, sound, network play). No game code or data is part of this repository or its
releases: you need your own copy of the game.

![A mission in widescreen at 1920x1080](docs/screenshots/game.png)

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/precinct.png" alt="Precinct Assault"></td>
    <td width="50%"><img src="docs/screenshots/menu.png" alt="The main menu"></td>
  </tr>
  <tr>
    <td align="center"><em>Precinct Assault</em></td>
    <td align="center"><em>The main menu</em></td>
  </tr>
</table>

## Playing

1. Download from the [releases]: `fcop-...-windows-x64.zip` for Windows or
   `fcop-...-linux-x64.tar.gz` for Linux (x86-64, OpenGL 3.3), and unpack it.
2. Put an image of your CD (`.iso` or `.bin`) next to `fcop.exe` (Linux: `fcop`), or copy your
   game into the `game_files` folder: the contents of the CD, an image of it, or your
   installed game folder.
3. Start `fcop.exe` (Linux: `./fcop` in its folder). It finds the game, checks the version,
   and keeps only what the game needs (`game_files/FCopLAPD`: FCopLAPD.exe, febmp.bin,
   missions; `game_files/Video`: the videos; taken out of a CD image); everything else in
   `game_files` goes to the recycle bin (Linux: it is deleted). The first start compiles the
   game (a second or two; kept in `game_files/FCopLAPD/compiled.bin`), later starts are quick.

Only the PC version's `FCopLAPD.exe` (SHA-256 `2dcdd1e7...7e5e`) is supported; with any other,
or without one, fcop says so. Its messages go to `fcop.log` next to it on Windows, to the
terminal on Linux. Settings go to `registry.txt`, key bindings to `controls.txt` and saves to
`save.cfg` in `game_files/FCopLAPD`. On Linux the libraries for sound (ALSA, or PipeWire/
PulseAudio through it) and gamepads (libudev) are needed; every desktop distribution has them.

What is new compared to the original:
- High resolution and widescreen (Hor+), fullscreen (Alt+Enter), anti-aliasing,
  texture filtering; all in the game's Preferences dialog
- Frame rates up to unlimited, with the game's logic unchanged
- Gamepads (up to two): choose the joystick profile in the Controller dialog. Left stick
  moves, RT gun, LT heavy weapon, A jump, B special, X action, Y change target, LB/RB jink,
  Back map, R3 camera, Start pause; the D-pad is free to bind
- Network play over TCP/IP (Host Game / Join Game; port 47624, type the host's address in
  the Join dialog)
- The arrow keys for player 1, and a camera close behind the robot

## Building

```
cargo build --release
cargo run --release -- path/to/FCopLAPD.exe
```

Without a path it sets up and uses `game_files` (next to fcop.exe, else in the current
folder) as above. `cargo test --release`
needs no game files. How it works and what is open: `CLAUDE.md` and `PLAN.md`.

[iced-x86]: https://github.com/icedland/iced
[Cranelift]: https://cranelift.dev
[releases]: https://github.com/RadiatorTwo/future_rust_cop/releases
