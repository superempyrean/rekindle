//! Instruction shapes: a run of instructions written as mnemonics and operand
//! kinds, with registers as variables and displacements, branch targets and
//! large immediates left out.
//!
//! A compiler change or a fork reallocates registers and moves stack slots and
//! field offsets, while the instruction sequence of a function stays. In
//! gpui's `Window::draw_roots`, rustc 1.98.1 with thin LTO and rustc 1.97.1
//! with fat LTO emit the same run with `self` in `rbx` in one and `rcx` in the
//! other:
//!
//! ```text
//! cmp d[r0+*],0; je *; movd x0,d[r0+*]; movd x1,d[r0+*]; movss x2,d[r0+*]
//! ```
//!
//! # Syntax
//!
//! Instructions are separated by `; `. Each is a mnemonic, then its operands
//! separated by `,`:
//!
//! - `r0`, `r1`, ...: general registers, numbered in order of first use, with
//!   `d`, `w` or `b` for a 32-, 16- or 8-bit use (`r0d` is `eax` where `r0` is
//!   `rax`). `x0`, `y0`, `z0`: vector registers at 128, 256 and 512 bits, one
//!   numbering for all three. A variable is one value: a register from where
//!   it is written to the next write of it, so an instruction that only
//!   writes a register starts a new variable. One variable is one register,
//!   and two live variables are two registers. A compiler that reuses a dead
//!   register in one build and takes a free one in another still matches.
//! - `d[r0+r1*8+*]`: memory, with its size (`b w d q o y z`, or the byte
//!   count, or `m` where the instruction gives none, as `lea` does) before
//!   the bracket. `+*` is a non-zero displacement, whatever its
//!   value. `[rip]` is a RIP-relative address.
//! - A decimal immediate matches only itself. In a pattern, `*` matches any
//!   operand.
//! - A branch or call target is `*`.

use std::collections::HashMap;

use iced_x86::{Instruction, OpKind, Register};

/// Immediates from this size up are written as `*` in suggested patterns:
/// they tend to be sizes and offsets, which a fork changes.
const LARGE_IMM: i64 = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    /// A register: its namespace (`r` general, `v` vector), the letter it is
    /// written with, its id (a full register, or a variable number in a
    /// pattern), its size suffix, and whether the instruction only writes it
    /// (set on decoded instructions: a pattern takes it from the instruction
    /// it is matched against).
    Reg {
        ns: char,
        letter: char,
        id: u32,
        suffix: &'static str,
        write: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Any,
    Pieces(Vec<Piece>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Insn {
    mnemonic: String,
    ops: Vec<Op>,
}

thread_local! {
    static INFO: std::cell::RefCell<iced_x86::InstructionInfoFactory> =
        std::cell::RefCell::new(iced_x86::InstructionInfoFactory::new());
}

fn mnemonic_of(ins: &Instruction) -> String {
    let mut m = format!("{:?}", ins.mnemonic()).to_lowercase();
    if ins.has_rep_prefix() {
        m.insert_str(0, "rep ");
    } else if ins.has_repne_prefix() {
        m.insert_str(0, "repne ");
    }
    m
}

fn reg_piece(r: Register) -> Piece {
    if r.is_gpr() {
        let suffix = match r.size() {
            8 => "",
            4 => "d",
            2 => "w",
            _ => "b",
        };
        return Piece::Reg {
            ns: 'r',
            letter: 'r',
            id: r.full_register() as u32,
            suffix,
            write: false,
        };
    }
    if r.is_vector_register() {
        let letter = match r.size() {
            16 => 'x',
            32 => 'y',
            _ => 'z',
        };
        return Piece::Reg {
            ns: 'v',
            letter,
            id: r.full_register() as u32,
            suffix: "",
            write: false,
        };
    }
    Piece::Text(format!("{r:?}").to_lowercase())
}

fn size_letter(bytes: usize) -> String {
    match bytes {
        1 => "b".into(),
        2 => "w".into(),
        4 => "d".into(),
        8 => "q".into(),
        16 => "o".into(),
        32 => "y".into(),
        64 => "z".into(),
        0 => "m".into(),
        n => n.to_string(),
    }
}

/// An instruction's shape, with registers as the registers themselves.
pub(crate) fn of(ins: &Instruction) -> Insn {
    let mnemonic = mnemonic_of(ins);
    let info = INFO.with(|f| f.borrow_mut().info(ins).clone());
    let ops = (0..ins.op_count())
        .map(|i| {
            let text = |s: String| Op::Pieces(vec![Piece::Text(s)]);
            match ins.op_kind(i) {
                OpKind::Register => {
                    let mut p = reg_piece(ins.op_register(i));
                    if let Piece::Reg { write, .. } = &mut p {
                        *write = info.op_access(i) == iced_x86::OpAccess::Write;
                    }
                    Op::Pieces(vec![p])
                }
                OpKind::Memory => {
                    let mut p = Vec::new();
                    let seg = ins.segment_prefix();
                    let mut head = String::new();
                    if seg != Register::None {
                        head.push_str(&format!("{seg:?}:").to_lowercase());
                    }
                    head.push_str(&size_letter(ins.memory_size().size()));
                    head.push('[');
                    if ins.is_ip_rel_memory_operand() {
                        head.push_str("rip]");
                        return text(head);
                    }
                    p.push(Piece::Text(head));
                    let mut first = true;
                    if ins.memory_base() != Register::None {
                        p.push(reg_piece(ins.memory_base()));
                        first = false;
                    }
                    if ins.memory_index() != Register::None {
                        if !first {
                            p.push(Piece::Text("+".into()));
                        }
                        p.push(reg_piece(ins.memory_index()));
                        p.push(Piece::Text(format!("*{}", ins.memory_index_scale())));
                        first = false;
                    }
                    if ins.memory_displacement64() != 0 || first {
                        p.push(Piece::Text(if first { "*".into() } else { "+*".into() }));
                    }
                    p.push(Piece::Text("]".into()));
                    Op::Pieces(merge(p))
                }
                OpKind::Immediate8
                | OpKind::Immediate16
                | OpKind::Immediate32
                | OpKind::Immediate64
                | OpKind::Immediate8to16
                | OpKind::Immediate8to32
                | OpKind::Immediate8to64
                | OpKind::Immediate32to64 => text((ins.immediate(i) as i64).to_string()),
                // Branch and call targets, among others.
                _ => text("*".into()),
            }
        })
        .collect();
    Insn { mnemonic, ops }
}

/// Adjacent text pieces as one, so a parsed pattern and a decoded
/// instruction compare piece by piece.
fn merge(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        match (out.last_mut(), p) {
            (Some(Piece::Text(a)), Piece::Text(b)) => a.push_str(&b),
            (_, p) => out.push(p),
        }
    }
    out
}

/// Parse a pattern. Errors name the instruction that doesn't parse.
pub(crate) fn parse(pattern: &str) -> Result<Vec<Insn>, String> {
    let mut out = Vec::new();
    for raw in pattern.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (mnemonic, rest) = match raw.find(' ') {
            Some(i) if raw.starts_with("rep ") || raw.starts_with("repne ") => {
                let j = raw[i + 1..].find(' ').map(|j| i + 1 + j);
                match j {
                    Some(j) => (&raw[..j], raw[j + 1..].trim()),
                    None => (raw, ""),
                }
            }
            Some(i) => (&raw[..i], raw[i + 1..].trim()),
            None => (raw, ""),
        };
        let ops = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split(',')
                .map(|o| parse_op(o.trim()))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("`{raw}`: {e}"))?
        };
        out.push(Insn {
            mnemonic: mnemonic.to_string(),
            ops,
        });
    }
    if out.is_empty() {
        return Err("empty shape".into());
    }
    Ok(out)
}

fn parse_op(s: &str) -> Result<Op, String> {
    if s == "*" {
        return Ok(Op::Any);
    }
    if s.is_empty() {
        return Err("empty operand".into());
    }
    let b = s.as_bytes();
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        let boundary = i == 0 || !(b[i - 1] as char).is_ascii_alphanumeric();
        if boundary
            && matches!(c, 'r' | 'x' | 'y' | 'z')
            && b.get(i + 1).is_some_and(u8::is_ascii_digit)
        {
            let mut j = i + 1;
            while b.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            let id: u32 = s[i + 1..j]
                .parse()
                .map_err(|_| format!("bad register `{s}`"))?;
            let mut suffix = "";
            if c == 'r'
                && let Some(&l) = b.get(j)
            {
                let next_alnum = b
                    .get(j + 1)
                    .is_some_and(|x| (*x as char).is_ascii_alphanumeric());
                if !next_alnum {
                    suffix = match l {
                        b'd' => "d",
                        b'w' => "w",
                        b'b' => "b",
                        _ => "",
                    };
                }
            }
            j += suffix.len();
            if !text.is_empty() {
                pieces.push(Piece::Text(std::mem::take(&mut text)));
            }
            pieces.push(Piece::Reg {
                ns: if c == 'r' { 'r' } else { 'v' },
                letter: c,
                id,
                suffix,
                write: false,
            });
            i = j;
            continue;
        }
        text.push(c);
        i += 1;
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok(Op::Pieces(pieces))
}

/// Whether `insns` matches the pattern, with one consistent binding of
/// pattern variables to registers (one to one).
pub(crate) fn matches(pattern: &[Insn], insns: &[Insn]) -> bool {
    if pattern.len() != insns.len() {
        return false;
    }
    // Variable -> register, and register -> the variable live in it.
    let mut fwd: HashMap<(char, u32), u32> = HashMap::new();
    let mut back: HashMap<(char, u32), u32> = HashMap::new();
    for (p, c) in pattern.iter().zip(insns) {
        if p.mnemonic != c.mnemonic || p.ops.len() != c.ops.len() {
            return false;
        }
        let mut writes = Vec::new();
        for (po, co) in p.ops.iter().zip(&c.ops) {
            let (Op::Pieces(pp), Op::Pieces(cp)) = (po, co) else {
                if matches!(po, Op::Any) {
                    continue;
                }
                return false;
            };
            if pp.len() != cp.len() {
                return false;
            }
            for (a, b) in pp.iter().zip(cp) {
                match (a, b) {
                    (Piece::Text(x), Piece::Text(y)) if x == y => {}
                    (
                        Piece::Reg {
                            ns,
                            letter,
                            id: var,
                            suffix,
                            ..
                        },
                        Piece::Reg {
                            ns: ns2,
                            letter: l2,
                            id: reg,
                            suffix: s2,
                            write,
                        },
                    ) if ns == ns2 && letter == l2 && suffix == s2 => {
                        if *write {
                            writes.push((*ns, *var, *reg));
                        } else if *fwd.entry((*ns, *var)).or_insert(*reg) != *reg
                            || *back.entry((*ns, *reg)).or_insert(*var) != *var
                        {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
        }
        // A write starts a new variable: not one already used, and the
        // variable the register held is gone.
        for (ns, var, reg) in writes {
            if fwd.contains_key(&(ns, var)) {
                return false;
            }
            if let Some(old) = back.insert((ns, reg), var) {
                fwd.remove(&(ns, old));
            }
            fwd.insert((ns, var), reg);
        }
    }
    true
}

/// A run as a pattern: registers renamed to variables in order of first use,
/// large immediates (and every immediate of instruction `wild_imm`, the one
/// whose immediate is the entry) written as `*`.
pub(crate) fn write(insns: &[Insn], wild_imm: Option<usize>) -> String {
    let mut vars: HashMap<(char, u32), u32> = HashMap::new();
    let mut next: HashMap<char, u32> = HashMap::new();
    let fresh = |ns: char, next: &mut HashMap<char, u32>| {
        let c = next.entry(ns).or_default();
        *c += 1;
        *c - 1
    };
    let mut out = Vec::new();
    for (n, ins) in insns.iter().enumerate() {
        // Reads see the variables live before the instruction, and its
        // writes start new ones.
        let mut names: HashMap<(usize, usize), u32> = HashMap::new();
        for pass in [false, true] {
            for (o, op) in ins.ops.iter().enumerate() {
                let Op::Pieces(p) = op else { continue };
                for (k, piece) in p.iter().enumerate() {
                    let Piece::Reg { ns, id, write, .. } = piece else {
                        continue;
                    };
                    if *write != pass {
                        continue;
                    }
                    let v = if *write {
                        let v = fresh(*ns, &mut next);
                        vars.insert((*ns, *id), v);
                        v
                    } else {
                        *vars
                            .entry((*ns, *id))
                            .or_insert_with(|| fresh(*ns, &mut next))
                    };
                    names.insert((o, k), v);
                }
            }
        }
        let ops: Vec<String> = ins
            .ops
            .iter()
            .enumerate()
            .map(|(o, op)| match op {
                Op::Any => "*".to_string(),
                Op::Pieces(p) => {
                    if let [Piece::Text(t)] = p.as_slice()
                        && let Ok(v) = t.parse::<i64>()
                        && (wild_imm == Some(n) || v.abs() >= LARGE_IMM)
                    {
                        return "*".into();
                    }
                    p.iter()
                        .enumerate()
                        .map(|(k, piece)| match piece {
                            Piece::Text(t) => t.clone(),
                            Piece::Reg { letter, suffix, .. } => {
                                format!("{letter}{}{suffix}", names[&(o, k)])
                            }
                        })
                        .collect()
                }
            })
            .collect();
        out.push(if ops.is_empty() {
            ins.mnemonic.clone()
        } else {
            format!("{} {}", ins.mnemonic, ops.join(","))
        });
    }
    out.join("; ")
}

/// Whether a run is distinctive enough to stand for a function anywhere in
/// a binary, not just inside one: at least `min` instructions, one of them a
/// call or a RIP-relative operand (code and data the function names), and at
/// most half of them reaching memory through a displacement left out. A
/// short run of moves through `[reg+*]` is unique in one build by chance,
/// and in the drill one such run was unique in a different function.
pub(crate) fn distinctive(pattern: &[Insn], min: usize) -> bool {
    let text = |op: &Op| match op {
        Op::Pieces(p) => p
            .iter()
            .filter_map(|x| {
                if let Piece::Text(t) = x {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect::<String>(),
        Op::Any => String::new(),
    };
    let named = pattern
        .iter()
        .any(|i| i.mnemonic == "call" || i.ops.iter().any(|o| text(o).contains("[rip]")));
    let blind = pattern
        .iter()
        .filter(|i| i.ops.iter().any(|o| text(o).contains("+*]")))
        .count();
    pattern.len() >= min && named && blind * 2 <= pattern.len()
}

/// Start indices in `insns` where `pattern` matches.
pub(crate) fn find(pattern: &[Insn], insns: &[Insn]) -> Vec<usize> {
    if pattern.len() > insns.len() {
        return Vec::new();
    }
    (0..=insns.len() - pattern.len())
        .filter(|&i| matches(pattern, &insns[i..i + pattern.len()]))
        .collect()
}

/// Mnemonics as patterns write them, by instruction kind: `find_raw`
/// compares a run's mnemonics with a pattern's before it shapes any
/// operands.
pub(crate) type Mnemonics = HashMap<(iced_x86::Mnemonic, bool, bool), String>;

/// Start indices in `raw` where `pattern` matches. A run is shaped only when
/// all of its mnemonics match the pattern's, so this is quick over a whole
/// binary.
pub(crate) fn find_raw(pattern: &[Insn], raw: &[Instruction], memo: &mut Mnemonics) -> Vec<usize> {
    let n = pattern.len();
    if n == 0 || n > raw.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..=raw.len() - n {
        let same = raw[i..i + n].iter().zip(pattern).all(|(ins, p)| {
            let key = (ins.mnemonic(), ins.has_rep_prefix(), ins.has_repne_prefix());
            memo.entry(key).or_insert_with(|| mnemonic_of(ins)) == &p.mnemonic
        });
        if !same {
            continue;
        }
        let run: Vec<Insn> = raw[i..i + n].iter().map(of).collect();
        if matches(pattern, &run) {
            out.push(i);
        }
    }
    out
}

/// Which side of the target a suggested run prefers its context on. A
/// compiler schedules the instructions around a field access differently
/// from build to build, often on one side only, so an operand anchor takes
/// one shape of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lead {
    /// The run starts at the target, and reaches back only when it must.
    After,
    /// The run ends at the target, and reaches forward only when it must.
    Before,
}

/// The shortest run of at least `min` instructions around `target` (an index
/// into `insns`) whose pattern matches once in `insns`: the pattern and the
/// target's index in it.
pub(crate) fn suggest(
    insns: &[Insn],
    target: usize,
    wild_imm: bool,
    min: usize,
    max: usize,
    lead: Lead,
) -> Option<(String, usize)> {
    suggest_where(insns, target, wild_imm, min, max, lead, |pat| {
        find(pat, insns).len() == 1
    })
}

/// As [`suggest`], with `unique` deciding whether a pattern is unique where
/// it will be resolved (the whole binary, say).
pub(crate) fn suggest_where(
    insns: &[Insn],
    target: usize,
    wild_imm: bool,
    min: usize,
    max: usize,
    lead: Lead,
    mut unique: impl FnMut(&[Insn]) -> bool,
) -> Option<(String, usize)> {
    for len in min..=max {
        let order: Vec<usize> = match lead {
            Lead::After => (0..len).collect(),
            Lead::Before => (0..len).rev().collect(),
        };
        for before in order {
            let Some(start) = target.checked_sub(before) else {
                break;
            };
            let end = start + len;
            if end > insns.len() {
                continue;
            }
            let text = write(&insns[start..end], wild_imm.then_some(before));
            let pat = parse(&text).ok()?;
            if unique(&pat) {
                return Some((text, before));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(code: &[u8]) -> Vec<Insn> {
        iced_x86::Decoder::with_ip(64, code, 0x1000, iced_x86::DecoderOptions::NONE)
            .into_iter()
            .map(|i| of(&i))
            .collect()
    }

    // Both runs from `Window::draw_roots`: thin LTO with rustc 1.98.1
    // (`self` in rbx), fat LTO with rustc 1.97.1 on a fork (`self` in rcx,
    // every field 40 bytes on).
    const THIN: &str = "83 bb b8 15 00 00 00 0f 84 09 5b 00 00 66 0f 6e bb b0 15 00 00 66 0f 6e b3 b4 15 00 00 f3 44 0f 10 83 00 1d 00 00";
    const FAT: &str = "83 b9 e0 15 00 00 00 0f 84 6c 63 00 00 66 0f 6e b9 d8 15 00 00 66 0f 6e b1 dc 15 00 00 f3 44 0f 10 81 28 1d 00 00";

    fn hex(s: &str) -> Vec<u8> {
        s.split(' ')
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[test]
    fn a_run_matches_across_register_allocation() {
        let thin = decode(&hex(THIN));
        let text = write(&thin, None);
        assert_eq!(
            text,
            "cmp d[r0+*],0; je *; movd x0,d[r0+*]; movd x1,d[r0+*]; movss x2,d[r0+*]"
        );
        let pat = parse(&text).unwrap();
        assert_eq!(find(&pat, &decode(&hex(FAT))), vec![0]);
    }

    #[test]
    fn one_variable_is_one_value() {
        // mov rax,[rbx+8]; mov rcx,[rax+8]: rax is read where it was written.
        let code = decode(&hex("48 8b 43 08 48 8b 48 08"));
        assert_eq!(write(&code, None), "mov r1,q[r0+*]; mov r2,q[r1+*]");
        assert!(!matches(
            &parse("mov r1,q[r0+*]; mov r2,q[r0+*]").unwrap(),
            &code
        ));
        // Two live variables can't be one register: rbx and rax both read.
        let both = decode(&hex("48 8b 43 08 48 03 03"));
        assert!(!matches(
            &parse("mov r1,q[r0+*]; add r2,q[r2]").unwrap(),
            &both
        ));
        assert!(matches(
            &parse("mov r1,q[r0+*]; add r1,q[r0]").unwrap(),
            &both
        ));
    }

    #[test]
    fn a_write_frees_the_register() {
        // From draw_roots: one build overwrites the base it reads from
        // (`mov rcx,[rcx+0x678]`), the other loads into a free register
        // (`mov rcx,[rdi+0x680]`).
        let thin = decode(&hex(
            "66 0f 6e b1 00 1d 00 00 48 8b 81 60 06 00 00 48 8b 89 78 06 00 00 66 0f 6f 00",
        ));
        let fat = decode(&hex(
            "66 0f 6e bf 28 1d 00 00 48 8b 87 68 06 00 00 48 8b 8f 80 06 00 00 66 0f 6f 00",
        ));
        let text = write(&thin, None);
        assert_eq!(
            text,
            "movd x0,d[r0+*]; mov r1,q[r0+*]; mov r2,q[r0+*]; movdqa x1,o[r1]"
        );
        assert!(matches(&parse(&text).unwrap(), &fat));
    }

    #[test]
    fn sizes_and_immediates_are_kept() {
        // mov eax,[rbx+8]; add rcx,0x14c0
        let code = decode(&hex("8b 43 08 48 81 c1 c0 14 00 00"));
        assert_eq!(write(&code, None), "mov r1d,d[r0+*]; add r2,*");
        assert!(!matches(&parse("mov r1,d[r0+*]; add r2,*").unwrap(), &code));
        // A small immediate stays exact.
        let small = decode(&hex("48 83 c1 10"));
        assert_eq!(write(&small, None), "add r0,16");
        assert_eq!(write(&small, Some(0)), "add r0,*");
    }

    #[test]
    fn a_function_wide_shape_names_something() {
        let movs = parse("mov r1,q[r0+*]; mov q[r2+*],r1; mov r3,q[r0+*]; mov q[r2+*],r3").unwrap();
        assert!(!distinctive(&movs, 4));
        let named = parse("lea r0,m[rip]; mov r1,r0; call *; test r1,r1").unwrap();
        assert!(distinctive(&named, 4));
        assert!(!distinctive(&named, 8));
    }

    #[test]
    fn suggestion_is_unique_in_its_function() {
        let mut code = hex(THIN);
        code.extend(hex(THIN));
        code.extend(hex("48 8b 43 08"));
        let insns = decode(&code);
        // The movss at index 4 occurs twice in the doubled run, so the
        // suggestion has to reach the trailing mov.
        let (text, at) = suggest(&insns, 9, false, 2, 12, Lead::After).unwrap();
        let pat = parse(&text).unwrap();
        let hits = find(&pat, &insns);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0] + at, 9);
    }
}
