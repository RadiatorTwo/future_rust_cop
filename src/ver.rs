//! Versions of FCopLAPD.exe. Everything we know about the game is by the addresses of the US
//! version (data/functions.json, data/symbols.txt, the hooks and the game's data we read).
//! Other versions are the same code linked again (the German one: built a week later, its
//! functions and data a few bytes elsewhere, a few functions changed): `data/map-*.txt` gives
//! each US address's place there (`examples/vermap.rs` makes it from both EXEs; only addresses).
//! A US address is a `Us`; `at` is where it is in the version that runs.

use std::sync::OnceLock;

/// A known version: SHA-256 of its FCopLAPD.exe, name, address map (none: the US one).
pub struct Version {
    pub sha256: &'static str,
    pub name: &'static str,
    map: Option<&'static str>,
}

pub const VERSIONS: [Version; 2] = [
    Version { sha256: "2dcdd1e78b99947d125712e1a159548e6f5f3929020c29af9e156314ed1c7e5e", name: "US", map: None },
    Version {
        sha256: "bffbc4bb437d7b804b1f7f91794d77271cab68ec56474277d68b3d01536a74f4",
        name: "German",
        map: Some(include_str!("../data/map-de.txt")),
    },
];

/// An address in the US version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Us(pub u32);

impl Us {
    /// Where it is in the version that runs.
    pub fn at(self) -> u32 {
        at(self.0)
    }
}

/// (US address, delta) runs, sorted; empty for the US version
static MAP: OnceLock<Vec<(u32, i32)>> = OnceLock::new();
/// the starts and ends of what the version that runs has not (functions taken out)
static GONE: OnceLock<Vec<(u32, bool)>> = OnceLock::new();

/// The version with this SHA-256 (lower-case hex), if we know it.
pub fn known(sha256: &str) -> Option<&'static Version> {
    VERSIONS.iter().find(|v| v.sha256 == sha256)
}

/// The version of this FCopLAPD.exe, if we know it.
pub fn of_exe(data: &[u8]) -> Option<&'static Version> {
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
    known(&hash)
}

/// Run this version (before anything asks `at`; the first choice stays).
pub fn select(v: &Version) {
    if MAP.get().is_some() {
        return;
    }
    let (mut runs, mut gone) = (Vec::new(), Vec::new());
    for l in v.map.unwrap_or("").lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let mut w = l.split_whitespace();
        let a = u32::from_str_radix(w.next().unwrap(), 16).expect("address map");
        match w.next().unwrap() {
            "-" => gone.push((a, true)),
            d => {
                runs.push((a, d.parse().expect("address map")));
                gone.push((a, false));
            }
        }
    }
    let _ = MAP.set(runs);
    let _ = GONE.set(gone);
    if v.map.is_some() {
        eprintln!("FCopLAPD.exe: the {} version", v.name);
    }
}

fn delta(runs: &[(u32, i32)], a: u32) -> i32 {
    match runs.partition_point(|r| r.0 <= a) {
        0 => 0,
        k => runs[k - 1].1,
    }
}

/// Where the US address `us` is in the version that runs.
pub fn at(us: u32) -> u32 {
    let runs = MAP.get().map_or(&[][..], Vec::as_slice);
    us.wrapping_add_signed(delta(runs, us))
}

/// Is the US address `us` in something the version that runs has not?
pub fn gone(us: u32) -> bool {
    let g = GONE.get().map_or(&[][..], Vec::as_slice);
    match g.partition_point(|r| r.0 <= us) {
        0 => false,
        k => g[k - 1].1,
    }
}
