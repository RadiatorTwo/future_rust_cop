//! The game's EXE: sections at their virtual addresses, and its import table.

use std::path::Path;

pub struct Section {
    pub name: String,
    pub va: u32,
    pub vsize: u32,
    pub raw: u32,
    pub rsize: u32,
}

pub struct Image {
    pub data: Vec<u8>,
    pub base: u32,
    pub entry: u32,
    pub sections: Vec<Section>,
}

/// One imported function: the IAT slot the game calls through.
pub struct Import {
    pub iat: u32,
    pub dll: String,
    pub name: String,
}

fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

impl Image {
    pub fn load(path: &Path) -> std::io::Result<Image> {
        let data = std::fs::read(path)?;
        let pe = le32(&data, 0x3c) as usize;
        let nsec = le16(&data, pe + 6) as usize;
        let opt = le16(&data, pe + 20) as usize;
        let base = le32(&data, pe + 24 + 28);
        let entry = base + le32(&data, pe + 24 + 16);
        let sections = (0..nsec)
            .map(|i| {
                let o = pe + 24 + opt + i * 40;
                let name = data[o..o + 8].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
                Section {
                    name,
                    vsize: le32(&data, o + 8),
                    va: base + le32(&data, o + 12),
                    rsize: le32(&data, o + 16),
                    raw: le32(&data, o + 20),
                }
            })
            .collect();
        Ok(Image { data, base, entry, sections })
    }

    /// Bytes at a virtual address; zeros past a section's raw data (.bss); `None` outside
    /// every section.
    pub fn read(&self, addr: u32, n: usize) -> Option<Vec<u8>> {
        let s = self.sections.iter().find(|s| addr >= s.va && addr < s.va + s.vsize.max(s.rsize))?;
        let off = addr - s.va;
        Some(
            (0..n as u32)
                .map(|k| {
                    let o = off + k;
                    if o < s.rsize { self.data.get((s.raw + o) as usize).copied().unwrap_or(0) } else { 0 }
                })
                .collect(),
        )
    }

    pub fn byte(&self, addr: u32) -> u8 {
        self.read(addr, 1).map_or(0, |b| b[0])
    }

    pub fn u32(&self, addr: u32) -> u32 {
        self.read(addr, 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn cstr(&self, addr: u32) -> String {
        (0..).map(|k| self.byte(addr + k)).take_while(|&b| b != 0).map(|b| b as char).collect()
    }

    pub fn imports(&self) -> Vec<Import> {
        let pe = le32(&self.data, 0x3c) as usize;
        let rva = le32(&self.data, pe + 24 + 104);
        let mut out = Vec::new();
        let mut d = self.base + rva;
        loop {
            let (oft, name, ft) = (self.u32(d), self.u32(d + 12), self.u32(d + 16));
            if oft == 0 && name == 0 && ft == 0 {
                break;
            }
            let dll = self.cstr(self.base + name);
            for k in 0.. {
                let thunk = self.u32(self.base + if oft != 0 { oft } else { ft } + 4 * k);
                if thunk == 0 {
                    break;
                }
                let name =
                    if thunk & 0x8000_0000 != 0 { format!("#{}", thunk & 0xffff) } else { self.cstr(self.base + thunk + 2) };
                out.push(Import { iat: self.base + ft + 4 * k, dll: dll.clone(), name });
            }
            d += 20;
        }
        out
    }
}

/// One entry of data/functions.json (function boundaries from a Ghidra export).
#[derive(serde::Deserialize)]
pub struct FuncInfo {
    pub addr: String,
    pub size: u32,
    pub name: String,
}

pub fn load_functions(path: &Path) -> std::io::Result<Vec<(u32, u32, String)>> {
    let list: Vec<FuncInfo> = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut out: Vec<(u32, u32, String)> = list
        .into_iter()
        .map(|f| {
            let lo = u32::from_str_radix(f.addr.trim_start_matches("0x"), 16).expect("functions.json: bad addr");
            (lo, lo + f.size, f.name)
        })
        .collect();
    out.sort();
    Ok(out)
}
