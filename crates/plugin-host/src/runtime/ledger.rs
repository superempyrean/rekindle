//! What each plugin did: its host calls by interface, the hosts it sent
//! requests to, the actions it took, and the client's elements, surfaces and
//! slots it changed. The manager shows it on the plugin's page, for this
//! session and the last [`DAYS`] days.
//!
//! The ledger counts and names. It keeps no message content, no URL past
//! its host, and none of the values a plugin passed. Plugins have no import
//! that reads or writes it.
//!
//! A plugin's two stores and the runtime share one [`Ledger`], which the
//! runtime keeps across reloads. A host call costs one atomic add, made in
//! the linker's getter for the call's interface ([`link`]). Everything else
//! is named and taken under a lock, on calls that allocate anyway: a
//! request, an action, an element op.
//!
//! On disk it is `plugins/<id>/ledger.json` in the home: one [`Activity`]
//! per UTC day ([`Days`]), the last [`DAYS`] kept.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use wasmtime::component::{HasSelf, Linker};

use super::state::PluginState;
use crate::bindings::wit;
pub use crate::manifest::ElementVerb;
use crate::manifest::Manifest;

/// Days kept on disk, today included.
pub const DAYS: i64 = 7;

/// Distinct names kept per kind (hosts, actions, elements and so on). The
/// rest are counted under [`OTHERS`], so a plugin can't grow the file by
/// making up names.
pub const MAX_NAMES: usize = 64;

/// Where names past [`MAX_NAMES`] are counted.
pub const OTHERS: &str = "(others)";

/// The interfaces with functions, as the ledger counts them. Type-only
/// interfaces aren't here: nothing calls them.
pub const INTERFACES: [&str; 28] = [
    "host",
    "host-events",
    "settings",
    "storage",
    "client-state",
    "users",
    "messages",
    "inspect",
    "render-control",
    "ui",
    "elements",
    "wire-raw",
    "theming",
    "overlay",
    "audio",
    "notify",
    "http",
    "clipboard",
    "local-actions",
    "reactions",
    "actions",
    "presence",
    "tweaks",
    "files",
    "compose-control",
    "moderation",
    "history",
    "view",
];

const fn index(name: &str) -> usize {
    let mut i = 0;
    while i < INTERFACES.len() {
        let (a, b) = (INTERFACES[i].as_bytes(), name.as_bytes());
        if a.len() == b.len() {
            let mut j = 0;
            while j < a.len() && a[j] == b[j] {
                j += 1;
            }
            if j == a.len() {
                return i;
            }
        }
        i += 1;
    }
    panic!("not in INTERFACES")
}

/// Link every host function, each interface with a getter that counts the
/// call in the plugin's ledger.
pub(super) fn link(linker: &mut Linker<PluginState>) -> Result<()> {
    type D = HasSelf<PluginState>;
    macro_rules! types_only {
        ($($m:ident),* $(,)?) => {$(
            wit::$m::add_to_linker::<PluginState, D>(linker, |s| s)?;
        )*};
    }
    macro_rules! counted {
        ($($m:ident = $name:literal),* $(,)?) => {$(
            wit::$m::add_to_linker::<PluginState, D>(linker, |s| {
                s.ledger.call(const { index($name) });
                s
            })?;
        )*};
    }
    types_only!(
        types,
        events,
        records,
        action_types,
        compose_types,
        command_types,
        render_types,
        notification_types,
    );
    counted!(
        host = "host",
        host_events = "host-events",
        settings = "settings",
        storage = "storage",
        client_state = "client-state",
        users = "users",
        messages = "messages",
        inspect = "inspect",
        render_control = "render-control",
        ui = "ui",
        elements = "elements",
        wire_raw = "wire-raw",
        theming = "theming",
        overlay = "overlay",
        audio = "audio",
        notify = "notify",
        http = "http",
        clipboard = "clipboard",
        local_actions = "local-actions",
        reactions = "reactions",
        actions = "actions",
        presence = "presence",
        tweaks = "tweaks",
        files = "files",
        compose_control = "compose-control",
        moderation = "moderation",
        history = "history",
        view = "view",
    );
    Ok(())
}

/// Name to count.
pub type Counts = BTreeMap<String, u64>;

/// Counts of what a plugin did, by name. Every map is name to count.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Activity {
    /// Host calls, by interface ([`INTERFACES`]).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub calls: Counts,
    /// `http.fetch` requests that passed the checks, by host.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub requests: Counts,
    /// Requests to a host the manifest's `network` doesn't cover, by host.
    /// They were refused.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub refused_hosts: Counts,
    /// Actions handed to the client, by kind (`react`, `send-message`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: Counts,
    /// Actions refused because no gesture of the user's was being
    /// delivered, by kind.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub ungestured: Counts,
    /// The client's elements by name, by what was done to them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub elements: BTreeMap<ElementVerb, Counts>,
    /// `theming.set-surface`, by surface.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub surfaces: Counts,
    /// Items added to slots, by slot.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub slots: Counts,
    /// `wire-raw.set-work` calls that set work.
    #[serde(skip_serializing_if = "is_zero")]
    pub wire_raw: u64,
    /// Host calls refused for a capability the plugin wasn't granted, by
    /// capability. The plugin trapped on each.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub denied: Counts,
}

#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde's signature")]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn bump(map: &mut Counts, key: &str, n: u64) {
    if n == 0 {
        return;
    }
    if let Some(v) = map.get_mut(key) {
        *v += n;
        return;
    }
    let key = if map.len() >= MAX_NAMES { OTHERS } else { key };
    *map.entry(key.to_string()).or_insert(0) += n;
}

fn add_counts(to: &mut Counts, from: &Counts) {
    for (k, n) in from {
        bump(to, k, *n);
    }
}

fn minus_counts(a: &Counts, base: &Counts) -> Counts {
    a.iter()
        .filter_map(|(k, n)| {
            let d = n.saturating_sub(base.get(k).copied().unwrap_or(0));
            (d > 0).then(|| (k.clone(), d))
        })
        .collect()
}

impl Activity {
    pub fn is_empty(&self) -> bool {
        *self == Activity::default()
    }

    /// Host calls in all.
    pub fn total_calls(&self) -> u64 {
        self.calls.values().sum()
    }

    /// Add `other`'s counts to these.
    pub fn add(&mut self, other: &Activity) {
        add_counts(&mut self.calls, &other.calls);
        add_counts(&mut self.requests, &other.requests);
        add_counts(&mut self.refused_hosts, &other.refused_hosts);
        add_counts(&mut self.actions, &other.actions);
        add_counts(&mut self.ungestured, &other.ungestured);
        for (verb, names) in &other.elements {
            add_counts(self.elements.entry(*verb).or_default(), names);
        }
        add_counts(&mut self.surfaces, &other.surfaces);
        add_counts(&mut self.slots, &other.slots);
        self.wire_raw += other.wire_raw;
        add_counts(&mut self.denied, &other.denied);
    }

    /// What these counts have that `base`'s don't.
    fn minus(&self, base: &Activity) -> Activity {
        let elements = self
            .elements
            .iter()
            .filter_map(|(verb, names)| {
                let d = minus_counts(names, base.elements.get(verb).unwrap_or(&Counts::new()));
                (!d.is_empty()).then_some((*verb, d))
            })
            .collect();
        Activity {
            calls: minus_counts(&self.calls, &base.calls),
            requests: minus_counts(&self.requests, &base.requests),
            refused_hosts: minus_counts(&self.refused_hosts, &base.refused_hosts),
            actions: minus_counts(&self.actions, &base.actions),
            ungestured: minus_counts(&self.ungestured, &base.ungestured),
            elements,
            surfaces: minus_counts(&self.surfaces, &base.surfaces),
            slots: minus_counts(&self.slots, &base.slots),
            wire_raw: self.wire_raw.saturating_sub(base.wire_raw),
            denied: minus_counts(&self.denied, &base.denied),
        }
    }
}

/// One plugin's counts for this session.
pub struct Ledger {
    calls: [AtomicU64; INTERFACES.len()],
    /// Everything but `calls`.
    named: Mutex<Activity>,
    /// The session's counts as of the last [`Ledger::unflushed`].
    flushed: Mutex<Activity>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            calls: std::array::from_fn(|_| AtomicU64::new(0)),
            named: Mutex::default(),
            flushed: Mutex::default(),
        }
    }
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Ledger").field(&self.session()).finish()
    }
}

impl Ledger {
    /// A host call on interface `INTERFACES[i]`.
    #[inline]
    pub(crate) fn call(&self, i: usize) {
        self.calls[i].fetch_add(1, Ordering::Relaxed);
    }

    fn with(&self, f: impl FnOnce(&mut Activity)) {
        f(&mut self.named.lock().unwrap());
    }

    pub(crate) fn request(&self, host: &str) {
        self.with(|a| bump(&mut a.requests, host, 1));
    }

    pub(crate) fn refused_host(&self, host: &str) {
        self.with(|a| bump(&mut a.refused_hosts, host, 1));
    }

    pub(crate) fn action(&self, kind: &str) {
        self.with(|a| bump(&mut a.actions, kind, 1));
    }

    pub(crate) fn ungestured(&self, kind: &str) {
        self.with(|a| bump(&mut a.ungestured, kind, 1));
    }

    pub(crate) fn element(&self, verb: ElementVerb, name: &str) {
        self.with(|a| bump(a.elements.entry(verb).or_default(), name, 1));
    }

    pub(crate) fn surface(&self, surface: &str) {
        self.with(|a| bump(&mut a.surfaces, surface, 1));
    }

    pub(crate) fn slot(&self, slot: &str) {
        self.with(|a| bump(&mut a.slots, slot, 1));
    }

    pub(crate) fn wire_raw(&self) {
        self.with(|a| a.wire_raw += 1);
    }

    pub(crate) fn denied(&self, cap: &str) {
        self.with(|a| bump(&mut a.denied, cap, 1));
    }

    /// Everything counted this session.
    pub fn session(&self) -> Activity {
        let mut a = self.named.lock().unwrap().clone();
        a.calls = INTERFACES
            .iter()
            .zip(&self.calls)
            .filter_map(|(name, n)| {
                let n = n.load(Ordering::Relaxed);
                (n > 0).then(|| ((*name).to_string(), n))
            })
            .collect();
        a
    }

    /// What was counted since the last call, for the day's record on disk.
    pub(crate) fn unflushed(&self) -> Activity {
        let now = self.session();
        let mut flushed = self.flushed.lock().unwrap();
        let delta = now.minus(&flushed);
        *flushed = now;
        delta
    }
}

/// Days since 1970-01-01, UTC.
pub fn today() -> i64 {
    (super::timefmt::now_ms() / 86_400_000) as i64
}

/// `YYYY-MM-DD` for a day since 1970-01-01.
pub fn day_key(day: i64) -> String {
    let (y, m, d) = super::timefmt::civil(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A plugin's `ledger.json`: an [`Activity`] per UTC day, by `YYYY-MM-DD`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Days {
    #[serde(default)]
    pub days: BTreeMap<String, Activity>,
}

/// The file in a plugin's directory.
pub const FILE_NAME: &str = "ledger.json";

impl Days {
    /// Read `path`. A missing or unreadable file is an empty record: the
    /// ledger is a summary, and losing it loses nothing else.
    pub fn load(path: &Path) -> Days {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
    }

    /// Add `delta` to day `day`'s counts and drop the days before the
    /// window that ends on it.
    pub fn record(&mut self, day: i64, delta: &Activity) {
        if !delta.is_empty() {
            self.days.entry(day_key(day)).or_default().add(delta);
        }
        self.prune(day);
    }

    /// Keep the [`DAYS`] days that end on `today`.
    pub fn prune(&mut self, today: i64) {
        let first = day_key(today - (DAYS - 1));
        self.days.retain(|k, _| *k >= first);
    }

    /// The counts of the [`DAYS`] days that end on `today`, added up.
    pub fn week(&self, today: i64) -> Activity {
        let first = day_key(today - (DAYS - 1));
        let last = day_key(today);
        let mut sum = Activity::default();
        for (_, a) in self.days.range(first..=last) {
            sum.add(a);
        }
        sum
    }
}

/// Something a plugin did outside what its manifest declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flag {
    /// Requests to a host its `network` doesn't cover.
    Host { host: String, count: u64 },
    /// One of the client's elements its `elements` doesn't name for this.
    Element {
        verb: ElementVerb,
        name: String,
        count: u64,
    },
    /// Host calls that needed a capability it wasn't granted.
    Capability { cap: String, count: u64 },
    /// Actions outside a gesture of the user's.
    NoGesture { action: String, count: u64 },
}

/// `once`, `3 times`.
pub(crate) fn times(n: u64) -> String {
    if n == 1 {
        "once".into()
    } else {
        format!("{n} times")
    }
}

impl Flag {
    /// One line for the plugin's page.
    pub fn describe(&self) -> String {
        match self {
            Flag::Host { host, count } => format!(
                "Tried to contact {host}, which it doesn't list, {}. rekindle refused.",
                times(*count)
            ),
            Flag::Element { verb, name, count } => {
                let did = match verb {
                    ElementVerb::Hide => "Hid",
                    ElementVerb::Style => "Restyled",
                    ElementVerb::Press => "Took presses on",
                    ElementVerb::Insert => "Added its own elements next to",
                };
                format!(
                    "{did} {}, which it doesn't list, {}.",
                    crate::review::element_label(name),
                    times(*count)
                )
            }
            Flag::Capability { cap, count } => format!(
                "Called a function that needs `{cap}`, which it wasn't granted, {}. It was stopped.",
                times(*count)
            ),
            Flag::NoGesture { action, count } => format!(
                "Tried `{action}` when you hadn't clicked anything, {}. rekindle refused.",
                times(*count)
            ),
        }
    }
}

/// What in `a` the manifest doesn't declare.
pub fn flags(manifest: &Manifest, a: &Activity) -> Vec<Flag> {
    let mut out: Vec<Flag> = a
        .refused_hosts
        .iter()
        .map(|(host, &count)| Flag::Host {
            host: host.clone(),
            count,
        })
        .collect();
    for (verb, names) in &a.elements {
        for (name, &count) in names {
            if !manifest.elements.declares(*verb, name) {
                out.push(Flag::Element {
                    verb: *verb,
                    name: name.clone(),
                    count,
                });
            }
        }
    }
    out.extend(a.denied.iter().map(|(cap, &count)| Flag::Capability {
        cap: cap.clone(),
        count,
    }));
    out.extend(a.ungestured.iter().map(|(action, &count)| Flag::NoGesture {
        action: action.clone(),
        count,
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_indices_are_their_positions() {
        for (i, name) in INTERFACES.iter().enumerate() {
            assert_eq!(index(name), i);
        }
    }

    #[test]
    fn counts_calls_and_names() {
        let l = Ledger::default();
        l.call(index("storage"));
        l.call(index("storage"));
        l.call(index("ui"));
        l.request("api.example.com");
        l.request("api.example.com");
        l.refused_host("evil.example.net");
        l.action("react");
        l.element(ElementVerb::Hide, "composer.gift");
        l.wire_raw();
        let a = l.session();
        assert_eq!(a.calls["storage"], 2);
        assert_eq!(a.calls["ui"], 1);
        assert_eq!(a.total_calls(), 3);
        assert_eq!(a.requests["api.example.com"], 2);
        assert_eq!(a.refused_hosts["evil.example.net"], 1);
        assert_eq!(a.actions["react"], 1);
        assert_eq!(a.elements[&ElementVerb::Hide]["composer.gift"], 1);
        assert_eq!(a.wire_raw, 1);
    }

    #[test]
    fn names_past_the_cap_go_under_others() {
        let l = Ledger::default();
        for i in 0..MAX_NAMES + 10 {
            l.request(&format!("h{i}.example.com"));
        }
        l.request("h0.example.com");
        let a = l.session();
        assert_eq!(a.requests.len(), MAX_NAMES + 1);
        assert_eq!(a.requests[OTHERS], 10);
        assert_eq!(a.requests["h0.example.com"], 2);
    }

    #[test]
    fn unflushed_is_what_came_since_the_last_one() {
        let l = Ledger::default();
        l.call(0);
        l.request("a.example.com");
        let first = l.unflushed();
        assert_eq!(first.calls["host"], 1);
        assert!(l.unflushed().is_empty());
        l.call(0);
        l.call(0);
        let second = l.unflushed();
        assert_eq!(second.calls["host"], 2);
        assert!(second.requests.is_empty());
        assert_eq!(l.session().calls["host"], 3);
    }

    fn calls(n: u64) -> Activity {
        Activity {
            calls: [("ui".to_string(), n)].into(),
            ..Activity::default()
        }
    }

    #[test]
    fn days_keep_a_rolling_week() {
        let mut d = Days::default();
        let start = 20_000;
        for day in start..start + 10 {
            d.record(day, &calls(1));
        }
        assert_eq!(d.days.len(), DAYS as usize);
        assert_eq!(d.days.keys().next().unwrap(), &day_key(start + 3));
        assert_eq!(d.week(start + 9).calls["ui"], 7);
        // Two days on, the two oldest fall out of the window.
        assert_eq!(d.week(start + 11).calls["ui"], 5);
        d.prune(start + 11);
        assert_eq!(d.days.len(), 5);
        d.record(start + 11, &calls(4));
        assert_eq!(d.week(start + 11).calls["ui"], 9);
    }

    #[test]
    fn day_keys_are_dates() {
        assert_eq!(day_key(0), "1970-01-01");
        assert_eq!(day_key(20_781), "2026-11-24");
    }

    #[test]
    fn days_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("rekindle-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        let l = Ledger::default();
        l.call(index("http"));
        l.request("api.example.com");
        l.element(ElementVerb::Press, "composer.send");
        l.denied("network");
        let mut d = Days::default();
        d.record(20_781, &l.unflushed());
        d.save(&path).unwrap();
        let back = Days::load(&path);
        assert_eq!(back, d);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"press\""), "{text}");
        assert!(!text.contains("https://"), "{text}");
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(Days::load(&path), Days::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn manifest() -> Manifest {
        let mut m = crate::manifest::tests::sample();
        m.network = vec!["api.example.com".into()];
        m.elements.hide = vec!["composer.gift".into()];
        m.elements.press = vec!["*".into()];
        m
    }

    #[test]
    fn flags_what_the_manifest_does_not_declare() {
        let l = Ledger::default();
        l.request("api.example.com");
        l.refused_host("evil.example.net");
        l.element(ElementVerb::Hide, "composer.gift");
        l.element(ElementVerb::Hide, "search.entry");
        l.element(ElementVerb::Press, "anything.at-all");
        l.element(ElementVerb::Insert, "composer");
        l.denied("act");
        l.ungestured("react");
        let f = flags(&manifest(), &l.session());
        assert_eq!(
            f,
            [
                Flag::Host {
                    host: "evil.example.net".into(),
                    count: 1
                },
                Flag::Element {
                    verb: ElementVerb::Hide,
                    name: "search.entry".into(),
                    count: 1
                },
                Flag::Element {
                    verb: ElementVerb::Insert,
                    name: "composer".into(),
                    count: 1
                },
                Flag::Capability {
                    cap: "act".into(),
                    count: 1
                },
                Flag::NoGesture {
                    action: "react".into(),
                    count: 1
                },
            ]
        );
        assert!(f[0].describe().contains("evil.example.net"));
    }

    #[test]
    fn nothing_undeclared_flags_nothing() {
        let l = Ledger::default();
        l.request("api.example.com");
        l.element(ElementVerb::Hide, "composer.gift");
        assert!(flags(&manifest(), &l.session()).is_empty());
    }
}
