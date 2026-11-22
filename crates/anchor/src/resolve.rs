//! Resolving an anchor set against a binary: where each locator matches,
//! and each anchor's verdict.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use rekindle_sigmatch::sig::Signature;

use crate::descriptor::{Anchor, AnchorSet, Locator};
use crate::index::{BinaryIndex, find_all};

/// Where a locator resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Status {
    /// Exactly one file offset: the confident result.
    Unique {
        offset: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        section: Option<String>,
    },
    /// More than one: the locator is no longer specific.
    Ambiguous { offsets: Vec<usize> },
    /// Not found.
    Missing,
}

impl Status {
    pub fn is_unique(&self) -> bool {
        matches!(self, Status::Unique { .. })
    }
}

/// How one locator of an anchor resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocatorResult {
    /// The locator's [label](Locator::label).
    pub label: String,
    #[serde(flatten)]
    pub status: Status,
    /// A function anchor's byte or shape locator that matched at a
    /// function's entry: its prologue. A prologue's shape follows from the
    /// registers and arguments a function uses, so another function in
    /// another build can share it. A function anchor doesn't resolve on
    /// prologues alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prologue: bool,
}

/// How an anchor fared overall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// At least one locator resolved uniquely (for a function anchor, one
    /// that isn't only its prologue).
    Resolved,
    /// None unique, but at least one matched ambiguously; or a function
    /// anchor whose only unique locators matched a prologue.
    Ambiguous,
    /// No locator found anything.
    Broken,
    /// Locators resolved uniquely but to different places, or to different
    /// values, with no majority: at least one points at the wrong thing.
    Conflict,
}

/// How one anchor resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorResult {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub verdict: Verdict,
    /// How many of the anchor's locators resolved uniquely, to the place or
    /// value the anchor resolved to: the redundancy left.
    pub unique_locators: usize,
    pub total_locators: usize,
    pub locators: Vec<LocatorResult>,
    /// For a function anchor whose unique locators disagree, where the
    /// majority of them agree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_offset: Option<usize>,
    /// For an operand anchor, the value the majority of its unique locators
    /// read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<i64>,
}

impl AnchorResult {
    /// The file offset the anchor resolved to: the majority's place, or else
    /// the first unique locator's. `None` unless the verdict is
    /// [`Verdict::Resolved`].
    pub fn offset(&self) -> Option<usize> {
        if self.verdict != Verdict::Resolved {
            return None;
        }
        self.resolved_offset.or_else(|| {
            self.locators.iter().find_map(|l| match l.status {
                Status::Unique { offset, .. } => Some(offset),
                _ => None,
            })
        })
    }

    /// Resolved, with at least two locators agreeing. One surviving locator
    /// can be the one that now finds the wrong thing, so it isn't trusted
    /// to stand for the anchor.
    pub fn corroborated(&self) -> bool {
        self.verdict == Verdict::Resolved && self.unique_locators >= 2
    }
}

/// How many anchors got each verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub total: usize,
    pub resolved: usize,
    pub ambiguous: usize,
    pub broken: usize,
    #[serde(default)]
    pub conflict: usize,
}

impl Summary {
    fn of(anchors: &[AnchorResult]) -> Self {
        let mut s = Summary {
            total: anchors.len(),
            ..Default::default()
        };
        for a in anchors {
            match a.verdict {
                Verdict::Resolved => s.resolved += 1,
                Verdict::Ambiguous => s.ambiguous += 1,
                Verdict::Broken => s.broken += 1,
                Verdict::Conflict => s.conflict += 1,
            }
        }
        s
    }
}

/// An anchor set resolved against one binary: one result per anchor, in the
/// set's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionReport {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub anchors: Vec<AnchorResult>,
    pub summary: Summary,
}

impl ResolutionReport {
    /// Parse a report `rekindle-anchor resolve --out` wrote.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("resolution report is not valid JSON")
    }

    /// The anchors that didn't resolve: broken integration points. CI treats
    /// any as a failure.
    pub fn broken(&self) -> Vec<&AnchorResult> {
        self.anchors
            .iter()
            .filter(|a| a.verdict != Verdict::Resolved)
            .collect()
    }
}

impl AnchorSet {
    /// Resolve every anchor against `binary`, each after the anchors it
    /// depends on (see [`Anchor::depends_on`]).
    pub fn resolve(&self, binary: &[u8]) -> ResolutionReport {
        let index = BinaryIndex::new(binary);
        let whole: Vec<(&str, usize)> = self
            .anchors
            .iter()
            .filter(|a| a.within.is_none())
            .flat_map(|a| &a.locators)
            .filter_map(|l| match l {
                Locator::Shape { insns, at } => Some((insns.as_str(), *at)),
                _ => None,
            })
            .collect();
        index.prime_shapes(&whole);
        // Each round resolves the anchors whose dependencies have resolved:
        // the one they're `within` (a scope may itself be scoped: a callee
        // found inside another anchor's function) and those their call-graph
        // locators name. What's left after the last round depends on an
        // anchor that is absent or in a cycle, and finds nothing: an empty
        // scope, and call-graph locators that are missing.
        let mut found: HashMap<String, usize> = HashMap::new();
        let mut results: Vec<Option<AnchorResult>> = vec![None; self.anchors.len()];
        let mut pending: Vec<usize> = (0..self.anchors.len()).collect();
        loop {
            let waiting = |w: &str| pending.iter().any(|&j| self.anchors[j].name == w);
            let ready: Vec<usize> = pending
                .iter()
                .copied()
                .filter(|&i| self.anchors[i].depends_on().all(|w| !waiting(w)))
                .collect();
            if ready.is_empty() {
                break;
            }
            for i in ready {
                let a = &self.anchors[i];
                let scope = a.within.as_ref().map(|w| {
                    found
                        .get(w)
                        .and_then(|&off| index.function_range(off))
                        .unwrap_or((0, 0))
                });
                let r = resolve_anchor(a, &index, scope, &found);
                // A scoped anchor that isn't a function names a point inside
                // a function (an operand, a layout entry), not something to
                // scope or walk from.
                if (a.within.is_none() || a.function)
                    && let Some(off) = r.offset()
                {
                    found.insert(a.name.clone(), off);
                }
                results[i] = Some(r);
            }
            pending.retain(|&i| results[i].is_none());
        }
        for i in pending {
            let a = &self.anchors[i];
            let scope = a.within.as_ref().map(|_| (0, 0));
            results[i] = Some(resolve_anchor(a, &index, scope, &HashMap::new()));
        }
        let mut anchors: Vec<AnchorResult> = results
            .into_iter()
            .map(|r| r.expect("every anchor resolves in a round or after the last"))
            .collect();
        apply_relative(&self.anchors, &mut anchors);
        ResolutionReport {
            target: self.target.clone(),
            summary: Summary::of(&anchors),
            anchors,
        }
    }
}

/// Turn each `relative_to` operand anchor's raw value into its difference
/// from its base anchor's raw value, then add any `plus`. A base that didn't
/// resolve (or isn't an operand anchor) breaks the anchor relative to it: its
/// value means nothing alone.
fn apply_relative(anchors: &[Anchor], results: &mut [AnchorResult]) {
    // Raw values first: a base is read before any anchor relative to it changes.
    let raw: HashMap<String, Option<i64>> = results
        .iter()
        .map(|r| {
            let value = r.value.filter(|_| r.verdict == Verdict::Resolved);
            (r.name.clone(), value)
        })
        .collect();
    for (a, r) in anchors.iter().zip(results.iter_mut()) {
        if let (Some(plus), Some(v)) = (a.plus, r.value) {
            r.value = Some(v + plus);
        }
        let Some(base) = &a.relative_to else { continue };
        match (r.value, raw.get(base).copied().flatten()) {
            (Some(v), Some(b)) if r.verdict == Verdict::Resolved => r.value = Some(v - b),
            _ => {
                r.value = None;
                if r.verdict == Verdict::Resolved {
                    r.verdict = Verdict::Broken;
                }
            }
        }
    }
}

/// Where `loc` matches. `found` holds the offsets of the anchors resolved so
/// far, for call-graph locators.
pub(crate) fn resolve_locator(
    loc: &Locator,
    index: &BinaryIndex,
    scope: Option<(usize, usize)>,
    found: &HashMap<String, usize>,
) -> Status {
    let hits = match loc {
        Locator::String { text } => find_all(index.bytes, text.as_bytes()),
        Locator::Symbol { name } => index.symbols.get(name).cloned().unwrap_or_default(),
        Locator::Bytes { pattern } => pattern_hits(pattern, index, scope),
        Locator::StringRef { text } => index.string_refs(text.as_bytes()),
        Locator::CallTarget { pattern } => {
            let len = Signature::parse(pattern).map_or(0, |s| s.len());
            pattern_hits(pattern, index, scope)
                .into_iter()
                .filter_map(|o| index.first_call_target(o, len))
                .collect()
        }
        Locator::Callee { from, nth } => found
            .get(from)
            .and_then(|&off| index.callees(off).get(*nth).copied())
            .into_iter()
            .collect(),
        Locator::Caller { of } => found
            .get(of)
            .map(|&off| index.callers(off))
            .unwrap_or_default(),
        Locator::Shape { insns, at } => index.shape_hits(insns, *at, scope),
        Locator::PanicLocation { file, line, col } => index.panic_refs(file, *line, *col),
    };
    index.status_from(hits)
}

/// Where a byte pattern matches: inside `scope` when scoped (an empty range,
/// when the scope anchor didn't resolve, finds nothing), else in executable
/// sections. A malformed pattern matches nowhere rather than failing the run.
fn pattern_hits(pattern: &str, index: &BinaryIndex, scope: Option<(usize, usize)>) -> Vec<usize> {
    let Ok(sig) = Signature::parse(pattern) else {
        return Vec::new();
    };
    let ranges: Vec<(usize, usize)> = match scope {
        Some(range) => vec![range],
        None if index.exec.is_empty() => vec![(0, index.bytes.len())],
        None => index.exec.clone(),
    };
    ranges
        .into_iter()
        .filter_map(|(start, end)| {
            let hits = sig.scan(index.bytes.get(start..end)?);
            Some(hits.into_iter().map(move |o| start + o))
        })
        .flatten()
        .collect()
}

/// Map a locator's hits to their functions' entries (see [`Anchor::function`]).
fn normalize(status: Status, index: &BinaryIndex) -> Status {
    let entry = |o: usize| index.function_entry(o).unwrap_or(o);
    match status {
        Status::Unique { offset, .. } => index.status_from(vec![entry(offset)]),
        Status::Ambiguous { offsets } => {
            index.status_from(offsets.into_iter().map(entry).collect())
        }
        Status::Missing => Status::Missing,
    }
}

/// Resolve one anchor's locators and give it a verdict. `scope` is the file
/// range its `within` anchor's function covers.
pub(crate) fn resolve_anchor(
    anchor: &Anchor,
    index: &BinaryIndex,
    scope: Option<(usize, usize)>,
    found: &HashMap<String, usize>,
) -> AnchorResult {
    let locators: Vec<LocatorResult> = anchor
        .locators
        .iter()
        .map(|loc| {
            let status = resolve_locator(loc, index, scope, found);
            let prologue = anchor.function
                && matches!(loc, Locator::Bytes { .. } | Locator::Shape { .. })
                && matches!(status, Status::Unique { offset, .. } if index.function_entry(offset) == Some(offset));
            let status = if anchor.function {
                normalize(status, index)
            } else {
                status
            };
            let status = match status {
                Status::Ambiguous { offsets } if anchor.any_of => {
                    index.status_from(vec![offsets[0]])
                }
                other => other,
            };
            LocatorResult {
                label: loc.label(),
                status,
                prologue,
            }
        })
        .collect();
    let mut unique = locators.iter().filter(|l| l.status.is_unique()).count();
    let mut votes: BTreeMap<usize, usize> = BTreeMap::new();
    for l in &locators {
        if let Status::Unique { offset, .. } = l.status {
            *votes.entry(offset).or_default() += 1;
        }
    }
    // Function anchors normalize every locator to one point, so their unique
    // locators vote: a strict majority wins (the rest are counted as lost,
    // which `diff` reports as weakening); a tie is a conflict.
    let mut resolved_offset = None;
    let mut conflict = false;
    if anchor.function
        && votes.len() > 1
        && let Some((&place, &n)) = votes.iter().max_by_key(|(_, n)| **n)
    {
        if n * 2 > unique {
            resolved_offset = Some(place);
            unique = n;
        } else {
            conflict = true;
        }
    }
    // Operand anchors vote on the value each unique locator's instruction
    // carries: a strict majority wins. A tie is a conflict, and no value at
    // all a broken anchor.
    let mut value = None;
    if let Some(operand) = anchor.operand {
        let mut by_value: BTreeMap<i64, usize> = BTreeMap::new();
        for l in &locators {
            if let Status::Unique { offset, .. } = l.status
                && let Some(v) = index.operand_value(offset, operand)
            {
                *by_value.entry(v).or_default() += 1;
            }
        }
        let voters: usize = by_value.values().sum();
        match by_value.iter().max_by_key(|(_, n)| **n) {
            Some((&v, &n)) if n * 2 > voters => {
                value = Some(v);
                unique = n;
            }
            Some(_) => conflict = true,
            None => unique = 0,
        }
    }
    // A function anchor needs a unique locator other than a prologue among
    // those that agree on its place.
    if anchor.function && !conflict && unique > 0 {
        let place = resolved_offset.or_else(|| votes.keys().next().copied());
        let prologues_only = locators
            .iter()
            .filter(|l| matches!(l.status, Status::Unique { offset, .. } if Some(offset) == place))
            .all(|l| l.prologue);
        if prologues_only {
            unique = 0;
            resolved_offset = None;
        }
    }
    let verdict = if conflict {
        Verdict::Conflict
    } else if unique > 0 {
        Verdict::Resolved
    } else if locators
        .iter()
        .any(|l| matches!(l.status, Status::Ambiguous { .. }) || l.prologue)
    {
        Verdict::Ambiguous
    } else {
        Verdict::Broken
    };
    AnchorResult {
        name: anchor.name.clone(),
        note: anchor.note.clone(),
        verdict,
        unique_locators: unique,
        total_locators: locators.len(),
        locators,
        resolved_offset,
        value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Operand;
    use crate::fixtures::{set, tiny_pe};

    #[test]
    fn any_of_resolves_interchangeable_hits_to_the_first() {
        let bin = b"xx ab yy ab zz".to_vec();
        let anchor = |any_of| AnchorSet {
            target: "t".into(),
            anchors: vec![Anchor {
                name: "twin".into(),
                any_of,
                locators: vec![Locator::String { text: "ab".into() }],
                ..Anchor::default()
            }],
        };
        // Two hits: ambiguous, unless the anchor says any of them will do.
        assert_eq!(
            anchor(false).resolve(&bin).anchors[0].verdict,
            Verdict::Ambiguous
        );
        let r = anchor(true).resolve(&bin);
        assert_eq!(r.anchors[0].verdict, Verdict::Resolved);
        assert_eq!(r.anchors[0].offset(), Some(3));
    }

    #[test]
    fn resolves_strings_with_verdicts() {
        let bin = b"xx hello world xx needle yy abab".to_vec();
        let r = set().resolve(&bin);
        let g = &r.anchors[0];
        assert_eq!(g.verdict, Verdict::Resolved);
        assert_eq!(g.unique_locators, 2);
        assert_eq!(g.offset(), Some(3));
        assert_eq!(r.anchors[1].verdict, Verdict::Broken);
        // "ab" occurs twice (abab): ambiguous, no unique locator.
        assert_eq!(r.anchors[2].verdict, Verdict::Ambiguous);
        assert_eq!(
            r.summary,
            Summary {
                total: 3,
                resolved: 1,
                ambiguous: 1,
                broken: 1,
                conflict: 0
            }
        );
        assert_eq!(r.broken().len(), 2);
    }

    #[test]
    fn function_anchors_flag_disagreeing_locators() {
        // Not a PE, so no normalization: two unique locators at different
        // offsets. A plain anchor is fine with that; a function anchor says
        // its locators are the same point, so it's a conflict.
        let mk = |function| AnchorSet {
            target: String::new(),
            anchors: vec![Anchor {
                name: "f".into(),
                function,
                locators: vec![
                    Locator::String {
                        text: "alpha".into(),
                    },
                    Locator::String {
                        text: "omega".into(),
                    },
                ],
                ..Anchor::default()
            }],
        };
        let bin = b"..alpha..omega..";
        assert_eq!(mk(false).resolve(bin).anchors[0].verdict, Verdict::Resolved);
        let r = mk(true).resolve(bin);
        assert_eq!(r.anchors[0].verdict, Verdict::Conflict);
        assert_eq!(r.anchors[0].offset(), None);
        assert_eq!(r.summary.conflict, 1);
        // Losing one of the two clears the conflict.
        assert_eq!(
            mk(true).resolve(b"..alpha..").anchors[0].verdict,
            Verdict::Resolved
        );
    }

    #[test]
    fn a_majority_outvotes_one_wrong_locator() {
        // Two locators agree on "alpha" (one is a longer string at the same
        // offset); one finds "omega" elsewhere. 2 of 3 is a majority.
        let set = AnchorSet {
            target: String::new(),
            anchors: vec![Anchor {
                name: "f".into(),
                function: true,
                locators: vec![
                    Locator::String {
                        text: "alpha".into(),
                    },
                    Locator::String {
                        text: "alpha..o".into(),
                    },
                    Locator::String {
                        text: "omega".into(),
                    },
                ],
                ..Anchor::default()
            }],
        };
        let r = set.resolve(b"..alpha..omega..");
        let a = &r.anchors[0];
        assert_eq!(a.verdict, Verdict::Resolved);
        assert_eq!(a.offset(), Some(2));
        assert_eq!(a.unique_locators, 2, "the outvoted locator doesn't count");
    }

    #[test]
    fn relative_operands_subtract_their_base() {
        let op = |name: &str, base: Option<&str>| Anchor {
            name: name.into(),
            operand: Some(Operand::Disp),
            relative_to: base.map(Into::into),
            ..Anchor::default()
        };
        let res = |name: &str, verdict: Verdict, value: Option<i64>| AnchorResult {
            name: name.into(),
            note: String::new(),
            verdict,
            unique_locators: 1,
            total_locators: 1,
            locators: Vec::new(),
            resolved_offset: None,
            value,
        };
        let anchors = [
            op("frame", None),
            op("field", Some("frame")),
            op("orphan", Some("gone")),
            op("on_broken", Some("broken")),
            op("broken", None),
        ];
        let mut results = vec![
            res("frame", Verdict::Resolved, Some(0x30)),
            res("field", Verdict::Resolved, Some(0x48)),
            res("orphan", Verdict::Resolved, Some(0x10)),
            res("on_broken", Verdict::Resolved, Some(0x10)),
            res("broken", Verdict::Broken, None),
        ];
        apply_relative(&anchors, &mut results);
        assert_eq!(results[0].value, Some(0x30), "a base keeps its own value");
        assert_eq!(results[1].value, Some(0x18), "field - frame");
        assert_eq!(
            (results[2].verdict, results[2].value),
            (Verdict::Broken, None),
            "no such base"
        );
        assert_eq!(
            (results[3].verdict, results[3].value),
            (Verdict::Broken, None),
            "base didn't resolve"
        );
    }

    #[test]
    fn plus_adds_to_an_operand_and_not_to_what_is_relative_to_it() {
        let anchors = [
            Anchor {
                name: "last_copy".into(),
                operand: Some(Operand::Disp),
                plus: Some(8),
                ..Anchor::default()
            },
            Anchor {
                name: "on_it".into(),
                operand: Some(Operand::Disp),
                relative_to: Some("last_copy".into()),
                ..Anchor::default()
            },
        ];
        let res = |name: &str, value: i64| AnchorResult {
            name: name.into(),
            note: String::new(),
            verdict: Verdict::Resolved,
            unique_locators: 1,
            total_locators: 1,
            locators: Vec::new(),
            resolved_offset: None,
            value: Some(value),
        };
        let mut results = vec![res("last_copy", 0x18), res("on_it", 0x20)];
        apply_relative(&anchors, &mut results);
        assert_eq!(
            results[0].value,
            Some(0x20),
            "the last qword copy's displacement + 8"
        );
        assert_eq!(
            results[1].value,
            Some(0x8),
            "relative to the operand as read"
        );
    }

    #[test]
    fn a_call_target_finds_nothing_outside_a_pe() {
        let loc = Locator::CallTarget {
            pattern: "e8 ?? ?? ?? ?? 90".into(),
        };
        let index = BinaryIndex::new(b"\xe8\0\0\0\0\x90");
        assert_eq!(
            resolve_locator(&loc, &index, None, &HashMap::new()),
            Status::Missing
        );
    }

    #[test]
    fn reports_round_trip_through_json() {
        let s = set();
        let j = serde_json::to_vec(&s).unwrap();
        assert_eq!(AnchorSet::from_json(&j).unwrap(), s);
        let r = s.resolve(b"hello world needle");
        let rj = serde_json::to_vec(&r).unwrap();
        assert_eq!(ResolutionReport::from_json(&rj).unwrap(), r);
    }

    #[test]
    fn call_graph_locators_follow_direct_calls() {
        // a @0x00: call b ; call c ; call b ; ret
        // b @0x20: ret
        // c @0x30: call b ; ret
        let mut code = vec![0xcc; 0x40];
        let call = |code: &mut Vec<u8>, at: usize, to: usize| {
            code[at] = 0xe8;
            code[at + 1..at + 5].copy_from_slice(&((to as i32) - (at as i32 + 5)).to_le_bytes());
        };
        call(&mut code, 0x00, 0x20);
        call(&mut code, 0x05, 0x30);
        call(&mut code, 0x0a, 0x20);
        code[0x0f] = 0xc3;
        code[0x20] = 0xc3;
        call(&mut code, 0x30, 0x20);
        code[0x35] = 0xc3;
        let bin = tiny_pe(&code, &[(0x00, 0x10), (0x20, 0x21), (0x30, 0x36)]);
        let entry = |rel: usize| 0x200 + rel;

        let anchor = |name: &str, locator| Anchor {
            name: name.into(),
            function: true,
            locators: vec![locator],
            ..Anchor::default()
        };
        let callee = |from: &str, nth| Locator::Callee {
            from: from.into(),
            nth,
        };
        let caller = |of: &str| Locator::Caller { of: of.into() };
        let set = AnchorSet {
            target: String::new(),
            anchors: vec![
                // Listed before `a`: still resolves after it, like `within`.
                anchor("before-a", callee("a", 0)),
                // Inside `a`, not at its entry: a function anchor doesn't
                // resolve on its prologue alone.
                anchor(
                    "a",
                    Locator::Bytes {
                        pattern: "e8 ?? ?? ?? ?? e8 ?? ?? ?? ?? c3".into(),
                    },
                ),
                anchor("b", callee("a", 0)),
                anchor("c", callee("a", 1)),
                anchor("past-end", callee("a", 2)),
                anchor("calls-c", caller("c")),
                anchor("calls-b", caller("b")),
                anchor("absent", caller("no-such-anchor")),
                // A cycle never resolves: each finds nothing, it doesn't guess.
                anchor("cyc-x", callee("cyc-y", 0)),
                anchor("cyc-y", caller("cyc-x")),
            ],
        };
        let r = set.resolve(&bin);
        let by = |n: &str| r.anchors.iter().find(|a| a.name == n).unwrap();
        assert_eq!(by("before-a").offset(), Some(entry(0x20)));
        assert_eq!(by("absent").verdict, Verdict::Broken);
        assert_eq!(by("cyc-x").verdict, Verdict::Broken);
        assert_eq!(by("cyc-y").verdict, Verdict::Broken);
        assert_eq!(by("a").offset(), Some(entry(0x00)));
        assert_eq!(by("b").offset(), Some(entry(0x20)));
        assert_eq!(by("c").offset(), Some(entry(0x30)));
        assert_eq!(by("past-end").verdict, Verdict::Broken);
        assert_eq!(by("calls-c").offset(), Some(entry(0x00)));
        assert_eq!(
            by("calls-b").locators[0].status,
            Status::Ambiguous {
                offsets: vec![entry(0x00), entry(0x30)]
            }
        );
    }
}
