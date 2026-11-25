//! A plugin's "What it did" page: its ledger's counts for the last
//! [`ledger::DAYS`] days, read from `ledger.json` in its directory
//! ([`crate::home::Home::activity`]). Anything its manifest doesn't declare
//! is listed first.
//!
//! The runtime writes a plugin's counts once a minute, so the client calls
//! [`crate::runtime::Runtime::flush_activity`] while this page is open
//! ([`super::Manager::activity_of`]).

use crate::manifest::{ElementVerb, Manifest};
use crate::model::Element;
use crate::review::element_label;
use crate::runtime::ledger::{self, Activity, Days};

use super::{header, slot_feature, text};

/// The page for the plugin called `name`, as of day `today`.
pub(super) fn page(
    name: &str,
    manifest: Option<&Manifest>,
    days: &Days,
    today: i64,
    el: &mut Vec<Element>,
) {
    el.push(header(
        format!("What {name} did"),
        format!("The last {} days", ledger::DAYS),
        None,
        None,
    ));
    let week = days.week(today);
    if week.is_empty() {
        el.push(text(format!(
            "{name} hasn't called rekindle in the last {} days.",
            ledger::DAYS
        )));
        return;
    }
    if let Some(m) = manifest {
        let flags = ledger::flags(m, &week);
        if !flags.is_empty() {
            el.push(heading("Outside its manifest"));
            el.extend(flags.iter().map(|f| text(format!("⚠ {}", f.describe()))));
        }
    }
    sections(&week, el);
    by_day(days, today, el);
}

fn sections(a: &Activity, el: &mut Vec<Element>) {
    table(
        el,
        "calls",
        "Calls to rekindle",
        ["Interface", "Calls"],
        &a.calls,
        str::to_string,
    );
    table(
        el,
        "requests",
        "Websites it contacted",
        ["Host", "Requests"],
        &a.requests,
        str::to_string,
    );
    table(
        el,
        "actions",
        "Actions",
        ["Action", "Times"],
        &a.actions,
        str::to_string,
    );
    let mut rows: Vec<(u64, Vec<String>)> = a
        .elements
        .iter()
        .flat_map(|(verb, names)| {
            names.iter().map(|(name, &n)| {
                let label = element_label(name);
                (n, vec![label, verb_label(*verb).into(), n.to_string()])
            })
        })
        .collect();
    if !rows.is_empty() {
        rows.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        el.push(heading("The client's elements"));
        el.push(Element::Table {
            id: "activity-elements".into(),
            label: String::new(),
            columns: vec!["Element".into(), "Change".into(), "Times".into()],
            rows: rows.into_iter().map(|(_, r)| r).collect(),
        });
    }
    table(
        el,
        "surfaces",
        "Surfaces it restyled",
        ["Surface", "Times"],
        &a.surfaces,
        str::to_string,
    );
    table(
        el,
        "slots",
        "Places it added to",
        ["Place", "Items"],
        &a.slots,
        slot_feature,
    );
    if a.wire_raw > 0 {
        el.push(text(format!(
            "It set engine work of its own on the client's screen {}.",
            ledger::times(a.wire_raw)
        )));
    }
}

/// Host calls on each day that has any, newest first.
fn by_day(days: &Days, today: i64, el: &mut Vec<Element>) {
    let first = ledger::day_key(today - (ledger::DAYS - 1));
    let rows: Vec<Vec<String>> = days
        .days
        .iter()
        .rev()
        .filter(|(day, a)| **day >= first && !a.is_empty())
        .map(|(day, a)| vec![day.clone(), a.total_calls().to_string()])
        .collect();
    if rows.len() > 1 {
        el.push(heading("By day"));
        el.push(Element::Table {
            id: "activity-days".into(),
            label: String::new(),
            columns: vec!["Day (UTC)".into(), "Calls".into()],
            rows,
        });
    }
}

/// A heading and a two-column table of `counts`, most first. Nothing when
/// `counts` is empty.
fn table(
    el: &mut Vec<Element>,
    id: &str,
    title: &str,
    columns: [&str; 2],
    counts: &ledger::Counts,
    label: impl Fn(&str) -> String,
) {
    if counts.is_empty() {
        return;
    }
    let mut rows: Vec<(&String, &u64)> = counts.iter().collect();
    rows.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    el.push(heading(title));
    el.push(Element::Table {
        id: format!("activity-{id}"),
        label: String::new(),
        columns: columns.iter().map(|c| (*c).to_string()).collect(),
        rows: rows
            .into_iter()
            .map(|(k, n)| vec![label(k), n.to_string()])
            .collect(),
    });
}

fn verb_label(verb: ElementVerb) -> &'static str {
    match verb {
        ElementVerb::Hide => "hidden",
        ElementVerb::Style => "restyled",
        ElementVerb::Press => "presses taken",
        ElementVerb::Insert => "elements added next to it",
    }
}

fn heading(t: &str) -> Element {
    Element::Heading { text: t.into() }
}
