//! A binary prepared for resolving anchors against it: its sections,
//! symbols and functions, and the references between them, each built once
//! and on first use.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind};
use object::{Object, ObjectSection, ObjectSymbol};

use rekindle_analysis::sections::SectionMap;
use rekindle_sigmatch::short::{Machine, PeImage};

use crate::a64;
use crate::descriptor::{Operand, file_key};
use crate::resolve::Status;
use crate::shape;

/// How many instructions after `mov reg, [rip+slot]` a `call reg` still
/// counts as calling through that slot.
const LOAD_REACH: usize = 8;

/// The longest x64 instruction, in bytes.
const MAX_INSN: usize = 15;

pub(crate) struct BinaryIndex<'a> {
    pub(crate) bytes: &'a [u8],
    /// The PE view (sections and `.pdata` functions), when it is one.
    pub(crate) pe: Option<PeImage<'a>>,
    /// Reference index, built on first use (see [`BinaryIndex::xrefs`]).
    xrefs: OnceCell<Xrefs>,
    /// Hits of whole-binary shapes by (pattern, at), found together in one
    /// pass over the image ([`BinaryIndex::prime_shapes`]).
    whole_shapes: RefCell<HashMap<(String, usize), Vec<usize>>>,
    /// Panic location records by (file key, line, col): their VAs.
    panic_records: OnceCell<HashMap<(String, u32, u32), Vec<u64>>>,
    sections: Option<SectionMap>,
    /// Executable file ranges. Empty for a container that doesn't parse,
    /// whose whole file is scanned instead.
    pub(crate) exec: Vec<(usize, usize)>,
    /// Symbol name -> file offsets of its definitions.
    pub(crate) symbols: HashMap<String, Vec<usize>>,
    /// ELF dynamic symbol (export) name -> file offsets of its definitions.
    pub(crate) exports: HashMap<String, Vec<usize>>,
    /// ELF relative relocations, plain or packed (`APS2`): slot VA -> the
    /// VA written there at load.
    pub(crate) relative: HashMap<u64, u64>,
}

/// Every RIP-relative memory reference and every call in the image: target
/// VA -> entries (file offsets) of the functions that make it.
#[derive(Default)]
struct Xrefs {
    rip: HashMap<u64, Vec<usize>>,
    calls: HashMap<u64, Vec<usize>>,
}

impl<'a> BinaryIndex<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        let mut exec = Vec::new();
        let mut symbols: HashMap<String, Vec<usize>> = HashMap::new();
        let mut exports: HashMap<String, Vec<usize>> = HashMap::new();
        let mut relative = HashMap::new();
        if let Ok(obj) = object::File::parse(bytes) {
            for sec in obj.sections() {
                if sec.kind() == object::SectionKind::Text
                    && let Some((off, size)) = sec.file_range()
                {
                    let (off, size) = (off as usize, size as usize);
                    if off.checked_add(size).is_some_and(|end| end <= bytes.len()) {
                        exec.push((off, off + size));
                    }
                }
            }
            // Release binaries are usually stripped of these. Their symbol
            // locators then go missing and the anchor's other locators carry
            // it, which is the redundancy anchors are designed around.
            for sym in obj.symbols().filter(ObjectSymbol::is_definition) {
                if let Ok(name) = sym.name()
                    && !name.is_empty()
                    && let Some(off) = addr_to_offset(&obj, sym.address())
                {
                    symbols.entry(name.to_string()).or_default().push(off);
                }
            }
            if obj.format() == object::BinaryFormat::Elf {
                for sym in obj.dynamic_symbols().filter(ObjectSymbol::is_definition) {
                    if let Ok(name) = sym.name()
                        && !name.is_empty()
                        && let Some(off) = addr_to_offset(&obj, sym.address())
                    {
                        exports.entry(name.to_string()).or_default().push(off);
                    }
                }
            }
            relative = rekindle_analysis::relocs::relative(&obj);
        }
        for v in symbols.values_mut().chain(exports.values_mut()) {
            v.sort_unstable();
            v.dedup();
        }
        Self {
            bytes,
            pe: PeImage::parse(bytes).ok(),
            xrefs: OnceCell::new(),
            whole_shapes: RefCell::default(),
            panic_records: OnceCell::new(),
            sections: SectionMap::parse(bytes).ok(),
            exec,
            symbols,
            exports,
            relative,
        }
    }

    /// Whether the image's code is AArch64, which `a64` reads, rather than
    /// x64, which iced-x86 does.
    pub(crate) fn is_a64(&self) -> bool {
        self.pe.as_ref().is_some_and(|pe| pe.machine == Machine::Aarch64)
    }

    /// The VA of file offset `off`.
    pub(crate) fn va_of(&self, off: usize) -> Option<u64> {
        let pe = self.pe.as_ref()?;
        Some(pe.image_base + u64::from(pe.offset_to_rva(off)?))
    }

    /// The file offset of `va`.
    pub(crate) fn offset_of_va(&self, va: u64) -> Option<usize> {
        let pe = self.pe.as_ref()?;
        pe.rva_to_offset(pe.to_rva(va).ok()?)
    }

    /// The 8-byte word the file holds at `va`.
    fn word_at(&self, va: u64) -> Option<u64> {
        let off = self.offset_of_va(va)?;
        let b = self.bytes.get(off..off + 8)?;
        Some(u64::from_le_bytes(b.try_into().ok()?))
    }

    /// The pointer at `va` once loaded at the image's base: a relative
    /// relocation's addend on ELF, the word itself on PE.
    fn pointer_at(&self, va: u64) -> Option<u64> {
        if let Some(&p) = self.relative.get(&va) {
            return Some(p);
        }
        let pe = self.pe.as_ref()?;
        let p = self.word_at(va)?;
        (self.relative.is_empty() && pe.to_rva(p).is_ok_and(|r| pe.rva_to_offset(r).is_some()))
            .then_some(p)
    }

    /// The VAs of pointer-aligned words that point at `target` once loaded.
    fn pointers_to(&self, target: u64) -> Vec<u64> {
        if !self.relative.is_empty() {
            return self
                .relative
                .iter()
                .filter(|&(_, &v)| v == target)
                .map(|(&slot, _)| slot)
                .collect();
        }
        let Some(pe) = &self.pe else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for s in pe.sections.iter().filter(|s| !s.executable) {
            let start = s.rva.next_multiple_of(8);
            let end = s.rva + s.raw_size;
            for rva in (start..end.saturating_sub(7)).step_by(8) {
                let va = pe.image_base + u64::from(rva);
                if self.word_at(va) == Some(target) {
                    out.push(va);
                }
            }
        }
        out
    }

    /// File offsets of the vtable slots that hold the function at file
    /// offset `func` as their `method`th method, in a vtable whose header
    /// matches `size` and `align` and whose earlier methods are pointers too.
    pub(crate) fn vtable_slots(
        &self,
        func: usize,
        method: usize,
        size: Option<u64>,
        align: Option<u64>,
    ) -> Vec<usize> {
        let Some(target) = self.va_of(func) else {
            return Vec::new();
        };
        let mut out: Vec<usize> = self
            .pointers_to(target)
            .into_iter()
            .filter(|&slot| {
                let Some(header) = slot.checked_sub(8 * (3 + method as u64)) else {
                    return false;
                };
                size.is_none_or(|s| self.word_at(header + 8) == Some(s))
                    && align.is_none_or(|a| self.word_at(header + 16) == Some(a))
                    && (0..method).all(|k| self.pointer_at(header + 24 + 8 * k as u64).is_some())
            })
            .filter_map(|slot| self.offset_of_va(slot))
            .collect();
        out.sort_unstable();
        out
    }

    /// The AArch64 `bl` calls in file range `[start, end)`: (site, target).
    pub(crate) fn a64_calls(&self, start: usize, end: usize) -> Vec<(u64, u64)> {
        self.a64_refs(start, end)
            .into_iter()
            .filter_map(|(site, r)| match r {
                a64::Ref::Call(t) => Some((site, t)),
                a64::Ref::Data(_) => None,
            })
            .collect()
    }

    /// The AArch64 references in file range `[start, end)`.
    fn a64_refs(&self, start: usize, end: usize) -> Vec<(u64, a64::Ref)> {
        match (self.va_of(start), self.bytes.get(start..end)) {
            (Some(va), Some(code)) => a64::refs(code, va),
            _ => Vec::new(),
        }
    }

    /// File range `[start, end)` of the `.pdata` function containing `off`.
    pub(crate) fn function_range(&self, off: usize) -> Option<(usize, usize)> {
        let pe = self.pe.as_ref()?;
        let (begin, end) = pe.function_containing(pe.offset_to_rva(off)?)?;
        Some((pe.rva_to_offset(begin)?, pe.rva_to_offset(end - 1)? + 1))
    }

    /// File offset of the entry of the function containing file offset `off`.
    pub(crate) fn function_entry(&self, off: usize) -> Option<usize> {
        let pe = self.pe.as_ref()?;
        let (begin, _) = pe.function_containing(pe.offset_to_rva(off)?)?;
        pe.rva_to_offset(begin)
    }

    /// File ranges of every `.pdata` function.
    pub(crate) fn function_ranges(&self) -> Vec<(usize, usize)> {
        let Some(pe) = &self.pe else {
            return Vec::new();
        };
        pe.functions
            .iter()
            .filter_map(|&(b, e)| Some((pe.rva_to_offset(b)?, pe.rva_to_offset(e - 1)? + 1)))
            .collect()
    }

    /// The `operand` of the instruction at file offset `off`.
    pub(crate) fn operand_value(&self, off: usize, operand: Operand) -> Option<i64> {
        if self.is_a64() {
            return None;
        }
        let pe = self.pe.as_ref()?;
        let ip = pe.image_base + u64::from(pe.offset_to_rva(off)?);
        let code = self
            .bytes
            .get(off..(off + MAX_INSN).min(self.bytes.len()))?;
        let ins = Decoder::with_ip(64, code, ip, DecoderOptions::NONE).decode();
        if ins.is_invalid() {
            return None;
        }
        operand_of(&ins, operand)
    }

    /// File offset of the target of the near `call` at file offset `off`.
    pub(crate) fn call_target(&self, off: usize) -> Option<usize> {
        self.first_call_target(off, 1)
    }

    /// File offset of the target of the first near `call` starting in
    /// `[off, off + len)`.
    pub(crate) fn first_call_target(&self, off: usize, len: usize) -> Option<usize> {
        if self.is_a64() {
            let end = off + len.next_multiple_of(4);
            return self
                .a64_refs(off, end)
                .into_iter()
                .find_map(|(_, r)| match r {
                    a64::Ref::Call(t) => self.offset_of_va(t),
                    a64::Ref::Data(_) => None,
                });
        }
        let pe = self.pe.as_ref()?;
        let ip = pe.image_base + u64::from(pe.offset_to_rva(off)?);
        // Enough to decode a call through a slot loaded LOAD_REACH
        // instructions before it.
        let reach = len + MAX_INSN * (LOAD_REACH + 1);
        let code = self.bytes.get(off..(off + reach).min(self.bytes.len()))?;
        let insns = Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
            .into_iter()
            .take_while(|i| !i.is_invalid());
        let (_, target) = self
            .calls(insns)
            .into_iter()
            .find(|&(site, _)| site < ip + len as u64)?;
        pe.rva_to_offset(pe.to_rva(target).ok()?)
    }

    /// Each call as (site, target), through GOT slots too: `call [rip+slot]`,
    /// or `call reg` after `mov reg, [rip+slot]`, whose site is the load.
    pub(crate) fn calls(&self, insns: impl IntoIterator<Item = Instruction>) -> Vec<(u64, u64)> {
        /// A `mov reg, [rip+slot]` a later `call reg` may call through.
        struct Load {
            reg: iced_x86::Register,
            slot: u64,
            site: u64,
            /// Instructions since the load.
            age: usize,
        }
        let mut out = Vec::new();
        let mut loaded: Option<Load> = None;
        for ins in insns {
            if let Some(target) = direct_call(&ins) {
                out.push((ins.ip(), target));
            } else if ins.is_call_near_indirect() {
                let slot = if ins.op0_kind() == OpKind::Memory && ins.is_ip_rel_memory_operand() {
                    Some((ins.ip(), ins.ip_rel_memory_address()))
                } else {
                    loaded
                        .as_ref()
                        .filter(|l| {
                            ins.op0_kind() == OpKind::Register && ins.op0_register() == l.reg
                        })
                        .map(|l| (l.site, l.slot))
                };
                if let Some((site, slot)) = slot
                    && let Some(&target) = self.relative.get(&slot)
                {
                    out.push((site, target));
                }
            }
            loaded = if ins.mnemonic() == Mnemonic::Mov
                && ins.op0_kind() == OpKind::Register
                && ins.op1_kind() == OpKind::Memory
                && ins.is_ip_rel_memory_operand()
            {
                Some(Load {
                    reg: ins.op0_register().full_register(),
                    slot: ins.ip_rel_memory_address(),
                    site: ins.ip(),
                    age: 0,
                })
            } else {
                // A call, a write to the register or LOAD_REACH instructions
                // end what the load can explain.
                let overwritten = |l: &Load| {
                    ins.op0_kind() == OpKind::Register
                        && ins.op0_register().full_register() == l.reg
                };
                loaded
                    .filter(|l| {
                        !ins.is_call_near_indirect() && !overwritten(l) && l.age < LOAD_REACH
                    })
                    .map(|l| Load {
                        age: l.age + 1,
                        ..l
                    })
            };
        }
        out
    }

    /// Entries of the functions that load the address of any occurrence of
    /// `needle` with a RIP-relative operand.
    pub(crate) fn string_refs(&self, needle: &[u8]) -> Vec<usize> {
        let Some(pe) = &self.pe else {
            return Vec::new();
        };
        let refs = &self.xrefs().rip;
        find_all(self.bytes, needle)
            .into_iter()
            .filter_map(|o| pe.offset_to_rva(o))
            .flat_map(|r| {
                refs.get(&(pe.image_base + u64::from(r)))
                    .cloned()
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The reference index, built by decoding each `.pdata` function once.
    fn xrefs(&self) -> &Xrefs {
        self.xrefs.get_or_init(|| {
            let mut xrefs = Xrefs::default();
            let Some(pe) = &self.pe else { return xrefs };
            let add = |map: &mut HashMap<u64, Vec<usize>>, target: u64, entry: usize| {
                let v = map.entry(target).or_default();
                if v.last() != Some(&entry) {
                    v.push(entry);
                }
            };
            if pe.machine == Machine::Aarch64 {
                for (start, end) in self.function_ranges() {
                    for (_, r) in self.a64_refs(start, end) {
                        match r {
                            a64::Ref::Data(t) => add(&mut xrefs.rip, t, start),
                            a64::Ref::Call(t) => add(&mut xrefs.calls, t, start),
                        }
                    }
                }
                return xrefs;
            }
            for &(begin, end) in &pe.functions {
                let Some((start, d)) = self.decoder(begin, end) else {
                    continue;
                };
                let insns: Vec<Instruction> = d.into_iter().collect();
                for target in insns
                    .iter()
                    .filter(|i| i.is_ip_rel_memory_operand())
                    .map(Instruction::ip_rel_memory_address)
                {
                    add(&mut xrefs.rip, target, start);
                }
                for (_, target) in self.calls(insns) {
                    add(&mut xrefs.calls, target, start);
                }
            }
            xrefs
        })
    }

    /// A decoder over the `.pdata` function `[begin, end)`, and its entry offset.
    pub(crate) fn decoder(&self, begin: u32, end: u32) -> Option<(usize, Decoder<'a>)> {
        let pe = self.pe.as_ref().filter(|pe| pe.machine == Machine::X86_64)?;
        let (start, last) = (pe.rva_to_offset(begin)?, pe.rva_to_offset(end - 1)?);
        let code = self.bytes.get(start..=last)?;
        let ip = pe.image_base + u64::from(begin);
        Some((start, Decoder::with_ip(64, code, ip, DecoderOptions::NONE)))
    }

    /// Entries of the distinct functions directly called by the function
    /// containing `off`, in the order their first call appears.
    pub(crate) fn callees(&self, off: usize) -> Vec<usize> {
        let Some(pe) = &self.pe else {
            return Vec::new();
        };
        if pe.machine == Machine::Aarch64 {
            let Some((start, end)) = self.function_range(off) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for (_, r) in self.a64_refs(start, end) {
                if let a64::Ref::Call(t) = r
                    && let Some(o) = self.offset_of_va(t)
                    && !out.contains(&o)
                {
                    out.push(o);
                }
            }
            return out;
        }
        let Some((_, d)) = pe
            .offset_to_rva(off)
            .and_then(|r| pe.function_containing(r))
            .and_then(|(begin, end)| self.decoder(begin, end))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (_, target) in self.calls(d) {
            if let Some(o) = pe.to_rva(target).ok().and_then(|r| pe.rva_to_offset(r))
                && !out.contains(&o)
            {
                out.push(o);
            }
        }
        out
    }

    /// Entries of the functions that directly call the function containing `off`.
    pub(crate) fn callers(&self, off: usize) -> Vec<usize> {
        let Some(pe) = &self.pe else {
            return Vec::new();
        };
        let entry = self.function_entry(off).unwrap_or(off);
        let Some(rva) = pe.offset_to_rva(entry) else {
            return Vec::new();
        };
        self.xrefs()
            .calls
            .get(&(pe.image_base + u64::from(rva)))
            .cloned()
            .unwrap_or_default()
    }

    /// The decoded instructions of file range `[start, end)`.
    pub(crate) fn instructions(&self, start: usize, end: usize) -> Vec<Instruction> {
        let (Some(pe), Some(code)) = (self.pe.as_ref(), self.bytes.get(start..end)) else {
            return Vec::new();
        };
        // Shapes are x64 instructions; AArch64 code has none.
        if pe.machine != Machine::X86_64 {
            return Vec::new();
        }
        let Some(rva) = pe.offset_to_rva(start) else {
            return Vec::new();
        };
        let ip = pe.image_base + u64::from(rva);
        Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
            .into_iter()
            .collect()
    }

    /// The decoded instructions of file range `[start, end)`: their file
    /// offsets and shapes.
    pub(crate) fn shapes(&self, start: usize, end: usize) -> (Vec<usize>, Vec<shape::Insn>) {
        let raw = self.instructions(start, end);
        let Some(first) = raw.first().map(Instruction::ip) else {
            return Default::default();
        };
        raw.iter()
            .map(|i| (offset_of_ip(start, first, i.ip()), shape::of(i)))
            .unzip()
    }

    /// Where a shape matches: the file offset of its `at`th instruction, per
    /// match, inside `scope` or else in every `.pdata` function.
    pub(crate) fn shape_hits(
        &self,
        pattern: &str,
        at: usize,
        scope: Option<(usize, usize)>,
    ) -> Vec<usize> {
        if scope.is_none()
            && let Some(hits) = self.whole_shapes.borrow().get(&(pattern.to_string(), at))
        {
            return hits.clone();
        }
        let Ok(pat) = shape::parse(pattern) else {
            return Vec::new();
        };
        if at >= pat.len() {
            return Vec::new();
        }
        let ranges = match scope {
            Some(r) => vec![r],
            None => self.function_ranges(),
        };
        let mut memo = HashMap::new();
        let mut out = Vec::new();
        for (start, end) in ranges {
            out.extend(self.shape_hits_in(&pat, at, start, end, &mut memo));
        }
        out
    }

    /// Find every whole-binary shape of `shapes` (pattern, at) in one pass
    /// over the image, decoding each function once, for [`Self::shape_hits`].
    /// Each shape alone would decode the whole image again.
    pub(crate) fn prime_shapes(&self, shapes: &[(&str, usize)]) {
        let parsed: Vec<((String, usize), Vec<shape::Insn>)> = shapes
            .iter()
            .filter_map(|&(p, at)| {
                let pat = shape::parse(p).ok().filter(|pat| at < pat.len())?;
                Some(((p.to_string(), at), pat))
            })
            .collect();
        if parsed.is_empty() {
            return;
        }
        let mut memos: Vec<shape::Mnemonics> = vec![HashMap::new(); parsed.len()];
        let mut hits: Vec<Vec<usize>> = vec![Vec::new(); parsed.len()];
        for (start, end) in self.function_ranges() {
            let raw = self.instructions(start, end);
            let Some(first) = raw.first().map(Instruction::ip) else {
                continue;
            };
            for (k, ((_, at), pat)) in parsed.iter().enumerate() {
                hits[k].extend(
                    shape::find_raw(pat, &raw, &mut memos[k])
                        .into_iter()
                        .map(|i| offset_of_ip(start, first, raw[i + at].ip())),
                );
            }
        }
        let mut cache = self.whole_shapes.borrow_mut();
        for ((key, _), h) in parsed.into_iter().zip(hits) {
            cache.insert(key, h);
        }
    }

    /// Where `pat` matches in file range `[start, end)`: the file offset of
    /// its `at`th instruction, per match.
    fn shape_hits_in(
        &self,
        pat: &[shape::Insn],
        at: usize,
        start: usize,
        end: usize,
        memo: &mut shape::Mnemonics,
    ) -> Vec<usize> {
        let raw = self.instructions(start, end);
        let Some(first) = raw.first().map(Instruction::ip) else {
            return Vec::new();
        };
        shape::find_raw(pat, &raw, memo)
            .into_iter()
            .map(|i| offset_of_ip(start, first, raw[i + at].ip()))
            .collect()
    }

    /// Panic location records by (file key, line, col): their VAs.
    fn panic_records(&self) -> &HashMap<(String, u32, u32), Vec<u64>> {
        self.panic_records.get_or_init(|| {
            let mut m: HashMap<(String, u32, u32), Vec<u64>> = HashMap::new();
            for (va, l) in rekindle_analysis::locations::records(self.bytes) {
                m.entry((file_key(&l.file), l.line, l.col))
                    .or_default()
                    .push(va);
            }
            m
        })
    }

    /// The panic location record at `va`, as (file key, line, col).
    pub(crate) fn panic_location_at(&self, va: u64) -> Option<(String, u32, u32)> {
        self.panic_records()
            .iter()
            .find(|(_, vas)| vas.contains(&va))
            .map(|(k, _)| k.clone())
    }

    /// Entries of the functions that load a panic location record of `file`,
    /// `line` and `col` (by a RIP-relative operand).
    pub(crate) fn panic_refs(&self, file: &str, line: u32, col: u32) -> Vec<usize> {
        let records = self.panic_records();
        let refs = &self.xrefs().rip;
        let mut out: Vec<usize> = records
            .get(&(file_key(file), line, col))
            .into_iter()
            .flatten()
            .flat_map(|va| refs.get(va).cloned().unwrap_or_default())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The status of a locator that found `offsets`: unique, ambiguous or
    /// missing.
    pub(crate) fn status_from(&self, mut offsets: Vec<usize>) -> Status {
        offsets.sort_unstable();
        offsets.dedup();
        match offsets[..] {
            [] => Status::Missing,
            [offset] => Status::Unique {
                offset,
                section: self
                    .sections
                    .as_ref()
                    .and_then(|m| m.locate(offset))
                    .map(str::to_string),
            },
            _ => Status::Ambiguous { offsets },
        }
    }
}

/// File offset of the instruction at `ip` in a run decoded from `start`,
/// whose first instruction is at `first_ip`.
fn offset_of_ip(start: usize, first_ip: u64, ip: u64) -> usize {
    start + (ip - first_ip) as usize
}

fn direct_call(ins: &Instruction) -> Option<u64> {
    (ins.is_call_near() && ins.op0_kind() == OpKind::NearBranch64).then(|| ins.near_branch_target())
}

/// Map a virtual address to a file offset using the section table.
fn addr_to_offset(obj: &object::File, addr: u64) -> Option<usize> {
    obj.sections().find_map(|sec| {
        let delta = addr.checked_sub(sec.address())?;
        if delta >= sec.size() {
            return None;
        }
        let (file_off, file_len) = sec.file_range()?;
        (delta < file_len).then(|| (file_off + delta) as usize)
    })
}

/// The value of `operand` in `ins`, if it has one. A RIP-relative operand's
/// displacement is an address, not a value, so it has none.
pub(crate) fn operand_of(ins: &Instruction, operand: Operand) -> Option<i64> {
    let mut kinds = (0..ins.op_count()).map(|i| (i, ins.op_kind(i)));
    match operand {
        Operand::Disp => (kinds.any(|(_, k)| k == OpKind::Memory)
            && !ins.is_ip_rel_memory_operand())
        .then(|| ins.memory_displacement64() as i64),
        Operand::Imm => kinds
            .find(|&(_, k)| {
                matches!(
                    k,
                    OpKind::Immediate8
                        | OpKind::Immediate16
                        | OpKind::Immediate32
                        | OpKind::Immediate64
                        | OpKind::Immediate8to16
                        | OpKind::Immediate8to32
                        | OpKind::Immediate8to64
                        | OpKind::Immediate32to64
                )
            })
            .map(|(i, _)| ins.immediate(i) as i64),
    }
}

/// Every offset where `needle` occurs in `hay`, overlapping matches included.
pub(crate) fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    hay.windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_follow_got_slots() {
        // mov rax,[rip+0x10]; mov edi,1; call rax; call [rip+0x20]; call rel32 0
        let code = [
            0x48, 0x8b, 0x05, 0x10, 0, 0, 0, 0xbf, 1, 0, 0, 0, 0xff, 0xd0, 0xff, 0x15, 0x20, 0, 0,
            0, 0xe8, 0, 0, 0, 0,
        ];
        let mut index = BinaryIndex::new(&[]);
        index.relative.insert(0x1017, 0x5000);
        index.relative.insert(0x1034, 0x6000);
        let insns = Decoder::with_ip(64, &code, 0x1000, DecoderOptions::NONE);
        assert_eq!(
            index.calls(insns),
            vec![(0x1000, 0x5000), (0x100e, 0x6000), (0x1014, 0x1019)]
        );
    }

    #[test]
    fn a_load_explains_a_call_only_until_its_register_is_written() {
        // mov rax,[rip+0x10]; mov eax,1; call rax
        let code = [
            0x48, 0x8b, 0x05, 0x10, 0, 0, 0, 0xb8, 1, 0, 0, 0, 0xff, 0xd0,
        ];
        let mut index = BinaryIndex::new(&[]);
        index.relative.insert(0x1017, 0x5000);
        let insns = Decoder::with_ip(64, &code, 0x1000, DecoderOptions::NONE);
        assert_eq!(index.calls(insns), vec![]);
    }

    #[test]
    fn find_all_includes_overlapping_matches() {
        assert_eq!(find_all(b"aaaa", b"aa"), [0, 1, 2]);
        assert_eq!(find_all(b"ab", b"abc"), Vec::<usize>::new());
        assert_eq!(find_all(b"ab", b""), Vec::<usize>::new());
    }
}
