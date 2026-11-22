//! Short unique signatures for a function in a binary: a Rust port of
//! [ida-sigmaker](https://github.com/mahmoudimus/ida-sigmaker)'s unique
//! generator, so rekindle can (re)generate them programmatically, without IDA.
//!
//! The algorithm is sigmaker's:
//!
//! 1. Decode instructions linearly from the function's first byte.
//! 2. Append each instruction's bytes. If it has a wildcardable operand
//!    (memory/displacement, immediate, near/far branch) at a non-zero offset,
//!    wildcard from the first such operand to the end of the instruction.
//! 3. After each instruction, stop as soon as the pattern matches exactly once
//!    in the image; trim trailing wildcards.
//! 4. Give up if the pattern would leave the function (its `.pdata` extent)
//!    first, unless [`ShortOptions::continue_outside`].
//!
//! Operand kinds can be kept as literal bytes ([`OperandKinds`]), sigmaker's
//! `--keep-operands`, for siblings that differ only in a field offset.
//!
//! Unlike the whole-function signatures `sigs` builds from a compiled crate,
//! these need no source: only the target binary and an address.

use anyhow::{Context, Result, anyhow, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind, Register};

use crate::sig::Signature;

// ---- a minimal PE view: sections and the .pdata function table ----

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub rva: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub executable: bool,
}

/// A PE32+ or x86-64 ELF image as laid out in its file: sections, and the
/// function extents from the exception directory (`.pdata` `RUNTIME_FUNCTION`s)
/// or `.eh_frame`, which x64 binaries carry even when stripped of symbols.
#[derive(Debug, Clone)]
pub struct PeImage<'a> {
    pub bytes: &'a [u8],
    pub image_base: u64,
    pub sections: Vec<Section>,
    /// `(begin_rva, end_rva)`, sorted by begin.
    pub functions: Vec<(u32, u32)>,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

impl<'a> PeImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.starts_with(b"\x7fELF") {
            return Self::parse_elf(bytes);
        }
        let bad = || anyhow!("not a PE32+ image");
        if bytes.get(0..2) != Some(b"MZ") {
            bail!(bad());
        }
        let nt = u32_at(bytes, 0x3c).ok_or_else(bad)? as usize;
        if u32_at(bytes, nt) != Some(0x0000_4550) {
            bail!(bad());
        }
        let n_sections = u16_at(bytes, nt + 6).ok_or_else(bad)? as usize;
        let opt_size = u16_at(bytes, nt + 20).ok_or_else(bad)? as usize;
        let opt = nt + 24;
        if u16_at(bytes, opt) != Some(0x20b) {
            bail!("only PE32+ (x64) images are supported");
        }
        let image_base = u64_at(bytes, opt + 24).ok_or_else(bad)?;
        let n_dirs = u32_at(bytes, opt + 108).ok_or_else(bad)? as usize;
        let mut sections = Vec::with_capacity(n_sections);
        let mut sh = opt + opt_size;
        for _ in 0..n_sections {
            let raw_name = bytes.get(sh..sh + 8).ok_or_else(bad)?;
            let name = String::from_utf8_lossy(raw_name)
                .trim_end_matches('\0')
                .to_string();
            let characteristics = u32_at(bytes, sh + 36).ok_or_else(bad)?;
            sections.push(Section {
                name,
                virtual_size: u32_at(bytes, sh + 8).ok_or_else(bad)?,
                rva: u32_at(bytes, sh + 12).ok_or_else(bad)?,
                raw_size: u32_at(bytes, sh + 16).ok_or_else(bad)?,
                raw_offset: u32_at(bytes, sh + 20).ok_or_else(bad)?,
                executable: characteristics & 0x2000_0000 != 0,
            });
            sh += 40;
        }
        let mut img = PeImage {
            bytes,
            image_base,
            sections,
            functions: Vec::new(),
        };
        // Exception directory = data directory 3.
        if n_dirs > 3 {
            let (rva, size) = (
                u32_at(bytes, opt + 112 + 3 * 8).unwrap_or(0),
                u32_at(bytes, opt + 112 + 3 * 8 + 4).unwrap_or(0),
            );
            if let Some(off) = img.rva_to_offset(rva) {
                for i in 0..(size as usize / 12) {
                    let e = off + i * 12;
                    if let (Some(b), Some(en)) = (u32_at(bytes, e), u32_at(bytes, e + 4))
                        && b != 0
                        && en > b
                    {
                        img.functions.push((b, en));
                    }
                }
            }
        }
        img.functions.sort_unstable();
        img.functions.dedup();
        Ok(img)
    }

    fn parse_elf(bytes: &'a [u8]) -> Result<Self> {
        use object::{Object, ObjectSection, ObjectSegment, SectionKind};
        let obj = object::File::parse(bytes).context("parsing the ELF image")?;
        if obj.architecture() != object::Architecture::X86_64 {
            bail!("only x86-64 ELF images are supported");
        }
        let image_base = obj.segments().map(|s| s.address()).min().unwrap_or(0);
        let rva = |addr: u64| u32::try_from(addr.wrapping_sub(image_base)).ok();
        let mut sections = Vec::new();
        for s in obj
            .sections()
            .filter(|s| s.address() >= image_base && s.address() != 0)
        {
            let (raw_offset, raw_size) = s.file_range().unwrap_or((0, 0));
            sections.push(Section {
                name: s.name().unwrap_or_default().to_string(),
                rva: rva(s.address()).context("section beyond 4 GiB")?,
                virtual_size: u32::try_from(s.size())?,
                raw_offset: u32::try_from(raw_offset)?,
                raw_size: u32::try_from(raw_size)?,
                executable: s.kind() == SectionKind::Text,
            });
        }
        let mut functions = Vec::new();
        if let Some(eh) = obj.section_by_name(".eh_frame") {
            use gimli::UnwindSection;
            let frame = gimli::EhFrame::new(eh.data()?, gimli::LittleEndian);
            let mut bases = gimli::BaseAddresses::default().set_eh_frame(eh.address());
            if let Some(text) = obj.section_by_name(".text") {
                bases = bases.set_text(text.address());
            }
            if let Some(got) = obj.section_by_name(".got") {
                bases = bases.set_got(got.address());
            }
            let mut entries = frame.entries(&bases);
            while let Ok(Some(entry)) = entries.next() {
                let gimli::CieOrFde::Fde(partial) = entry else {
                    continue;
                };
                let Ok(fde) = partial.parse(gimli::EhFrame::cie_from_offset) else {
                    continue;
                };
                if let (Some(b), Some(e)) = (rva(fde.initial_address()), rva(fde.end_address()))
                    && b != 0
                    && e > b
                {
                    functions.push((b, e));
                }
            }
        }
        functions.sort_unstable();
        functions.dedup();
        Ok(PeImage {
            bytes,
            image_base,
            sections,
            functions,
        })
    }

    /// The file offset of `rva`, if a section's file bytes hold it. A section
    /// maps `min(raw_size, virtual_size)` bytes of its file data, or all of
    /// it when `virtual_size` is 0; the rest of its raw data is padding.
    pub fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        self.sections.iter().find_map(|s| {
            let size = match s.virtual_size {
                0 => s.raw_size,
                v => s.raw_size.min(v),
            };
            let delta = rva.checked_sub(s.rva).filter(|&d| d < size)?;
            Some(s.raw_offset as usize + delta as usize)
        })
    }

    /// The RVA of the file offset `off`, if a section's raw data holds it.
    pub fn offset_to_rva(&self, off: usize) -> Option<u32> {
        let off = u32::try_from(off).ok()?;
        self.sections.iter().find_map(|s| {
            let delta = off.checked_sub(s.raw_offset).filter(|&d| d < s.raw_size)?;
            s.rva.checked_add(delta)
        })
    }

    /// Accept a VA (`>= image_base`) or an RVA.
    pub fn to_rva(&self, addr: u64) -> Result<u32> {
        let rva = if addr >= self.image_base {
            addr - self.image_base
        } else {
            addr
        };
        u32::try_from(rva).context("address out of range")
    }

    /// The `.pdata` function containing `rva`, as `(begin, end)`.
    pub fn function_containing(&self, rva: u32) -> Option<(u32, u32)> {
        let i = self.functions.partition_point(|&(b, _)| b <= rva);
        let (b, e) = *self.functions.get(i.checked_sub(1)?)?;
        (rva < e).then_some((b, e))
    }

    /// File byte ranges of every section (what a signature must be unique in).
    pub fn section_ranges(&self) -> Vec<(usize, usize)> {
        self.sections
            .iter()
            .filter(|s| s.raw_size > 0)
            .map(|s| {
                let start = s.raw_offset as usize;
                (start, start + s.raw_size as usize)
            })
            .filter(|&(_, end)| end <= self.bytes.len())
            .collect()
    }
}

// ---- operand kinds ----

/// Which operand kinds are wildcarded (sigmaker's x86 default: all of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperandKinds {
    /// Absolute or RIP-relative memory (`o_mem`).
    pub mem: bool,
    /// Base/index + displacement (`o_displ`): struct field offsets, stack slots.
    pub displ: bool,
    pub imm: bool,
    pub near: bool,
    pub far: bool,
}

impl Default for OperandKinds {
    fn default() -> Self {
        Self {
            mem: true,
            displ: true,
            imm: true,
            near: true,
            far: true,
        }
    }
}

impl OperandKinds {
    /// Wildcard everything except the comma-separated kinds in `keep`
    /// (`displ`, `imm`, `mem`, `near`, `far`), as sigmaker's `--keep-operands`.
    pub fn keeping(keep: &str) -> Result<Self> {
        let mut k = Self::default();
        for name in keep.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match name {
                "mem" => k.mem = false,
                "displ" => k.displ = false,
                "imm" => k.imm = false,
                "near" => k.near = false,
                "far" => k.far = false,
                "phrase" => {} // never has offset bytes on x86
                other => bail!("unknown operand kind `{other}` (displ, imm, mem, near, far)"),
            }
        }
        Ok(k)
    }
}

#[derive(Debug, Clone)]
pub struct ShortOptions {
    pub wildcard: OperandKinds,
    /// Let the pattern run past the function's end (more fragile).
    pub continue_outside: bool,
    /// Give up beyond this many bytes.
    pub max_len: usize,
    /// Keep growing until the pattern has at least this many fixed bytes, even
    /// once it's unique (0 = stop at the first unique pattern, as sigmaker).
    /// Very short unique patterns are unique by luck and can match uniquely in
    /// the wrong place in a later build.
    pub min_fixed: usize,
    /// Count matches only in this file range (e.g. one function) instead of
    /// every section: a pattern need only be unique where it will be
    /// searched for.
    pub scope: Option<(usize, usize)>,
}

impl Default for ShortOptions {
    fn default() -> Self {
        Self {
            wildcard: OperandKinds::default(),
            continue_outside: false,
            max_len: 1000,
            min_fixed: 0,
            scope: None,
        }
    }
}

/// Offset within `instr` at which wildcarding starts, if any: the first
/// wildcardable operand with a non-zero offset (sigmaker ignores offset 0).
fn wildcard_start(decoder: &Decoder, instr: &Instruction, kinds: OperandKinds) -> Option<usize> {
    let co = decoder.get_constant_offsets(instr);
    let mut starts = Vec::new();
    if co.has_displacement() {
        let absolute = instr.memory_base() == Register::None || instr.is_ip_rel_memory_operand();
        if (absolute && kinds.mem) || (!absolute && kinds.displ) {
            starts.push(co.displacement_offset());
        }
    }
    let branch = (0..instr.op_count()).find_map(|i| match instr.op_kind(i) {
        OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64 => Some(kinds.near),
        OpKind::FarBranch16 | OpKind::FarBranch32 => Some(kinds.far),
        _ => None,
    });
    let imm_allowed = branch.unwrap_or(kinds.imm);
    if co.has_immediate() && imm_allowed {
        starts.push(co.immediate_offset());
    }
    if co.has_immediate2() && kinds.imm {
        starts.push(co.immediate_offset2());
    }
    // iced reports a relative branch's displacement as neither; it's the
    // instruction's tail.
    if let Some(true) = branch
        && !co.has_immediate()
    {
        let size = match instr.op_kind(0) {
            OpKind::NearBranch16 => 2,
            _ if instr.len() <= 2 => 1,
            _ => 4,
        };
        starts.push(instr.len() - size.min(instr.len()));
    }
    starts.into_iter().filter(|&o| o != 0).min()
}

/// The shortest unique signature for the function at `addr` (a VA or RVA;
/// normally its first byte). Errors if none exists within the function (or
/// `max_len`).
pub fn unique_signature(img: &PeImage, addr: u64, opts: &ShortOptions) -> Result<Signature> {
    signature_among(img, addr, &[], opts)
}

/// Like [`unique_signature`], but the pattern may also match the functions in
/// `equivalent` (entry VAs or RVAs): for functions that are interchangeable for
/// the caller's purpose and differ only in what they reference (e.g. a generic
/// instantiated for two types with the same layout and delegating behaviour).
/// Stops once every match is the target or one of those; it must match the
/// target.
pub fn signature_among(
    img: &PeImage,
    addr: u64,
    equivalent: &[u64],
    opts: &ShortOptions,
) -> Result<Signature> {
    let allowed: Vec<usize> = equivalent
        .iter()
        .filter_map(|&a| img.to_rva(a).ok().and_then(|r| img.rva_to_offset(r)))
        .collect();
    let rva = img.to_rva(addr)?;
    let start = img
        .rva_to_offset(rva)
        .with_context(|| format!("{addr:#x} is not in any section"))?;
    let end_rva = match img.function_containing(rva) {
        Some((_, e)) if !opts.continue_outside => e,
        _ => rva.saturating_add(opts.max_len as u32),
    };
    let ranges = opts.scope.map_or_else(|| img.section_ranges(), |r| vec![r]);
    let code_end = img
        .rva_to_offset(end_rva.saturating_sub(1))
        .map_or(img.bytes.len(), |o| o + 1);
    let code_len = opts.max_len.max(16).saturating_add(16);
    let code = img
        .bytes
        .get(start..img.bytes.len().min(start.saturating_add(code_len)))
        .with_context(|| format!("{addr:#x} is past the end of the file"))?;

    let mut decoder = Decoder::with_ip(
        64,
        code,
        img.image_base + u64::from(rva),
        DecoderOptions::NONE,
    );
    let mut bytes: Vec<Option<u8>> = Vec::new();
    let mut candidates: Option<Vec<usize>> = None;
    let mut instr = Instruction::default();
    let mut pos = 0usize;
    while decoder.can_decode() {
        if start + pos >= code_end {
            bail!("signature left function scope without being unique");
        }
        if bytes.len() > opts.max_len {
            bail!("signature not unique within {} bytes", opts.max_len);
        }
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            bail!("undecodable instruction at {:#x}", instr.ip());
        }
        let len = instr.len();
        let raw = &code[pos..pos + len];
        let cut = wildcard_start(&decoder, &instr, opts.wildcard).unwrap_or(len);
        bytes.extend(raw.iter().enumerate().map(|(i, &b)| (i < cut).then_some(b)));
        pos += len;

        let sig = Signature::new(bytes.clone());
        let hits = match &candidates {
            None => ranges
                .iter()
                .flat_map(|&(s, e)| sig.scan(&img.bytes[s..e]).into_iter().map(move |o| s + o))
                .collect(),
            Some(prev) => prev
                .iter()
                .copied()
                .filter(|&o| sig.matches_at(img.bytes, o))
                .collect::<Vec<_>>(),
        };
        let fixed = bytes.iter().filter(|b| b.is_some()).count();
        if hits.contains(&start)
            && hits.iter().all(|h| *h == start || allowed.contains(h))
            && fixed >= opts.min_fixed
        {
            while bytes.last() == Some(&None) {
                bytes.pop();
            }
            return Ok(Signature::new(bytes));
        }
        if !hits.contains(&start) {
            bail!("pattern stopped matching its own function (bad decode?)");
        }
        candidates = Some(hits);
    }
    bail!("signature not unique (reached the end of the readable code)")
}

/// A second, independent signature for the function at `entry`: the shortest
/// unique one starting at the first instruction at or after `skip` bytes into
/// it (normally where the entry signature ends), so the two cover different
/// code and don't fail together. `equivalent` are other entries the pattern
/// may also match, as for [`signature_among`]; they're compared at the same
/// distance into their own function, which holds because the entry signature
/// matched them over those bytes.
pub fn interior_signature(
    img: &PeImage,
    entry: u64,
    skip: usize,
    equivalent: &[u64],
    opts: &ShortOptions,
) -> Result<(usize, Signature)> {
    let rva = img.to_rva(entry)?;
    let (begin, end) = img
        .function_containing(rva)
        .context("entry is in no .pdata function")?;
    let start = img
        .rva_to_offset(begin)
        .context("function not in the file")?;
    let len = (end - begin) as usize;
    let code = img
        .bytes
        .get(start..start + len)
        .context("function runs past the file")?;
    let mut d = Decoder::with_ip(
        64,
        code,
        img.image_base + u64::from(begin),
        DecoderOptions::NONE,
    );
    let mut ins = Instruction::default();
    let mut at = 0usize;
    while d.can_decode() && at < skip {
        d.decode_out(&mut ins);
        if ins.is_invalid() {
            bail!("undecodable instruction at {:#x}", ins.ip());
        }
        at += ins.len();
    }
    if at >= len {
        bail!("function is no longer than its entry signature");
    }
    let shifted: Vec<u64> = equivalent.iter().map(|&e| e + at as u64).collect();
    let sig = signature_among(
        img,
        img.image_base + u64::from(begin) + at as u64,
        &shifted,
        opts,
    )?;
    Ok((at, sig))
}

/// The shortest contiguous window of `pattern` (typically a whole-function
/// pattern compiled from source) that still matches exactly once in `img`,
/// at the right place: the full pattern must match uniquely first, and the
/// window must match only at that match plus its own offset. The window has
/// at least `min_fixed` fixed bytes, starts and ends on a fixed byte, and
/// begins at or after `skip` (to stay clear of another locator's bytes).
/// Returns (offset into the pattern, window).
pub fn trim_unique(
    img: &PeImage,
    pattern: &Signature,
    min_fixed: usize,
    skip: usize,
) -> Result<(usize, Signature)> {
    use crate::sig::TrigramIndex;
    let ranges = img.section_ranges();
    let indexes: Vec<(usize, TrigramIndex)> = ranges
        .iter()
        .map(|&(s, e)| (s, TrigramIndex::new(&img.bytes[s..e])))
        .collect();
    let find = |sig: &Signature| -> Vec<usize> {
        indexes
            .iter()
            .flat_map(|(base, idx)| idx.find(sig).into_iter().map(move |o| base + o))
            .collect()
    };
    let whole = find(pattern);
    let [origin] = whole[..] else {
        bail!(
            "the full pattern matches {} times in the target, need exactly one",
            whole.len()
        );
    };
    let toks = pattern.bytes();
    let n = toks.len();
    // A window starts and ends on a fixed byte, so it has at least one.
    for len in min_fixed.max(1)..=n {
        for i in skip..=n.saturating_sub(len) {
            let w = &toks[i..i + len];
            if w[0].is_none()
                || w[len - 1].is_none()
                || w.iter().filter(|b| b.is_some()).count() < min_fixed
            {
                continue;
            }
            let sig = Signature::new(w.to_vec());
            if find(&sig) == [origin + i] {
                return Ok((i, sig));
            }
        }
    }
    bail!("no window of the pattern past byte {skip} is unique in the target")
}

/// Where a signature matches in an image, as RVAs.
pub fn matches(img: &PeImage, sig: &Signature) -> Vec<u32> {
    img.section_ranges()
        .into_iter()
        .flat_map(|(s, e)| sig.scan(&img.bytes[s..e]).into_iter().map(move |o| s + o))
        .filter_map(|o| img.offset_to_rva(o))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_wildcard(code: &[u8], kinds: OperandKinds) -> Vec<Option<u8>> {
        let mut d = Decoder::with_ip(64, code, 0x1000, DecoderOptions::NONE);
        let mut out = Vec::new();
        let mut pos = 0;
        let mut instr = Instruction::default();
        while d.can_decode() {
            d.decode_out(&mut instr);
            let cut = wildcard_start(&d, &instr, kinds).unwrap_or(instr.len());
            out.extend(
                code[pos..pos + instr.len()]
                    .iter()
                    .enumerate()
                    .map(|(i, &b)| (i < cut).then_some(b)),
            );
            pos += instr.len();
        }
        out
    }

    fn pat(v: &[Option<u8>]) -> String {
        Signature::new(v.to_vec()).to_pattern()
    }

    #[test]
    fn wildcards_like_sigmaker() {
        // sub rsp, 0x48 ; mov [rsp+0x28], rdx ; call rel32 ; mov rax, [rip+disp]
        let code = [
            0x48, 0x83, 0xec, 0x48, 0x48, 0x89, 0x54, 0x24, 0x28, 0xe8, 1, 2, 3, 4, 0x48, 0x8b,
            0x05, 9, 9, 9, 9,
        ];
        assert_eq!(
            pat(&kinds_wildcard(&code, OperandKinds::default())),
            "48 83 ec ?? 48 89 54 24 ?? e8 ?? ?? ?? ?? 48 8b 05 ?? ?? ?? ??"
        );
        // keep displ: the stack slot stays literal.
        let k = OperandKinds::keeping("displ").unwrap();
        assert_eq!(
            pat(&kinds_wildcard(&code, k)),
            "48 83 ec ?? 48 89 54 24 28 e8 ?? ?? ?? ?? 48 8b 05 ?? ?? ?? ??"
        );
    }

    #[test]
    fn displacement_plus_immediate_wildcards_from_the_first() {
        // mov byte [rbp+0x1b7], 0  (C6 85 disp32 imm8): sigmaker -> C6 85 ?? ?? ?? ?? ??
        let code = [0xc6, 0x85, 0xb7, 0x01, 0, 0, 0x00];
        assert_eq!(
            pat(&kinds_wildcard(&code, OperandKinds::default())),
            "c6 85 ?? ?? ?? ?? ??"
        );
        // keep displ: only the immediate goes.
        assert_eq!(
            pat(&kinds_wildcard(
                &code,
                OperandKinds::keeping("displ").unwrap()
            )),
            "c6 85 b7 01 00 00 ??"
        );
    }

    #[test]
    fn short_jumps_and_no_operand_instructions() {
        // jmp rel8 ; ret ; nop dword [rax+rax+0] (0F 1F 44 00 00)
        let code = [0xeb, 0x05, 0xc3, 0x0f, 0x1f, 0x44, 0x00, 0x00];
        assert_eq!(
            pat(&kinds_wildcard(&code, OperandKinds::default())),
            "eb ?? c3 0f 1f 44 00 ??"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn elf_functions_come_from_eh_frame() {
        use object::{Object, ObjectSymbol, SymbolKind};
        let bytes = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let img = PeImage::parse(&bytes).unwrap();
        let obj = object::File::parse(&*bytes).unwrap();
        let funcs: Vec<_> = obj
            .symbols()
            .filter(|s| s.kind() == SymbolKind::Text && s.size() > 0)
            .collect();
        let exact = funcs
            .iter()
            .filter(|s| {
                let rva = (s.address() - img.image_base) as u32;
                img.function_containing(rva) == Some((rva, rva + s.size() as u32))
            })
            .count();
        assert!(
            exact * 10 >= funcs.len() * 9,
            "{exact} of {} symbols match an .eh_frame extent",
            funcs.len()
        );
        let text = img.sections.iter().find(|s| s.name == ".text").unwrap();
        assert!(text.executable);
        assert_eq!(img.offset_to_rva(text.raw_offset as usize), Some(text.rva));
    }

    #[test]
    fn keeping_rejects_unknown_kinds() {
        assert!(OperandKinds::keeping("displ,bogus").is_err());
    }

    fn name8(s: &str) -> [u8; 8] {
        let mut n = [0u8; 8];
        n[..s.len()].copy_from_slice(s.as_bytes());
        n
    }

    /// A minimal PE32+ image: `.text` at RVA 0x1000 holding `funcs` back to
    /// back, and `.pdata` at 0x2000 giving each one's extent. Returns the file
    /// and each function's VA.
    fn fake_pe(funcs: &[&[u8]]) -> (Vec<u8>, Vec<u64>) {
        const BASE: u64 = 0x1_4000_0000;
        let text: Vec<u8> = funcs.iter().flat_map(|f| f.iter().copied()).collect();
        let text_off = 0x400usize;
        let pdata_off = text_off + text.len().next_multiple_of(16);
        let mut b = vec![0u8; pdata_off + funcs.len() * 12];
        let put16 =
            |b: &mut Vec<u8>, o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let put32 =
            |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        b[0..2].copy_from_slice(b"MZ");
        put32(&mut b, 0x3c, 0x80);
        let nt = 0x80;
        b[nt..nt + 2].copy_from_slice(b"PE");
        put16(&mut b, nt + 4, 0x8664);
        put16(&mut b, nt + 6, 2);
        put16(&mut b, nt + 20, 240);
        let opt = nt + 24;
        put16(&mut b, opt, 0x20b);
        b[opt + 24..opt + 32].copy_from_slice(&BASE.to_le_bytes());
        put32(&mut b, opt + 108, 16);
        put32(&mut b, opt + 112 + 3 * 8, 0x2000);
        put32(&mut b, opt + 112 + 3 * 8 + 4, (funcs.len() * 12) as u32);
        let mut sh = opt + 240;
        for (name, rva, off, size, ch) in [
            (".text", 0x1000u32, text_off, text.len(), 0x6000_0020u32),
            (".pdata", 0x2000, pdata_off, funcs.len() * 12, 0x4000_0040),
        ] {
            b[sh..sh + 8].copy_from_slice(&name8(name));
            put32(&mut b, sh + 8, size as u32);
            put32(&mut b, sh + 12, rva);
            put32(&mut b, sh + 16, size as u32);
            put32(&mut b, sh + 20, off as u32);
            put32(&mut b, sh + 36, ch);
            sh += 40;
        }
        b[text_off..text_off + text.len()].copy_from_slice(&text);
        let mut at = 0x1000u32;
        let mut vas = Vec::new();
        for (i, f) in funcs.iter().enumerate() {
            put32(&mut b, pdata_off + i * 12, at);
            put32(&mut b, pdata_off + i * 12 + 4, at + f.len() as u32);
            vas.push(BASE + u64::from(at));
            at += f.len() as u32;
        }
        (b, vas)
    }

    // push rbp ; sub rsp,0x20 ; mov rbp,rsp ; xor eax,eax ; nop ; mov eax,IMM ;
    // add rsp,0x20 ; pop rbp ; ret
    fn body(imm: u8) -> Vec<u8> {
        vec![
            0x55, 0x48, 0x83, 0xec, 0x20, 0x48, 0x89, 0xe5, 0x31, 0xc0, 0x90, 0xb8, imm, 0, 0, 0,
            0x48, 0x83, 0xc4, 0x20, 0x5d, 0xc3,
        ]
    }
    // Shares the first 8 bytes of `body`, then diverges.
    const OTHER: &[u8] = &[0x55, 0x48, 0x83, 0xec, 0x40, 0x48, 0x89, 0xe5, 0xc3];

    #[test]
    fn siblings_differing_only_in_an_immediate_are_never_unique() {
        let (a, b) = (body(1), body(2));
        let (pe, vas) = fake_pe(&[&a, &b, OTHER]);
        let img = PeImage::parse(&pe).unwrap();
        // The immediate is wildcarded, so a and b look identical end to end.
        assert!(unique_signature(&img, vas[0], &ShortOptions::default()).is_err());
        // The third function is unique on its own.
        assert!(unique_signature(&img, vas[2], &ShortOptions::default()).is_ok());
    }

    #[test]
    fn signature_among_accepts_the_equivalent_sibling_and_stops_early() {
        let (a, b) = (body(1), body(2));
        let (pe, vas) = fake_pe(&[&a, &b, OTHER]);
        let img = PeImage::parse(&pe).unwrap();
        let sig = signature_among(&img, vas[0], &[vas[1]], &ShortOptions::default()).unwrap();
        // Long enough to rule out OTHER (which diverges at `xor`), no longer.
        assert_eq!(sig.to_pattern(), "55 48 83 ec ?? 48 89 e5 31 c0");
        let rva = |v: u64| img.to_rva(v).unwrap();
        let hits: Vec<u32> = sig
            .scan(pe.as_slice())
            .iter()
            .filter_map(|&o| img.offset_to_rva(o))
            .collect();
        assert_eq!(hits, vec![rva(vas[0]), rva(vas[1])]);
        // Declaring the wrong function equivalent does not help.
        assert!(signature_among(&img, vas[0], &[vas[2]], &ShortOptions::default()).is_err());
    }

    #[test]
    fn trim_unique_with_no_minimum_finds_a_window() {
        let (a, b) = (body(1), body(2));
        let (pe, _) = fake_pe(&[&a, &b, OTHER]);
        let img = PeImage::parse(&pe).unwrap();
        let pattern = Signature::new(OTHER.iter().copied().map(Some).collect());
        let (at, window) = trim_unique(&img, &pattern, 0, 0).unwrap();
        assert!(!window.bytes().is_empty());
        assert!(at + window.bytes().len() <= OTHER.len());
    }

    /// The offset of the `.text` section header in a [`fake_pe`] image.
    const TEXT_HEADER: usize = 0x80 + 24 + 240;

    #[test]
    fn a_section_past_the_end_of_the_file_is_an_error_not_a_panic() {
        let (a, b) = (body(1), body(2));
        let (mut pe, vas) = fake_pe(&[&a, &b, OTHER]);
        // Raw data that starts past the file, and whose end overflows a u32.
        pe[TEXT_HEADER + 20..TEXT_HEADER + 24].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        let img = PeImage::parse(&pe).unwrap();
        assert!(img.section_ranges().iter().all(|&(_, e)| e <= pe.len()));
        assert!(unique_signature(&img, vas[1], &ShortOptions::default()).is_err());
    }

    #[test]
    fn raw_padding_past_the_virtual_size_is_not_mapped() {
        let (a, b) = (body(1), body(2));
        let (mut pe, _) = fake_pe(&[&a, &b, OTHER]);
        pe[TEXT_HEADER + 8..TEXT_HEADER + 12].copy_from_slice(&4u32.to_le_bytes());
        let img = PeImage::parse(&pe).unwrap();
        assert_eq!(img.rva_to_offset(0x1003), Some(0x403));
        assert_eq!(img.rva_to_offset(0x1004), None);
    }

    #[test]
    fn interior_signature_shifts_the_equivalents_with_it() {
        let (a, b) = (body(1), body(2));
        let (pe, vas) = fake_pe(&[&a, &b, OTHER]);
        let img = PeImage::parse(&pe).unwrap();
        let opts = ShortOptions::default();
        // Interior starting at `xor` (offset 8) matches a and b at the same
        // distance into each, so b's entry + 8 must be accepted, not b's entry.
        assert!(interior_signature(&img, vas[0], 8, &[], &opts).is_err());
        let (at, sig) = interior_signature(&img, vas[0], 8, &[vas[1]], &opts).unwrap();
        assert_eq!(at, 8);
        assert!(sig.to_pattern().starts_with("31 c0"));
    }
}
