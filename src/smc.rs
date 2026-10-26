//! Self-modifying code. The software renderer (0x45b000-0x461000) writes immediates,
//! displacements and branch offsets of its own inner loops, and FUN_0045b1c0 rewrites code
//! for the 555/565 pixel formats from the table at 0x48ea20. Fields that are exactly an
//! operand are read from guest memory by the generated code; instructions whose other bytes
//! change get one version per encoding the table can produce, chosen at run time by
//! comparing the bytes in guest memory.

use crate::discover::{Code, Program, TEXT_END, TEXT_START, is_code};
use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};
use std::collections::{BTreeMap, HashMap};

/// FUN_0045b1c0's patch table: entries of a size in bits (1 byte), a code address and the
/// value for each pixel format (4 bytes each); a zero size ends it.
const PATCH_TABLE: u32 = 0x48ea20;

/// The mission loop's frame limit: it waits until this many timer ticks (1/300 s, 8 = 37.5
/// fps) have passed since the last frame. The game never writes them; the platform layer
/// may. (instruction, field, size): `cmp reg, imm8` and `mov edi, imm32`.
pub const FRAME_LIMIT: [(u32, u32, u32); 5] = [
    (0x406a7f, 0x406a81, 1),
    (0x406b30, 0x406b31, 4),
    (0x4071aa, 0x4071ab, 4),
    (0x407278, 0x40727a, 1),
    (0x407295, 0x407297, 1),
];

/// A code field written at run time: its guest address and size in bytes.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub at: u32,
    pub size: u32,
}

#[derive(Default)]
pub struct Smc {
    /// instruction address -> its immediate (for branches: the offset), read at run time
    pub imm: HashMap<u32, Field>,
    /// instruction address -> its memory displacement, read at run time
    pub disp: HashMap<u32, Field>,
    /// instruction address -> the other encodings it can be rewritten to (same length)
    pub variants: HashMap<u32, Vec<(Vec<u8>, Instruction)>>,
}

impl Smc {
    pub fn find(prog: &Program, code: &mut Code) -> Smc {
        let mut starts: Vec<u32> = prog.fns.values().flat_map(|f| f.ins.iter().copied()).collect();
        starts.sort_unstable();
        starts.dedup();

        // fields: what `mov [code address], x` writes, the frame limit and the patch table's
        // entries
        let mut fields: BTreeMap<u32, (u32, Option<[u32; 2]>)> = BTreeMap::new();
        for &a in &starts {
            let i = code.at(a).unwrap();
            if i.mnemonic() == Mnemonic::Mov
                && i.op0_kind() == OpKind::Memory
                && i.memory_base() == Register::None
                && i.memory_index() == Register::None
                && is_code(i.memory_displacement32())
            {
                fields.insert(i.memory_displacement32(), (i.memory_size().size() as u32, None));
            }
        }
        for (a, f, n) in FRAME_LIMIT {
            if starts.binary_search(&a).is_ok() {
                fields.insert(f, (n, None));
            }
        }
        let table = patch_table(code);
        for &(f, n, v) in &table {
            fields.insert(f, (n, Some(v)));
        }

        let mut smc = Smc::default();
        let mut rewritten = Vec::new();
        for (&f, &(n, vals)) in &fields {
            let k = starts.partition_point(|&a| a <= f);
            let Some(&a) = k.checked_sub(1).map(|k| &starts[k]) else { continue };
            let i = code.at(a).unwrap();
            if f >= a + i.len() as u32 {
                eprintln!("warning: code field {f:#x} is not inside a decoded instruction");
                continue;
            }
            let co = offsets(code, a);
            let off = (f - a) as usize;
            let field = Field { at: f, size: n };
            if co.has_immediate() && off == co.immediate_offset() && n as usize == co.immediate_size() {
                smc.imm.insert(a, field);
            } else if co.has_displacement() && off == co.displacement_offset() && n as usize == co.displacement_size() {
                smc.disp.insert(a, field);
            } else if vals.is_some() {
                // every instruction the field overlaps
                rewritten.extend(starts[k - 1..].iter().copied().take_while(|&s| s < f + n));
            } else {
                eprintln!("warning: code field {f:#x} is not an operand of `{i}` at {a:#x}");
            }
        }

        // the code as each pixel format's table values leave it
        let images: Vec<HashMap<u32, u8>> = (0..2)
            .map(|k| {
                let mut img = HashMap::new();
                for &(f, n, v) in &table {
                    for (j, b) in v[k].to_le_bytes().iter().take(n as usize).enumerate() {
                        img.insert(f + j as u32, *b);
                    }
                }
                img
            })
            .collect();
        rewritten.sort_unstable();
        rewritten.dedup();
        for a in rewritten {
            let i = code.at(a).unwrap();
            let orig = code.img.read(a, i.len()).unwrap();
            let mut vs: Vec<(Vec<u8>, Instruction)> = Vec::new();
            for img in &images {
                let b: Vec<u8> = orig.iter().enumerate().map(|(j, &x)| *img.get(&(a + j as u32)).unwrap_or(&x)).collect();
                if b == orig || vs.iter().any(|v| v.0 == b) {
                    continue;
                }
                let v = Decoder::with_ip(32, &b, a as u64, DecoderOptions::NONE).decode();
                assert!(!v.is_invalid() && v.len() == i.len(), "rewritten instruction at {a:#x} changes its length");
                vs.push((b, v));
            }
            smc.variants.insert(a, vs);
        }
        smc
    }

    /// Targets of rewritten branches (discovery must decode them as code too).
    pub fn variant_targets(&self) -> impl Iterator<Item = u32> + '_ {
        self.variants.values().flatten().filter_map(|(_, v)| {
            (v.flow_control() != FlowControl::Next && v.op0_kind() == OpKind::NearBranch32).then(|| v.near_branch32())
        })
    }
}

fn patch_table(code: &Code) -> Vec<(u32, u32, [u32; 2])> {
    let mut out = Vec::new();
    let mut at = PATCH_TABLE;
    loop {
        let bits = code.img.byte(at);
        if bits == 0 {
            break;
        }
        let f = code.img.u32(at + 1);
        assert!((TEXT_START..TEXT_END).contains(&f), "patch table entry {at:#x} points outside the code");
        out.push((f, bits as u32 / 8, [code.img.u32(at + 5), code.img.u32(at + 9)]));
        at += 13;
    }
    out
}

fn offsets(code: &Code, a: u32) -> iced_x86::ConstantOffsets {
    let bytes = code.img.read(a, 16).unwrap();
    let mut d = Decoder::with_ip(32, &bytes, a as u64, DecoderOptions::NONE);
    let i = d.decode();
    d.get_constant_offsets(&i)
}
