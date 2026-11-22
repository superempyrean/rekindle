//! What an anchor author reads off a function: the operands of its
//! instructions, its calls and callers, and where a value is carried.

use std::collections::{HashSet, VecDeque};

use anyhow::{Context, Result};
use iced_x86::{Decoder, DecoderOptions, Instruction};

use rekindle_sigmatch::short::PeImage;

use crate::descriptor::Operand;
use crate::index::{BinaryIndex, operand_of};

/// Where a value is carried under a function's calls: the call instructions
/// from `function` down (empty if it's in `function` itself) and the
/// instruction with `operand` = `value`, searching breadth first to `depth`
/// calls deep. For a value that a build moved into a callee (a thunk the
/// compiler added, say): the path becomes call-target anchors, the
/// instruction an operand anchor inside the last.
pub fn find_value(
    binary: &[u8],
    function: u64,
    value: i64,
    operand: Operand,
    depth: usize,
) -> Result<Option<(Vec<u64>, u64)>> {
    let pe = PeImage::parse(binary)?;
    let index = BinaryIndex::new(binary);
    let mut queue = VecDeque::from([(function, Vec::<u64>::new())]);
    let mut seen = HashSet::from([function]);
    while let Some((f, path)) = queue.pop_front() {
        let Some((begin, end)) = pe.to_rva(f).ok().and_then(|r| pe.function_containing(r)) else {
            continue;
        };
        let (Some(start), Some(stop)) = (
            pe.rva_to_offset(begin),
            pe.rva_to_offset(end - 1).map(|o| o + 1),
        ) else {
            continue;
        };
        let insns: Vec<Instruction> = Decoder::with_ip(
            64,
            &binary[start..stop],
            pe.image_base + u64::from(begin),
            DecoderOptions::NONE,
        )
        .into_iter()
        .collect();
        if let Some(ins) = insns.iter().find(|i| operand_of(i, operand) == Some(value)) {
            return Ok(Some((path, ins.ip())));
        }
        let calls = index.calls(insns);
        if path.len() < depth {
            for (at, target) in calls {
                let off = pe.to_rva(at).ok().and_then(|r| pe.rva_to_offset(r));
                if off.and_then(|o| index.call_target(o)).is_some() && seen.insert(target) {
                    let mut p = path.clone();
                    p.push(at);
                    queue.push_back((target, p));
                }
            }
        }
    }
    Ok(None)
}

/// An instruction's VA, displacement, first immediate, and whether it names
/// other code or data. See [`operands`].
pub type InstructionOperands = (u64, Option<i64>, Option<i64>, bool);

/// What an operand anchor could read off each instruction of the function
/// containing `va`: the instruction's VA, its displacement and its first
/// immediate, by the same rules as [`Operand`] (none for a branch), and
/// whether it names other code or data (a call, or a RIP-relative operand),
/// which is where a function's shapes are taken. For finding the
/// instructions that carry a measured value.
pub fn operands(binary: &[u8], va: u64) -> Result<Vec<InstructionOperands>> {
    let pe = PeImage::parse(binary)?;
    let (begin, end) = pe
        .function_containing(pe.to_rva(va)?)
        .context("no .pdata function there")?;
    let index = BinaryIndex::new(binary);
    let start = pe.rva_to_offset(begin).context("function start")?;
    let stop = pe.rva_to_offset(end - 1).context("function end")? + 1;
    Ok(index
        .instructions(start, stop)
        .iter()
        .map(|i| {
            let branch = i.is_call_near() || i.is_jmp_near() || i.is_jcc_short_or_near();
            let value = |o| if branch { None } else { operand_of(i, o) };
            (
                i.ip(),
                value(Operand::Disp),
                value(Operand::Imm),
                i.is_call_near() || i.is_ip_rel_memory_operand(),
            )
        })
        .collect())
}

/// The calls in the function containing `va`, as (site VA, target VA): see
/// `BinaryIndex::calls` for calls through a GOT slot.
pub fn calls(binary: &[u8], va: u64) -> Result<Vec<(u64, u64)>> {
    let pe = PeImage::parse(binary)?;
    let (begin, end) = pe
        .function_containing(pe.to_rva(va)?)
        .context("no .pdata function there")?;
    let index = BinaryIndex::new(binary);
    let (_, d) = index.decoder(begin, end).context("function bytes")?;
    Ok(index.calls(d))
}

/// The entries (VAs) of the functions that call the function at `va`.
pub fn callers(binary: &[u8], va: u64) -> Result<Vec<u64>> {
    let pe = PeImage::parse(binary)?;
    let index = BinaryIndex::new(binary);
    let off = pe
        .rva_to_offset(pe.to_rva(va)?)
        .context("not in the file")?;
    Ok(index
        .callers(off)
        .into_iter()
        .filter_map(|o| Some(pe.image_base + u64::from(pe.offset_to_rva(o)?)))
        .collect())
}
