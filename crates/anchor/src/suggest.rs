//! Building anchors from a binary: locators for a function, a call, an
//! operand or an instruction's shape, each checked against the binary
//! before it is returned.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction};

use rekindle_sigmatch::short::{Machine, PeImage};
use rekindle_sigmatch::short::{
    OperandKinds, ShortOptions, interior_signature, signature_among, trim_unique,
};
use rekindle_sigmatch::sig::Signature;

use crate::a64;
use crate::descriptor::{Anchor, Locator, Operand};
use crate::index::{BinaryIndex, operand_of};
use crate::resolve::{Status, resolve_anchor, resolve_locator};
use crate::shape;

/// A suggested shape has at least this many instructions, so it stays unique
/// in a build that inlined more into the function, and at most this many.
const SHAPE_MIN: usize = 4;
const SHAPE_MAX: usize = 16;
/// A shape unique in the whole binary, as a function anchor's locator, is
/// at least this long and [distinctive](shape::distinctive).
const SHAPE_MIN_GLOBAL: usize = 8;

/// Build an operand anchor for the instruction at `insn` (a VA): the value
/// of its `operand`, located by a short signature of the instruction (its
/// operand bytes wildcarded, so the locator still finds it when the value
/// changes) that is unique inside its own function: the function another
/// anchor, `within`, finds in later builds. A shape of the instruction and
/// its neighbours (see the `shape` module) locates it too, in a build whose
/// registers or values moved and broke the bytes. Up to `max_locators - 1`
/// other instructions in the same function carrying the same value
/// (independent uses of the same field offset or size) are located the same
/// two ways, and corroborate it by vote.
pub fn suggest_operand(
    binary: &[u8],
    name: &str,
    insn: u64,
    operand: Operand,
    within: &str,
    relative_to: Option<&str>,
    max_locators: usize,
) -> Result<Anchor> {
    let pe = PeImage::parse(binary)?;
    if pe.machine != Machine::X86_64 {
        bail!("operand anchors are built from x64 code only");
    }
    let rva = pe.to_rva(insn)?;
    let off = pe
        .rva_to_offset(rva)
        .context("instruction is not in the file")?;
    let (begin, scope) = function_at(&pe, rva)?;
    let index = BinaryIndex::new(binary);
    let want = index
        .operand_value(off, operand)
        .with_context(|| format!("{name}: no {operand:?} operand at {insn:#x}"))?;
    let opts = ShortOptions {
        min_fixed: 6,
        scope: Some(scope),
        ..Default::default()
    };
    let keep_imm = ShortOptions {
        wildcard: OperandKinds::keeping("displ")?,
        ..opts
    };
    let sig_at = |va: u64| {
        signature_among(&pe, va, &[], &opts).or_else(|_| signature_among(&pe, va, &[], &keep_imm))
    };
    sig_at(insn)
        .with_context(|| format!("{name}: no unique signature at {insn:#x} within its function"))?;
    // The instruction, then others in the function carrying the same value
    // (independent uses of the same field offset or size), up to
    // `max_locators` instructions.
    let code = &binary[scope.0..scope.1];
    let mut chosen = vec![insn];
    let mut d = Decoder::with_ip(
        64,
        code,
        pe.image_base + u64::from(begin),
        DecoderOptions::NONE,
    );
    let mut ins = Instruction::default();
    while d.can_decode() && chosen.len() < max_locators {
        d.decode_out(&mut ins);
        if ins.ip() != insn && operand_of(&ins, operand) == Some(want) {
            chosen.push(ins.ip());
        }
    }
    // Each by its bytes, and by two shapes (context after it, context before
    // it), which outlive the bytes when the registers or the value change.
    // In such a build, shapes agreeing on a new value corroborate it.
    let (offs, insns) = index.shapes(scope.0, scope.1);
    let mut locators = Vec::new();
    for va in chosen {
        if let Ok(sig) = sig_at(va) {
            let pattern = sig.to_pattern();
            if !locators
                .iter()
                .any(|l| matches!(l, Locator::Bytes { pattern: p } if *p == pattern))
            {
                locators.push(Locator::Bytes { pattern });
            }
        }
        let at_off = pe.to_rva(va).ok().and_then(|r| pe.rva_to_offset(r));
        if let Some(i) = offs.iter().position(|&o| Some(o) == at_off) {
            for lead in [shape::Lead::After, shape::Lead::Before] {
                if let Some((insns, at)) = shape::suggest(
                    &insns,
                    i,
                    operand == Operand::Imm,
                    SHAPE_MIN,
                    SHAPE_MAX,
                    lead,
                ) {
                    let loc = Locator::Shape { insns, at };
                    if !locators.contains(&loc) {
                        locators.push(loc);
                    }
                }
            }
        }
    }
    let anchor = Anchor {
        name: name.to_string(),
        within: Some(within.to_string()),
        operand: Some(operand),
        relative_to: relative_to.map(str::to_string),
        plus: None,
        locators,
        ..Default::default()
    };
    // Check it resolves to the value here, scoped as it will be.
    let r = resolve_anchor(&anchor, &index, Some(scope), &HashMap::new());
    if r.value != Some(want) {
        bail!(
            "{name}: suggested locators resolve to {:?}, not {want}",
            r.value
        );
    }
    Ok(anchor)
}

/// Build an operand anchor from a value instead of an instruction: the
/// first instruction in the function at `function` (a VA) whose `operand` is
/// `value` (the layout entry as measured from source, looked for in a build
/// whose code moved), then as [`suggest_operand`].
pub fn suggest_operand_by_value(
    binary: &[u8],
    name: &str,
    function: u64,
    value: i64,
    operand: Operand,
    within: &str,
    max_locators: usize,
) -> Result<Anchor> {
    let pe = PeImage::parse(binary)?;
    if pe.machine != Machine::X86_64 {
        bail!("operand anchors are built from x64 code only");
    }
    let (begin, (start, stop)) = function_at(&pe, pe.to_rva(function)?)?;
    let mut d = Decoder::with_ip(
        64,
        &binary[start..stop],
        pe.image_base + u64::from(begin),
        DecoderOptions::NONE,
    );
    let mut ins = Instruction::default();
    while d.can_decode() {
        d.decode_out(&mut ins);
        if operand_of(&ins, operand) == Some(value) {
            return suggest_operand(binary, name, ins.ip(), operand, within, None, max_locators);
        }
    }
    bail!("{name}: no {operand:?} operand of {value} in the function at {function:#x}")
}

/// Build a function anchor for the function the `call` at `insn` (a VA)
/// calls, located by that call: a short signature of the call instruction,
/// unique inside the function another anchor, `within`, finds.
pub fn suggest_call(binary: &[u8], name: &str, insn: u64, within: &str) -> Result<Anchor> {
    let pe = PeImage::parse(binary)?;
    if pe.machine != Machine::X86_64 {
        bail!("call-target locators are built from x64 code only");
    }
    let rva = pe.to_rva(insn)?;
    let off = pe
        .rva_to_offset(rva)
        .context("instruction is not in the file")?;
    let (begin, scope) = function_at(&pe, rva)?;
    let index = BinaryIndex::new(binary);
    let want = index
        .call_target(off)
        .with_context(|| format!("{name}: no near call at {insn:#x}"))?;
    let opts = ShortOptions {
        min_fixed: 6,
        scope: Some(scope),
        ..Default::default()
    };
    // As for operands: if wildcarding displacements leaves no unique window,
    // keep them (the call's own target stays wildcarded either way).
    let keep_displ = ShortOptions {
        wildcard: OperandKinds::keeping("displ")?,
        ..opts
    };
    // A wrapper around one call through a GOT slot (`push rax; call [rip+slot]; pop rax; ret`) has 5 fixed bytes.
    let wrapper = ShortOptions {
        min_fixed: 5,
        ..opts
    };
    // Start at the call; failing that (the bytes after it run out of the
    // function before they're unique), at one of the few instructions before
    // it, as long as no other call comes first.
    let mut starts = vec![insn];
    let code = &binary[scope.0..scope.1];
    let mut d = Decoder::with_ip(
        64,
        code,
        pe.image_base + u64::from(begin),
        DecoderOptions::NONE,
    );
    let mut before = Vec::new();
    while d.can_decode() && d.ip() < insn {
        let ins = d.decode();
        if ins.flow_control() == iced_x86::FlowControl::Call {
            before.clear();
        } else {
            before.push(ins.ip());
        }
    }
    starts.extend(before.iter().rev().take(4));
    for start in starts {
        for o in [&opts, &keep_displ, &wrapper] {
            let Ok(sig) = signature_among(&pe, start, &[], o) else {
                continue;
            };
            if start + (sig.len() as u64) <= insn {
                continue;
            }
            let anchor = Anchor {
                name: name.to_string(),
                function: true,
                within: Some(within.to_string()),
                locators: vec![Locator::CallTarget {
                    pattern: sig.to_pattern(),
                }],
                ..Default::default()
            };
            if resolve_anchor(&anchor, &index, Some(scope), &HashMap::new()).offset() == Some(want)
            {
                return Ok(anchor);
            }
        }
    }
    bail!("{name}: no unique signature covering the call at {insn:#x} within its function")
}

/// A shape locator for the instruction at `insn` (a VA): the shortest run of
/// instructions around it whose shape matches once, inside its own function
/// when `scoped` (for an anchor `within` that function), else in the whole
/// binary (for a function anchor: a structural locator that outlives a
/// compiler change, where byte patterns don't). `wild_imm` writes the
/// instruction's immediates as `*`, for an operand anchor that reads one.
pub fn suggest_shape(binary: &[u8], insn: u64, scoped: bool, wild_imm: bool) -> Result<Locator> {
    let pe = PeImage::parse(binary)?;
    if pe.machine != Machine::X86_64 {
        bail!("shapes are x64 instructions only");
    }
    let rva = pe.to_rva(insn)?;
    let off = pe
        .rva_to_offset(rva)
        .context("instruction is not in the file")?;
    let (_, scope) = function_at(&pe, rva)?;
    let index = BinaryIndex::new(binary);
    let (offs, insns) = index.shapes(scope.0, scope.1);
    let target = offs
        .iter()
        .position(|&o| o == off)
        .with_context(|| format!("no instruction starts at {insn:#x}"))?;
    let found = if scoped {
        shape::suggest(
            &insns,
            target,
            wild_imm,
            SHAPE_MIN,
            SHAPE_MAX,
            shape::Lead::After,
        )
    } else {
        let all: Vec<Vec<Instruction>> = index
            .function_ranges()
            .into_iter()
            .map(|(a, b)| index.instructions(a, b))
            .collect();
        let mut memo = HashMap::new();
        shape::suggest_where(
            &insns,
            target,
            wild_imm,
            SHAPE_MIN_GLOBAL,
            SHAPE_MAX,
            shape::Lead::After,
            |pat| {
                if !shape::distinctive(pat, SHAPE_MIN_GLOBAL) {
                    return false;
                }
                memo.clear();
                let mut n = 0;
                for f in &all {
                    n += shape::find_raw(pat, f, &mut memo).len();
                    if n > 1 {
                        return false;
                    }
                }
                n == 1
            },
        )
    };
    let (insns, at) = found.with_context(|| {
        format!("no run of up to {SHAPE_MAX} instructions around {insn:#x} has a unique shape")
    })?;
    let hits = index.status_from(index.shape_hits(&insns, at, scoped.then_some(scope)));
    if !matches!(hits, Status::Unique { offset, .. } if offset == off) {
        bail!("suggested shape doesn't resolve back to {insn:#x}");
    }
    Ok(Locator::Shape { insns, at })
}

/// Build a function anchor for the function at `addr` (VA or RVA) in a PE
/// binary:
///
/// - a short unique byte signature at its entry (`rekindle_sigmatch::short`,
///   ida-sigmaker's algorithm), and a second from its interior, starting where
///   the first ends, so the two cover different code and fail independently;
/// - with `source` (a whole-function pattern compiled from the target's
///   open-source dependency, e.g. gpui), its shortest window that's still
///   unique at the right place (`trim_unique`), clear of the entry
///   signature's bytes. It is the locator that finds this function in a new build
///   before any address is known;
/// - a `string-ref` for each string the function loads by RIP-relative
///   address that resolves uniquely back to this function (strings usually
///   outlive the code around them), up to `max_strings`.
///
/// Every locator is checked against `binary` before it's kept.
pub fn suggest(
    binary: &[u8],
    name: &str,
    addr: u64,
    max_strings: usize,
    source: Option<&Signature>,
    equivalent: &[u64],
) -> Result<Anchor> {
    // With `equivalent` functions the signatures may match them too (and the
    // anchor is `any_of`); otherwise they must be unique.
    let unique_signature =
        |pe: &PeImage, a: u64, o: &ShortOptions| signature_among(pe, a, equivalent, o);
    let pe = PeImage::parse(binary)?;
    if pe.machine == Machine::Aarch64 {
        if !equivalent.is_empty() || source.is_some() {
            bail!("{name}: equivalent functions and source patterns are x64 only");
        }
        return suggest_a64(binary, name, addr, max_strings);
    }
    let rva = pe.to_rva(addr)?;
    let (begin, _) = pe
        .function_containing(rva)
        .with_context(|| format!("{addr:#x} is in no .pdata function"))?;
    let index = BinaryIndex::new(binary);
    let mut locators = Vec::new();
    // Field offsets kept literal is the fallback when the shape alone recurs.
    // At least as many fixed bytes as `trim_unique`'s floor, for the same
    // reason: a barely-unique pattern can match uniquely elsewhere later.
    let plain = ShortOptions {
        min_fixed: 12,
        ..Default::default()
    };
    let keep_displ = ShortOptions {
        wildcard: OperandKinds::keeping("displ")?,
        min_fixed: 12,
        ..Default::default()
    };
    // Siblings that differ only in an immediate (e.g. the memcpy size of a
    // generic instantiated for different types) need immediates kept too.
    let keep_displ_imm = ShortOptions {
        wildcard: OperandKinds::keeping("displ,imm")?,
        min_fixed: 12,
        ..Default::default()
    };
    let entry_sig = unique_signature(&pe, u64::from(begin), &plain)
        .or_else(|_| unique_signature(&pe, u64::from(begin), &keep_displ))
        .or_else(|_| unique_signature(&pe, u64::from(begin), &keep_displ_imm));
    let entry_len = match &entry_sig {
        Ok(sig) => {
            locators.push(Locator::Bytes {
                pattern: sig.to_pattern(),
            });
            sig.len()
        }
        Err(e) => {
            eprintln!("{name}: no unique entry signature ({e})");
            0
        }
    };
    let begin_va = pe.image_base + u64::from(begin);
    match interior_signature(&pe, begin_va, entry_len, equivalent, &plain)
        .or_else(|_| interior_signature(&pe, begin_va, entry_len, equivalent, &keep_displ))
        .or_else(|_| interior_signature(&pe, begin_va, entry_len, equivalent, &keep_displ_imm))
    {
        Ok((_, sig)) => locators.push(Locator::Bytes {
            pattern: sig.to_pattern(),
        }),
        Err(e) => eprintln!("{name}: no unique interior signature ({e})"),
    }
    if let Some(src) = source {
        match trim_unique(&pe, src, 12, entry_len) {
            Ok((_, w)) => locators.push(Locator::Bytes {
                pattern: w.to_pattern(),
            }),
            Err(e) => eprintln!("{name}: source pattern not usable here ({e})"),
        }
    }
    if let Ok((_, scope)) = function_at(&pe, begin) {
        locators.extend(string_refs(&pe, &index, begin, scope, max_strings));
    }
    if locators.is_empty() {
        bail!("{name}: no locator resolves uniquely to {addr:#x}");
    }
    Ok(Anchor {
        name: name.to_string(),
        note: String::new(),
        function: true,
        any_of: !equivalent.is_empty(),
        within: None,
        operand: None,
        relative_to: None,
        plus: None,
        source: None,
        locators,
    })
}

/// The function containing `rva`: its first RVA, and the file range of its
/// code.
fn function_at(pe: &PeImage, rva: u32) -> Result<(u32, (usize, usize))> {
    let (begin, end) = pe
        .function_containing(rva)
        .with_context(|| format!("RVA {rva:#x} is in no .pdata function"))?;
    let start = pe.rva_to_offset(begin).context("function start")?;
    let stop = pe.rva_to_offset(end - 1).context("function end")? + 1;
    if stop > pe.bytes.len() {
        bail!("the function at RVA {begin:#x} runs past the end of the file");
    }
    Ok((begin, (start, stop)))
}

/// Up to `max` `string-ref` locators for the strings the function at RVA
/// `begin` (its code at `scope` in the file) loads by RIP-relative address, each one that resolves back to that
/// function alone.
fn string_refs(
    pe: &PeImage,
    index: &BinaryIndex,
    begin: u32,
    scope: (usize, usize),
    max: usize,
) -> Vec<Locator> {
    let binary = pe.bytes;
    let code = &binary[scope.0..scope.1];
    let want = index.status_from(vec![scope.0]);
    let mut d = Decoder::with_ip(
        64,
        code,
        pe.image_base + u64::from(begin),
        DecoderOptions::NONE,
    );
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    while d.can_decode() && out.len() < max {
        let ins = d.decode();
        if ins.is_invalid() || !ins.is_ip_rel_memory_operand() {
            continue;
        }
        let target = ins.ip_rel_memory_address();
        let Some(off) = pe.to_rva(target).ok().and_then(|r| pe.rva_to_offset(r)) else {
            continue;
        };
        // A printable UTF-8 run at the target (Rust strs aren't NUL-terminated;
        // the linker may merge neighbours, so cap the run).
        let run: Vec<u8> = binary
            .get(off..)
            .unwrap_or_default()
            .iter()
            .take(48)
            .take_while(|&&b| (0x20..0x7f).contains(&b))
            .copied()
            .collect();
        if run.len() < 6 {
            continue;
        }
        // Without a length immediate there's no telling where the literal
        // ends (merged neighbours), so don't guess.
        let at = (ins.next_ip() - (pe.image_base + u64::from(begin))) as usize;
        let Some(n) = literal_len(&code[at.min(code.len())..], ins.next_ip(), run.len()) else {
            continue;
        };
        let text = String::from_utf8_lossy(&run[..n]).into_owned();
        let loc = Locator::StringRef { text: text.clone() };
        if resolve_locator(&loc, index, None, &HashMap::new()) == want && seen.insert(text) {
            out.push(loc);
        }
    }
    out
}

/// The length of a `&str` literal whose address was just loaded, at most
/// `max`: rustc loads a `&str`'s length as a small immediate within the
/// next few instructions of `code` (which starts at `ip`).
fn literal_len(code: &[u8], ip: u64, max: usize) -> Option<usize> {
    let mut look = Decoder::with_ip(64, code, ip, DecoderOptions::NONE);
    (0..4).find_map(|_| {
        let n = look.decode();
        let is_mov_imm = n.mnemonic() == iced_x86::Mnemonic::Mov
            && n.op_count() == 2
            && n.op0_kind() == iced_x86::OpKind::Register
            && matches!(
                n.op1_kind(),
                iced_x86::OpKind::Immediate8
                    | iced_x86::OpKind::Immediate16
                    | iced_x86::OpKind::Immediate32
                    | iced_x86::OpKind::Immediate32to64
            );
        is_mov_imm
            .then(|| n.immediate(1) as usize)
            .filter(|&l| l >= 4 && l <= max)
    })
}

/// The fewest fixed bytes an AArch64 signature carries, so it stays unique
/// in a build that adds code like it.
const A64_MIN_FIXED: usize = 12;
/// The longest AArch64 signature tried, in bytes.
const A64_MAX_LEN: usize = 96;

/// [`suggest`] for AArch64: masked byte signatures at the function's entry
/// and inside it, the exports at its entry, and the strings and panic
/// locations it loads, each checked to resolve back to it alone.
fn suggest_a64(binary: &[u8], name: &str, addr: u64, max_strings: usize) -> Result<Anchor> {
    let index = BinaryIndex::new(binary);
    let off = index
        .offset_of_va(addr)
        .with_context(|| format!("{addr:#x} is not in the file"))?;
    let (start, end) = index
        .function_range(off)
        .with_context(|| format!("{addr:#x} is in no .eh_frame function"))?;
    let want = index.status_from(vec![start]);
    let mut locators = Vec::new();
    let mut names: Vec<&String> = index
        .exports
        .iter()
        .filter(|(_, offs)| offs.contains(&start))
        .map(|(n, _)| n)
        .collect();
    names.sort();
    locators.extend(
        names
            .into_iter()
            .map(|n| Locator::Export { name: n.clone() }),
    );
    let entry = a64_signature(&index, start, end);
    let entry_len = entry.as_ref().map_or(0, Signature::len);
    if let Some(sig) = entry {
        locators.push(Locator::Bytes {
            pattern: sig.to_pattern(),
        });
    } else {
        eprintln!("{name}: no unique entry signature");
    }
    match (start + entry_len..end)
        .step_by(16)
        .take(16)
        .find_map(|at| a64_signature(&index, at, end))
    {
        Some(sig) => locators.push(Locator::Bytes {
            pattern: sig.to_pattern(),
        }),
        None => eprintln!("{name}: no unique interior signature"),
    }
    let va = index.va_of(start).context("function not in a section")?;
    let code = &binary[start..end];
    let mut seen = BTreeSet::new();
    let mut strings = 0;
    for (target, len) in a64::str_loads(code, va, 48) {
        if strings >= max_strings {
            break;
        }
        let (Some(len), Some(at)) = (len, index.offset_of_va(target)) else {
            continue;
        };
        let Some(text) = binary
            .get(at..at + len as usize)
            .and_then(|b| std::str::from_utf8(b).ok())
            .filter(|t| t.chars().all(|c| !c.is_control()))
        else {
            continue;
        };
        let loc = Locator::StringRef { text: text.into() };
        if seen.insert(text.to_string())
            && resolve_locator(&loc, &index, None, &HashMap::new()) == want
        {
            locators.push(loc);
            strings += 1;
        }
    }
    for (_, r) in a64::refs(code, va) {
        let a64::Ref::Data(target) = r else { continue };
        let Some((file, line, col)) = index.panic_location_at(target) else {
            continue;
        };
        let loc = Locator::PanicLocation { file, line, col };
        if !locators.contains(&loc) && resolve_locator(&loc, &index, None, &HashMap::new()) == want
        {
            locators.push(loc);
        }
    }
    if locators.is_empty() {
        bail!("{name}: no locator resolves uniquely to {addr:#x}");
    }
    Ok(Anchor {
        name: name.to_string(),
        function: true,
        locators,
        ..Anchor::default()
    })
}

/// The shortest masked signature starting at file offset `at`, inside the
/// function ending at `end`, that matches only there in the binary's code.
fn a64_signature(index: &BinaryIndex, at: usize, end: usize) -> Option<Signature> {
    let va = index.va_of(at)?;
    let code_end = end.min(at + A64_MAX_LEN);
    (16..=code_end.saturating_sub(at))
        .step_by(4)
        .map(|len| a64::masked(&index.bytes[at..at + len], va))
        .filter(|sig| a64::fixed(sig) >= A64_MIN_FIXED)
        .find(|sig| {
            let hits: Vec<usize> = index
                .exec
                .iter()
                .flat_map(|&(s, e)| sig.scan(&index.bytes[s..e]).into_iter().map(move |o| s + o))
                .take(2)
                .collect();
            hits == [at]
        })
}
