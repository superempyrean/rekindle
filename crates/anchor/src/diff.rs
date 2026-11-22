//! What changed for each anchor between two resolutions of a set.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::resolve::{ResolutionReport, Verdict};

/// What happened to an anchor between a baseline build and a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    /// Resolved in both. (Its offset usually moves between builds; that alone
    /// is not a break.)
    Ok,
    /// Resolved before, not now: a broken integration point.
    Broke,
    /// Not resolved before, resolved now.
    Recovered,
    /// Still resolved, but fewer locators do: redundancy is eroding.
    Weakened,
    /// Unresolved in both.
    StillBroken,
    /// Present only in the new report (added to the anchor set).
    New,
    /// Present only in the baseline (removed from the anchor set).
    Dropped,
}

/// One anchor's verdicts in both resolutions, and what changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorChange {
    pub name: String,
    pub change: Change,
    pub before: Option<Verdict>,
    pub after: Option<Verdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_unique: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_unique: Option<usize>,
}

/// What [`diff`] found, one entry per anchor in either resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffReport {
    pub changes: Vec<AnchorChange>,
}

impl DiffReport {
    /// Anchors that broke (`Broke`): what a CI gate should fail on.
    pub fn broke(&self) -> Vec<&AnchorChange> {
        self.changes
            .iter()
            .filter(|c| c.change == Change::Broke)
            .collect()
    }
}

/// Diff two resolutions of the same anchor set (baseline vs new build).
pub fn diff(before: &ResolutionReport, after: &ResolutionReport) -> DiffReport {
    let names: BTreeSet<&str> = before
        .anchors
        .iter()
        .chain(&after.anchors)
        .map(|a| a.name.as_str())
        .collect();
    let find =
        |r: &ResolutionReport, name: &str| r.anchors.iter().find(|a| a.name == name).cloned();

    let mut changes = Vec::new();
    for name in names {
        let b = find(before, name);
        let a = find(after, name);
        let change = match (&b, &a) {
            (Some(b), Some(a)) => {
                let before_ok = b.verdict == Verdict::Resolved;
                let after_ok = a.verdict == Verdict::Resolved;
                match (before_ok, after_ok) {
                    (true, true) if a.unique_locators < b.unique_locators => Change::Weakened,
                    (true, true) => Change::Ok,
                    (true, false) => Change::Broke,
                    (false, true) => Change::Recovered,
                    (false, false) => Change::StillBroken,
                }
            }
            (None, Some(_)) => Change::New,
            (Some(_), None) => Change::Dropped,
            (None, None) => unreachable!(),
        };
        changes.push(AnchorChange {
            name: name.to_string(),
            change,
            before: b.as_ref().map(|r| r.verdict),
            after: a.as_ref().map(|r| r.verdict),
            before_unique: b.as_ref().map(|r| r.unique_locators),
            after_unique: a.as_ref().map(|r| r.unique_locators),
        });
    }
    DiffReport { changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::AnchorSet;
    use crate::fixtures::set;

    #[test]
    fn diff_classifies_breaks_and_recovery() {
        let s = set();
        let before = s.resolve(b"hello world needle"); // greeting resolved (2 unique)
        let after = s.resolve(b"hello world"); // "needle" gone -> greeting weakened (1 unique), absent still broken
        let d = diff(&before, &after);
        let by = |n: &str| d.changes.iter().find(|c| c.name == n).unwrap().change;
        assert_eq!(by("greeting"), Change::Weakened);
        assert_eq!(by("absent"), Change::StillBroken);

        // A full break: greeting resolved -> both its strings gone.
        let gone = s.resolve(b"xxxxx");
        let d2 = diff(&before, &gone);
        assert_eq!(
            d2.changes
                .iter()
                .find(|c| c.name == "greeting")
                .unwrap()
                .change,
            Change::Broke
        );
        assert_eq!(d2.broke().len(), 1);

        // Recovery.
        let d3 = diff(&gone, &before);
        assert_eq!(
            d3.changes
                .iter()
                .find(|c| c.name == "greeting")
                .unwrap()
                .change,
            Change::Recovered
        );
    }

    #[test]
    fn new_and_dropped_anchors() {
        let a = AnchorSet {
            target: String::new(),
            anchors: vec![set().anchors[0].clone()],
        }
        .resolve(b"hello world needle");
        let b = set().resolve(b"hello world needle");
        let d = diff(&a, &b);
        assert_eq!(
            d.changes
                .iter()
                .find(|c| c.name == "absent")
                .unwrap()
                .change,
            Change::New
        );
    }
}
