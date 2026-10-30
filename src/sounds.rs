//! `fcop export-sounds`: the missions' sounds as WAV files, to `game_files/Sounds/`. Every
//! mission file carries its own copy of what it plays (the game loads one at a time), so most
//! sounds are in several; each is written once, named after the first mission (in name order)
//! that has it: `<mission>_wav<n>.wav` for its effects (`Cwav` resources, WAV files already),
//! `<mission>_<stream>.wav` for its spoken messages (`snds`, by their stream's name:
//! `ConFt_1_01_04.wav`), `<mission>_music.wav` (`music2`... if there are more tracks).
//! `sounds.txt` lists each file with every mission and name that uses it. Files already there
//! are kept.

use crate::mission;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A WAV file of 8-bit samples.
fn wav(rate: u32, channels: u16, samples: &[u8]) -> Vec<u8> {
    let mut w = Vec::with_capacity(44 + samples.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * channels as u32).to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&8u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    w.extend_from_slice(samples);
    w
}

pub fn export(missions: &Path, out: &Path) -> Result<(), String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(missions)
        .map_err(|e| format!("{}: {e}", missions.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_none())
        .collect();
    files.sort();
    // each different sound once: its file name and who uses it ("M2C wav128")
    let mut sounds: Vec<(String, Vec<u8>, Vec<String>)> = Vec::new();
    let mut known: HashMap<Vec<u8>, usize> = HashMap::new();
    for file in files {
        let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let mission = file.file_name().unwrap().to_string_lossy().to_string();
        let mut found: Vec<(String, Vec<u8>)> = Vec::new();
        for r in mission::resources(&data).into_iter().filter(|r| r.kind == "Cwav") {
            found.push((format!("wav{}", r.id), r.data));
        }
        let streams = mission::streams(&data);
        for (name, samples) in &streams.voices {
            let name = name.strip_suffix(".stream").unwrap_or(name);
            found.push((name.to_string(), wav(22050, 1, samples)));
        }
        for (i, track) in streams.music.iter().enumerate() {
            let name = if i == 0 { "music".to_string() } else { format!("music{}", i + 1) };
            found.push((name, wav(14260, 2, track)));
        }
        for (name, bytes) in found {
            let user = format!("{mission} {name}");
            match known.get(&bytes) {
                Some(&i) => sounds[i].2.push(user),
                None => {
                    known.insert(bytes.clone(), sounds.len());
                    sounds.push((format!("{mission}_{name}"), bytes, vec![user]));
                }
            }
        }
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut used = HashSet::new();
    let mut list = String::new();
    for (name, bytes, users) in sounds {
        // a name twice (a stream without a name of its own): numbered
        let name = (1..).map(|n| if n == 1 { name.clone() } else { format!("{name}_{n}") }).find(|n| used.insert(n.clone())).unwrap();
        let path = out.join(format!("{name}.wav"));
        if !path.exists() {
            std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        list += &format!("{name}.wav: {}
", users.join(", "));
    }
    let path = out.join("sounds.txt");
    std::fs::write(&path, list).map_err(|e| format!("{}: {e}", path.display()))
}
