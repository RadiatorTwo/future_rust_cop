//! The player's game files: the folder `game_files` next to fcop.exe (in a checkout: in the
//! current folder). The player copies the CD or an installed game into it, anyhow; at every
//! start we find FCopLAPD.exe, check it is the one version we know, and keep only what the game
//! needs, in this layout (the CD's: the game folder and the videos next to it):
//!
//! ```text
//! game_files/FCopLAPD/   FCopLAPD.exe, febmp.bin, missions/ (and our registry.txt,
//!                        controls.txt, save.cfg: settings, key bindings, saves)
//! game_files/Video/      the videos (optional)
//! ```
//!
//! Everything else in `game_files` (the CD's installer, DirectX, the Mac version's MISSIONS
//! folder...) goes to the recycle bin, without asking.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The version of FCopLAPD.exe data/functions.json was made for.
const GAME_SHA256: &str = "2dcdd1e78b99947d125712e1a159548e6f5f3929020c29af9e156314ed1c7e5e";
const GAME: &str = "FCopLAPD";
const VIDEO: &str = "Video";
/// what stays in the game folder: the game's files and ours
const GAME_FILES: [&str; 6] = ["FCopLAPD.exe", "febmp.bin", "missions", "registry.txt", "controls.txt", "save.cfg"];
/// our note in `game_files` (in the release)
const NOTE: &str = "README.txt";

/// The folder with the game files: `game_files` next to fcop.exe, else in the current folder.
pub fn root() -> PathBuf {
    let here = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("game_files")));
    match here {
        Some(d) if d.is_dir() => d,
        _ => PathBuf::from("game_files"),
    }
}

/// Set up the game files in `root` and return FCopLAPD.exe's path, or what the player must do.
pub fn game_exe(root: &Path) -> Result<PathBuf, String> {
    let copy_in = format!(
        "Copy your Future Cop: L.A.P.D. CD (or your installed game folder) into this folder:\n\n{}\n\nThen start fcop.exe again.",
        root.display()
    );
    if !root.is_dir() {
        return Err(format!("The folder game_files was not found.\n\n{copy_in}"));
    }
    let game = root.join(GAME);
    let exe = match find(&game, "FCopLAPD.exe", 0).or_else(|| find(root, "FCopLAPD.exe", 6)) {
        Some(e) => e,
        None => return Err(format!("Future Cop's game files were not found.\n\n{copy_in}")),
    };
    check(&exe)?;
    let from = exe.parent().unwrap().to_path_buf();
    if !same(&from, &game) {
        std::fs::create_dir_all(&game).map_err(|e| format!("{}: {e}", game.display()))?;
        for name in GAME_FILES {
            move_in(&from, name, &game.join(name))?;
        }
    }
    for need in ["FCopLAPD.exe", "febmp.bin", "missions"] {
        if named(&game, need).is_none() {
            return Err(format!("{need} is missing next to FCopLAPD.exe in {}.\n\n{copy_in}", from.display()));
        }
    }
    let video = root.join(VIDEO);
    if !video.is_dir() {
        match find(root, "intro.mpg", 6).and_then(|p| p.parent().map(Path::to_path_buf)) {
            Some(v) if !same(&v, &game) => move_into(&v, &video)?,
            _ => eprintln!("setup: the game's videos (the CD's Video folder) were not found; the game runs without them"),
        }
    }
    writable(&game);
    writable(&video);
    // what the game does not need
    let mut rest = others(root, &[GAME, VIDEO, NOTE]);
    rest.extend(others(&game, &GAME_FILES));
    for p in rest {
        eprintln!("setup: not needed, to the recycle bin: {}", p.display());
        if let Err(e) = trash(&p) {
            eprintln!("setup: cannot remove {}: {e}", p.display());
        }
    }
    Ok(game.join(named(&game, "FCopLAPD.exe").unwrap()))
}

/// Is this the FCopLAPD.exe we know?
pub fn check(exe: &Path) -> Result<(), String> {
    let hash = sha256(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    if hash != GAME_SHA256 {
        return Err(format!(
            "{} is a version of Future Cop this port does not know (only the PC version with SHA-256 \
             {GAME_SHA256} is supported).\n\nIts SHA-256: {hash}",
            exe.display()
        ));
    }
    Ok(())
}

/// The entry of `dir` called `name`, in any case.
fn named(dir: &Path, name: &str) -> Option<std::ffi::OsString> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.file_name()).find(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
}

/// The first file called `name` in `dir` or below it, at most `depth` folders down (links are
/// not followed).
fn find(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            dirs.push(e.path());
        } else if t.is_file() && e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Some(e.path());
        }
    }
    if depth == 0 {
        return None;
    }
    dirs.sort();
    dirs.iter().find_map(|d| find(d, name, depth - 1))
}

fn same(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Move `from`/`name` (any case) to `to`, unless it is not there or `to` already is.
fn move_in(from: &Path, name: &str, to: &Path) -> Result<(), String> {
    match named(from, name) {
        Some(n) if !to.exists() => move_into(&from.join(n), to),
        _ => Ok(()),
    }
}

fn move_into(from: &Path, to: &Path) -> Result<(), String> {
    eprintln!("setup: {} -> {}", from.display(), to.display());
    std::fs::rename(from, to).map_err(|e| format!("Cannot move {} to {}: {e}", from.display(), to.display()))
}

/// The entries of `dir` other than `keep` (any case).
fn others(dir: &Path, keep: &[&str]) -> Vec<PathBuf> {
    let Ok(d) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = d
        .flatten()
        .filter(|e| !keep.iter().any(|k| e.file_name().to_string_lossy().eq_ignore_ascii_case(k)))
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// Files copied from a CD are read-only; the game writes into its folder.
fn writable(dir: &Path) {
    let Ok(d) = std::fs::read_dir(dir) else { return };
    for e in d.flatten() {
        let p = e.path();
        if let Ok(m) = std::fs::symlink_metadata(&p) {
            let mut perm = m.permissions();
            if perm.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(&p, perm);
            }
            if m.is_dir() {
                writable(&p);
            }
        }
    }
}

fn sha256(p: &Path) -> std::io::Result<String> {
    let d = Sha256::digest(std::fs::read(p)?);
    Ok(d.iter().map(|b| format!("{b:02x}")).collect())
}

/// Move a file or folder to the recycle bin.
#[cfg(windows)]
fn trash(p: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::*;
    // a list of paths, each NUL-terminated, the list ending with another NUL
    let from: Vec<u16> = p.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        ..Default::default()
    };
    match unsafe { SHFileOperationW(&mut op) } {
        0 if op.fAnyOperationsAborted == 0 => Ok(()),
        e => Err(format!("error {e:#x}")),
    }
}

#[cfg(not(windows))]
fn trash(p: &Path) -> Result<(), String> {
    Err(format!("{}: no recycle bin here, left as it is", p.display()))
}

/// Tell the player: on the console, and in a message box (not with FCOP_BATCH set, for tests).
pub fn tell(text: &str) {
    eprintln!("{text}");
    #[cfg(windows)]
    if std::env::var_os("FCOP_BATCH").is_none() {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (t, c) = (w(text), w("Future Cop"));
        unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR) };
    }
}
