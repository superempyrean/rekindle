//! Healing: new locators for the anchors a build weakened, so they go into
//! the next build with their redundancy back.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow};

use rekindle_sigmatch::short::PeImage;

use crate::descriptor::{Anchor, AnchorSet, Locator};
use crate::index::BinaryIndex;
use crate::resolve::{AnchorResult, LocatorResult, ResolutionReport, Status, Verdict};
use crate::suggest::{suggest, suggest_call, suggest_operand};

/// What [`heal`] made of an anchor set.
#[derive(Debug, Clone)]
pub struct Healing {
    /// The set, with each healed anchor's lost locators replaced.
    pub set: AnchorSet,
    /// The anchors healed: (name, locators that still agreed, locators now).
    pub healed: Vec<(String, usize, usize)>,
    /// Weakened anchors left as they were, and why.
    pub skipped: Vec<(String, String)>,
}

/// Replace the locators a build broke, for every anchor that is weakened
/// (some locators lost) but still [corroborated](AnchorResult::corroborated)
/// in `binary`. The locators that agreed are kept, and new ones are built
/// at the place they agree on: [`suggest`] for a function anchor,
/// [`suggest_call`] for one found by a call inside another anchor's
/// function, [`suggest_operand`] at an agreeing instruction for an operand
/// anchor. `report` is `set` resolved against `binary`.
///
/// The healed set is resolved again before it's returned. Each healed
/// anchor must resolve with every locator agreeing, to the same place or
/// value as before, and no other anchor may change. An anchor that fails
/// this keeps its old locators.
///
/// Healing doesn't change what this build resolves to. It matters for the
/// next one: an anchor that went into this build with two of its three
/// locators goes into the next one with three again.
pub fn heal(set: &AnchorSet, report: &ResolutionReport, binary: &[u8]) -> Healing {
    let Ok(pe) = PeImage::parse(binary) else {
        return Healing {
            set: set.clone(),
            healed: Vec::new(),
            skipped: vec![("*".into(), "not a PE image".into())],
        };
    };
    let index = BinaryIndex::new(binary);
    let found: HashMap<&str, usize> = report
        .anchors
        .iter()
        .filter_map(|r| Some((r.name.as_str(), r.offset()?)))
        .collect();
    let mut candidate = set.clone();
    let mut healed = Vec::new();
    let mut skipped = Vec::new();
    for (a, r) in candidate.anchors.iter_mut().zip(&report.anchors) {
        if r.verdict != Verdict::Resolved || r.unique_locators == r.total_locators {
            continue;
        }
        if !r.corroborated() {
            skipped.push((a.name.clone(), "only one locator left".into()));
            continue;
        }
        if a.any_of {
            skipped.push((a.name.clone(), "any-of anchor".into()));
            continue;
        }
        let Some(rebuilt) = rebuild(a, r, binary, &pe, &index, &found) else {
            continue;
        };
        let (agree, fresh) = match rebuilt {
            Ok(rebuilt) => rebuilt,
            Err(e) => {
                skipped.push((a.name.clone(), format!("{e:#}")));
                continue;
            }
        };
        let mut locators: Vec<Locator> = a
            .locators
            .iter()
            .zip(&agree)
            .filter(|(_, ok)| **ok)
            .map(|(l, _)| l.clone())
            .collect();
        let kept = locators.len();
        for l in fresh {
            if !locators.contains(&l) {
                locators.push(l);
            }
        }
        if locators.len() == kept {
            skipped.push((a.name.clone(), "no new locator found".into()));
            continue;
        }
        healed.push((a.name.clone(), kept, locators.len()));
        a.locators = locators;
    }
    verify(set, report, binary, candidate, healed, skipped)
}

/// New locators for anchor `a`, built where its agreeing locators `r` point,
/// and which of its old locators agree. `None` for an anchor healing doesn't
/// rebuild: neither an operand nor a function anchor, or one whose
/// locators agree on nothing.
fn rebuild(
    a: &Anchor,
    r: &AnchorResult,
    binary: &[u8],
    pe: &PeImage,
    index: &BinaryIndex,
    found: &HashMap<&str, usize>,
) -> Option<Result<(Vec<bool>, Vec<Locator>)>> {
    let va = |off: usize| pe.offset_to_rva(off).map(|r| pe.image_base + u64::from(r));
    let at = |l: &LocatorResult| match l.status {
        Status::Unique { offset, .. } => Some(offset),
        _ => None,
    };
    if let Some(operand) = a.operand {
        // Raw values, before `plus` and `relative_to`: the vote's winner.
        let raw = |l: &LocatorResult| at(l).and_then(|o| index.operand_value(o, operand));
        let mut votes: BTreeMap<i64, usize> = BTreeMap::new();
        for v in r.locators.iter().filter_map(raw) {
            *votes.entry(v).or_default() += 1;
        }
        let (&want, _) = votes.iter().max_by_key(|(_, n)| **n)?;
        let agree: Vec<bool> = r.locators.iter().map(|l| raw(l) == Some(want)).collect();
        let insn = r
            .locators
            .iter()
            .zip(&agree)
            .find(|(_, ok)| **ok)
            .and_then(|(l, _)| at(l))
            .and_then(va);
        let made = match (insn, &a.within) {
            (Some(insn), Some(within)) => suggest_operand(
                binary,
                &a.name,
                insn,
                operand,
                within,
                a.relative_to.as_deref(),
                a.locators.len().max(3),
            ),
            _ => Err(anyhow!("no scope to rebuild it in")),
        };
        Some(made.map(|m| (agree, m.locators)))
    } else if a.function {
        let target = r.offset()?;
        let agree: Vec<bool> = r.locators.iter().map(|l| at(l) == Some(target)).collect();
        let made = match &a.within {
            None => va(target)
                .context("not in a section")
                .and_then(|t| suggest(binary, &a.name, t, 2, None, &[])),
            // Found by a call inside another function: rebuild from that call.
            Some(within) => found
                .get(within.as_str())
                .and_then(|&w| call_site(index, w, target))
                .and_then(va)
                .context("no call to it in its scope")
                .and_then(|call| suggest_call(binary, &a.name, call, within)),
        };
        Some(made.map(|m| (agree, m.locators)))
    } else {
        None
    }
}

/// Resolve the healed `candidate` and put back any healed anchor that
/// doesn't hold up. Reverting one can change the anchors resolved after it,
/// hence a second round; a set still wrong after that is given up on.
fn verify(
    set: &AnchorSet,
    report: &ResolutionReport,
    binary: &[u8],
    mut candidate: AnchorSet,
    mut healed: Vec<(String, usize, usize)>,
    mut skipped: Vec<(String, String)>,
) -> Healing {
    let same = |a: &AnchorResult, b: &AnchorResult| {
        a.verdict == b.verdict && a.offset() == b.offset() && a.value == b.value
    };
    for _ in 0..2 {
        let check = candidate.resolve(binary);
        let mut bad = Vec::new();
        for ((c, before), after) in candidate
            .anchors
            .iter()
            .zip(&report.anchors)
            .zip(&check.anchors)
        {
            let is_healed = healed.iter().any(|(n, ..)| *n == c.name);
            let full = after.unique_locators == after.total_locators;
            if !same(before, after) || (is_healed && !full) {
                bad.push(c.name.clone());
            }
        }
        if bad.is_empty() {
            return Healing {
                set: candidate,
                healed,
                skipped,
            };
        }
        for name in bad {
            let Some(i) = candidate.anchors.iter().position(|a| a.name == name) else {
                continue;
            };
            if healed.iter().any(|(n, ..)| *n == name) {
                candidate.anchors[i] = set.anchors[i].clone();
                healed.retain(|(n, ..)| *n != name);
                skipped.push((name, "didn't resolve the same with new locators".into()));
            }
        }
    }
    skipped.extend(
        healed
            .into_iter()
            .map(|(n, ..)| (n, "the healed set resolved differently".into())),
    );
    Healing {
        set: set.clone(),
        healed: Vec::new(),
        skipped,
    }
}

/// File offset of a `call` in the function containing `scope` whose target
/// is the function at file offset `target`.
fn call_site(index: &BinaryIndex, scope: usize, target: usize) -> Option<usize> {
    let pe = index.pe.as_ref()?;
    let (begin, end) = pe.function_containing(pe.offset_to_rva(scope)?)?;
    let (start, d) = index.decoder(begin, end)?;
    let want = pe.image_base + u64::from(pe.offset_to_rva(target)?);
    let first = pe.image_base + u64::from(begin);
    index
        .calls(d)
        .into_iter()
        .find(|&(_, t)| t == want)
        .map(|(site, _)| start + (site - first) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Operand;
    use crate::fixtures::tiny_pe;

    #[test]
    fn healing_replaces_lost_locators_where_two_still_agree() {
        #[rustfmt::skip]
        let f = [
            0x48, 0x89, 0x5c, 0x24, 0x08,       // mov [rsp+8], rbx
            0x57,                               // push rdi
            0x48, 0x83, 0xec, 0x20,             // sub rsp, 0x20
            0xba, 0x70, 0x00, 0x00, 0x00,       // mov edx, 0x70
            0xb9, 0x11, 0x00, 0x00, 0x00,       // mov ecx, 0x11
            0x41, 0xb8, 0x70, 0x00, 0x00, 0x00, // mov r8d, 0x70
            0x48, 0x83, 0xc4, 0x20,             // add rsp, 0x20
            0x5f,                               // pop rdi
            0xc3,                               // ret
        ];
        #[rustfmt::skip]
        let g = [
            0x48, 0x89, 0x4c, 0x24, 0x08,       // mov [rsp+8], rcx
            0x56,                               // push rsi
            0x48, 0x83, 0xec, 0x30,             // sub rsp, 0x30
            0xb8, 0x05, 0x00, 0x00, 0x00,       // mov eax, 5
            0x48, 0x83, 0xc4, 0x30,             // add rsp, 0x30
            0x5e,                               // pop rsi
            0xc3,                               // ret
        ];
        let mut code = vec![0xcc; 0x80];
        code[..f.len()].copy_from_slice(&f);
        code[0x40..0x40 + g.len()].copy_from_slice(&g);
        let bin = tiny_pe(
            &code,
            &[(0x00, f.len() as u32), (0x40, 0x40 + g.len() as u32)],
        );
        let bytes = |p: &str| Locator::Bytes { pattern: p.into() };
        let set = AnchorSet {
            target: String::new(),
            anchors: vec![
                // Two of three still agree: healed.
                Anchor {
                    name: "f".into(),
                    function: true,
                    locators: vec![
                        bytes("48 89 5c 24 ?? 57"),
                        bytes("41 b8 ?? ?? ?? ?? 48 83 c4"),
                        bytes("de ad be ef"),
                    ],
                    ..Anchor::default()
                },
                Anchor {
                    name: "f.size".into(),
                    within: Some("f".into()),
                    operand: Some(Operand::Imm),
                    locators: vec![
                        bytes("ba ?? ?? ?? ??"),
                        bytes("41 b8 ?? ?? ?? ??"),
                        bytes("bb ?? ?? ?? ??"),
                    ],
                    ..Anchor::default()
                },
                // Only its prologue left: a shape another function can share,
                // so it doesn't resolve, and nothing is rebuilt from it.
                Anchor {
                    name: "g".into(),
                    function: true,
                    locators: vec![bytes("48 89 4c 24 ?? 56"), bytes("ca fe ba be")],
                    ..Anchor::default()
                },
            ],
        };
        let report = set.resolve(&bin);
        let h = heal(&set, &report, &bin);
        let names: Vec<&str> = h.healed.iter().map(|(n, ..)| n.as_str()).collect();
        assert_eq!(names, ["f", "f.size"], "skipped: {:?}", h.skipped);
        assert_eq!(report.anchors[2].verdict, Verdict::Ambiguous);
        assert!(report.anchors[2].locators[0].prologue);
        assert!(h.skipped.is_empty(), "{:?}", h.skipped);
        let after = h.set.resolve(&bin);
        for (before, now) in report.anchors.iter().zip(&after.anchors) {
            assert_eq!(
                (before.offset(), before.value),
                (now.offset(), now.value),
                "{}",
                now.name
            );
        }
        for a in &after.anchors[..2] {
            assert!(
                a.total_locators > 2 && a.unique_locators == a.total_locators,
                "{a:?}"
            );
        }
        assert!(
            !format!("{:?}", h.set.anchors).contains("de ad be ef"),
            "lost locators are dropped"
        );
        assert_eq!(after.anchors[1].value, Some(0x70));
    }

    #[test]
    fn healing_leaves_a_set_alone_outside_a_pe() {
        let set = crate::fixtures::set();
        let report = set.resolve(b"hello world");
        let h = heal(&set, &report, b"hello world");
        assert_eq!(h.set, set);
        assert!(h.healed.is_empty());
        assert_eq!(h.skipped, [("*".to_string(), "not a PE image".to_string())]);
    }
}
