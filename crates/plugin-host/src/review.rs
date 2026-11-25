//! The install screen for a community or third-party plugin (owner
//! decisions 2026-11-07 and 2026-11-14).
//!
//! There are no per-capability questions: the user sees one plain-language
//! summary of what the plugin can do, led by the most serious thing, what
//! changed since they last reviewed it, and answers yes or no once. A yes
//! is bound to the file's sha256 (`home::InstallRecord`). First-party
//! plugins have no screen.
//!
//! UI-agnostic: [`screen`] builds a [`Screen`], [`render`] is the terminal
//! text, and a GUI draws the same fields.

use std::collections::BTreeSet;

use crate::capability::{Capability, Trust};
use crate::home::{InstallRecord, TrustResolution, TrustSource};
use crate::manifest::{
    ANY_ELEMENT, DeclaredElements, ElementVerb, Manifest, UpdateSource, is_discord_host,
};

/// The `network` line for a plugin that may reach any website.
pub const ANY_HOST_ABILITY: &str = "make HTTPS requests to any website";

/// Shown next to Discord hosts.
pub const DISCORD_WARNING: &str = "talks to Discord's API (it never sees your token or password)";

/// The header of a third-party plugin's install screen.
pub const THIRD_PARTY_WARNING: &str =
    "This plugin is not part of rekindle. Nobody at rekindle has checked it.";

/// What the screen says about a theme file (not a warning).
pub const THEME_NOTE: &str = "It's a theme: it changes colors, fonts, sizes and which parts of the client show. It runs no code.";

/// The line at the top of the screen: the author of a community plugin,
/// else [`THIRD_PARTY_WARNING`] with a warning sign.
pub fn header(trust: &TrustResolution) -> String {
    match &trust.source {
        TrustSource::Community { author, .. } => format!(
            "This plugin is by {author}, from rekindle's community index. The index checked what it asks for against its rules, not every line of its code."
        ),
        _ => format!("⚠ {THIRD_PARTY_WARNING}"),
    }
}

/// A host the plugin can contact.
#[derive(Debug, Clone)]
pub struct Host {
    /// Lowercase, as declared in the manifest's `network`.
    pub host: String,
    /// One of Discord's: show [`DISCORD_WARNING`].
    pub discord: bool,
    /// Not contacted by the version reviewed before.
    pub new: bool,
}

/// A capability the plugin requests.
#[derive(Debug, Clone)]
pub struct Ability {
    pub cap: Capability,
    /// [`Capability::describe`].
    pub description: &'static str,
    /// Not requested by the version reviewed before.
    pub new: bool,
}

/// How the file differs from the installed version it replaces.
#[derive(Debug, Clone)]
pub struct Update {
    pub previous_version: String,
    /// The installed version was first-party; this file isn't.
    pub was_first_party: bool,
    /// Requested before, not any more.
    pub dropped: Vec<Capability>,
}

/// What the user is shown.
#[derive(Debug, Clone)]
pub struct Screen {
    pub manifest: Manifest,
    pub trust: TrustResolution,
    /// The most serious things it can do, in a sentence each (e.g. "It can
    /// read your messages and send them to api.example.com").
    pub headline: Vec<String>,
    /// A theme file ([`crate::theme`]): show [`THEME_NOTE`].
    pub theme: bool,
    /// Everything it can do, one line per capability.
    pub abilities: Vec<Ability>,
    pub hosts: Vec<Host>,
    /// What it does to the client's own elements, a sentence per verb the
    /// manifest's `elements` declares ([`element_lines`]).
    pub elements: Vec<String>,
    /// Set when a version of the plugin is installed.
    pub update: Option<Update>,
    /// The host it checks for updates (`manifest.updates`), if any.
    pub updates_from: Option<String>,
}

impl Screen {
    /// What the screen says about the installed version this file replaces:
    /// a line about the change, then what it no longer asks for. Empty when
    /// nothing is installed.
    pub fn update_lines(&self) -> Vec<String> {
        let Some(u) = &self.update else {
            return Vec::new();
        };
        let m = &self.manifest;
        let mut lines = vec![if u.was_first_party {
            format!(
                "This replaces {} {}, which was part of rekindle. This file isn't.",
                m.name, u.previous_version
            )
        } else if u.previous_version == m.version {
            "This file is different from the one you installed before.".into()
        } else {
            format!(
                "Updating from {}. NEW marks what it didn't do before.",
                u.previous_version
            )
        }];
        if !u.dropped.is_empty() {
            let dropped: Vec<&str> = u.dropped.iter().map(|c| c.as_str()).collect();
            lines.push(format!("It no longer asks for: {}", dropped.join(", ")));
        }
        lines
    }
}

/// `a`, `a and b`, `a, b and c`.
pub(crate) fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// One of the client's elements as the user would say it: "the gift
/// button" for `composer.gift`. A name rekindle has no words for is shown
/// as it is.
pub fn element_label(name: &str) -> String {
    const KNOWN: [(&str, &str); 11] = [
        ("composer", "the message box"),
        ("composer.send", "the send button"),
        ("composer.gift", "the gift button"),
        ("composer.expressions", "the emoji button"),
        ("search.entry", "the search field"),
        ("expression-picker.sheet", "the emoji picker"),
        ("expression-picker.content", "the emoji picker's contents"),
        ("expression-picker.strip", "the emoji picker's tabs"),
        ("expression-picker.tab", "a tab of the emoji picker"),
        (
            "expression-picker.indicator",
            "the emoji picker's tab marker",
        ),
        (
            "expression-picker.search",
            "the emoji picker's search field",
        ),
    ];
    KNOWN
        .iter()
        .find(|(n, _)| *n == name)
        .map_or_else(|| format!("`{name}`"), |(_, l)| (*l).to_string())
}

/// What the install screen says about the elements a plugin declares: "It
/// hides the gift button.", "It can intercept presses on any of the
/// client's elements."
pub fn element_lines(declared: &DeclaredElements) -> Vec<String> {
    ElementVerb::ALL
        .into_iter()
        .filter_map(|verb| {
            let names = declared.get(verb);
            if names.is_empty() {
                return None;
            }
            let (does, can) = match verb {
                ElementVerb::Hide => ("hides", "hide"),
                ElementVerb::Style => ("restyles", "restyle"),
                ElementVerb::Press => ("intercepts presses on", "intercept presses on"),
                ElementVerb::Insert => (
                    "adds its own elements next to",
                    "add its own elements next to",
                ),
            };
            Some(if names.iter().any(|n| n == ANY_ELEMENT) {
                format!("It can {can} any of the client's elements.")
            } else {
                let labels: Vec<String> = names.iter().map(|n| element_label(n)).collect();
                format!("It {does} {}.", and_list(&labels))
            })
        })
        .collect()
}

/// Build the screen for a plugin, given the record of the installed version
/// (if any).
pub fn screen(
    manifest: &Manifest,
    trust: &TrustResolution,
    previous: Option<&InstallRecord>,
) -> Screen {
    let requested: BTreeSet<Capability> = manifest.capabilities.iter().copied().collect();
    let has = |c: Capability| {
        requested.contains(&c) || c.implied_by().iter().any(|i| requested.contains(i))
    };
    let mut hosts: Vec<String> = Vec::new();
    for h in &manifest.network {
        let h = h.to_ascii_lowercase();
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    let any_host = manifest.any_host();
    let to = if any_host {
        "any website".to_string()
    } else {
        and_list(&hosts)
    };

    let mut headline = Vec::new();
    if has(Capability::Network) {
        if has(Capability::ReadMessages) {
            headline.push(format!("It can read your messages and send them to {to}."));
        } else if has(Capability::ReadUsers) {
            headline.push(format!(
                "It can see your friends and what people are doing, and send that to {to}."
            ));
        } else {
            headline.push(format!("It can contact {to}."));
        }
    } else if has(Capability::ReadMessages) {
        headline.push("It can read your messages (it can't send them anywhere).".into());
    }
    if has(Capability::Compose) {
        headline.push("It can change the messages you send.".into());
    }
    if has(Capability::Intercept) {
        headline
            .push("It can hide or change messages and other things before you see them.".into());
    }
    if has(Capability::InterceptHttp) {
        headline.push(
            "It can change what the client asks Discord for and what comes back, for what it can read."
                .into(),
        );
    }
    if has(Capability::Act) {
        headline
            .push("It can pin, delete, edit and vote as you when you click its buttons.".into());
    }
    if has(Capability::WireRaw) {
        headline.push("It can change anything the client shows, and add things that look like the client's own.".into());
    }
    if has(Capability::Overlay) {
        headline.push(
            "It can draw anything over the client, including things that look like the client."
                .into(),
        );
    }

    let abilities = requested
        .iter()
        .map(|&cap| Ability {
            cap,
            description: if cap == Capability::Network && any_host {
                ANY_HOST_ABILITY
            } else {
                cap.describe()
            },
            new: previous.is_some_and(|p| !p.capabilities.contains(&cap)),
        })
        .collect();
    let hosts = hosts
        .into_iter()
        .map(|host| Host {
            discord: is_discord_host(&host),
            new: previous.is_some_and(|p| !p.hosts.contains(&host)),
            host,
        })
        .collect();
    let update = previous.map(|p| Update {
        previous_version: p.version.clone(),
        was_first_party: p.trust == Trust::FirstParty,
        dropped: p.capabilities.difference(&requested).copied().collect(),
    });
    Screen {
        manifest: manifest.clone(),
        trust: trust.clone(),
        headline,
        // Only a theme file's own bytes make a manifest a theme
        // (`manifest::extract`).
        theme: manifest.theme,
        abilities,
        hosts,
        elements: element_lines(&manifest.elements),
        update,
        updates_from: manifest.updates.as_ref().map(UpdateSource::host),
    }
}

/// Whether `manifest` requests a capability or a host that the version
/// reviewed before (`previous`) didn't. A community update that doesn't
/// installs without a new review (`updates`).
pub fn asks_for_more(previous: &InstallRecord, manifest: &Manifest) -> bool {
    manifest
        .capabilities
        .iter()
        .any(|c| !previous.capabilities.contains(c))
        || manifest
            .network
            .iter()
            .any(|h| !previous.hosts.contains(&h.to_ascii_lowercase()))
}

/// What the screen says about update checks.
pub fn updates_line(host: &str) -> String {
    format!(
        "rekindle checks {host} for new versions of it; you review each one before it's installed."
    )
}

/// The screen as terminal text.
pub fn render(s: &Screen) -> String {
    let m = &s.manifest;
    let mut out = String::new();
    let mut line = |l: String| {
        out.push_str(&l);
        out.push('\n');
    };
    line(format!("{} {} ({})", m.name, m.version, m.id));
    if !m.description.is_empty() {
        line(format!("  {}", m.description));
    }
    if !m.authors.is_empty() {
        line(format!("  by {}", m.authors.join(", ")));
    }
    line(String::new());
    line(header(&s.trust));
    for h in &s.headline {
        line(format!("⚠ {h}"));
    }
    if s.theme {
        line(THEME_NOTE.into());
    }
    let update = s.update_lines();
    if !update.is_empty() {
        line(String::new());
        update.into_iter().for_each(&mut line);
    }
    line(String::new());
    if s.abilities.is_empty() {
        line("It can't do anything beyond running.".into());
    } else {
        line("It can:".into());
    }
    let new = |new: bool| if new { "  [NEW]" } else { "" };
    for a in &s.abilities {
        line(format!("  - {}{}", a.description, new(a.new)));
    }
    if s.manifest.any_host() {
        let new = s.hosts.iter().any(|h| h.new);
        line(format!(
            "It can contact any website.{}",
            if new { "  [NEW]" } else { "" }
        ));
    } else if !s.hosts.is_empty() {
        line("It can contact:".into());
        for h in &s.hosts {
            let warn = if h.discord {
                format!("  ({DISCORD_WARNING})")
            } else {
                String::new()
            };
            line(format!("  - {}{warn}{}", h.host, new(h.new)));
        }
    }
    for e in &s.elements {
        line(e.clone());
    }
    if let Some(from) = &s.updates_from {
        line(updates_line(from));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn manifest(caps: &[Capability], hosts: &[&str]) -> Manifest {
        Manifest {
            schema: 1,
            id: "dev.example.p".into(),
            name: "P".into(),
            version: "1.0.0".into(),
            description: String::new(),
            authors: vec![],
            capabilities: caps.to_vec(),
            network: hosts.iter().map(std::string::ToString::to_string).collect(),
            wit: "0.8.0".into(),
            settings: vec![],
            strings: BTreeMap::default(),
            compose_uploads: false,
            elements: crate::manifest::DeclaredElements::default(),
            updates: None,
            theme: false,
        }
    }

    fn third() -> TrustResolution {
        TrustResolution {
            trust: Trust::ThirdParty,
            sha256: "0".repeat(64),
            source: TrustSource::Unsigned,
        }
    }

    #[test]
    fn the_worst_combination_leads() {
        use Capability::*;
        let m = manifest(&[ReadMessages, Network, Ui], &["a.example", "discord.com"]);
        let s = screen(&m, &third(), None);
        assert_eq!(
            s.headline[0],
            "It can read your messages and send them to a.example and discord.com."
        );
        let text = render(&s);
        assert!(
            text.contains(THIRD_PARTY_WARNING) && text.contains(DISCORD_WARNING),
            "{text}"
        );

        let s = screen(&manifest(&[ReadMessages], &[]), &third(), None);
        assert_eq!(
            s.headline,
            vec!["It can read your messages (it can't send them anywhere).".to_string()]
        );
        assert!(
            screen(&manifest(&[Ui, Theming], &[]), &third(), None)
                .headline
                .is_empty()
        );
    }

    #[test]
    fn each_host_is_listed_once() {
        use Capability::*;
        let m = manifest(&[Network], &["a.example", "b.example", "A.example"]);
        let s = screen(&m, &third(), None);
        assert_eq!(s.headline[0], "It can contact a.example and b.example.");
        assert_eq!(s.hosts.len(), 2);
    }

    #[test]
    fn any_host_is_a_plain_line() {
        use Capability::*;
        let m = manifest(&[ReadMessages, Network], &["*"]);
        let s = screen(&m, &third(), None);
        assert_eq!(
            s.headline[0],
            "It can read your messages and send them to any website."
        );
        let net = s.abilities.iter().find(|a| a.cap == Network).unwrap();
        assert_eq!(net.description, ANY_HOST_ABILITY);
        let text = render(&s);
        assert!(text.contains("It can contact any website."), "{text}");
        assert!(!text.contains("- *"), "{text}");
        let rec = InstallRecord::new(&manifest(&[Network], &["a.example"]), &third(), true);
        assert!(asks_for_more(&rec, &m), "`*` is a new host");
    }

    #[test]
    fn declared_elements_get_a_line_each() {
        use Capability::*;
        let mut m = manifest(&[Ui], &[]);
        m.elements.hide = vec!["composer.gift".into(), "nameless.thing".into()];
        m.elements.press = vec!["composer.send".into()];
        m.elements.style = vec!["*".into()];
        let s = screen(&m, &third(), None);
        assert_eq!(
            s.elements,
            [
                "It hides the gift button and `nameless.thing`.",
                "It can restyle any of the client's elements.",
                "It intercepts presses on the send button.",
            ]
        );
        let text = render(&s);
        assert!(text.contains("It hides the gift button"), "{text}");
        assert!(
            screen(&manifest(&[Ui], &[]), &third(), None)
                .elements
                .is_empty()
        );
    }

    #[test]
    fn a_community_plugin_names_its_author_instead_of_the_warning() {
        let mut t = third();
        t.trust = Trust::Community;
        t.source = TrustSource::Community {
            author: "alice".into(),
            serial: 1,
        };
        let text = render(&screen(&manifest(&[Capability::Ui], &[]), &t, None));
        assert!(
            text.contains("by alice, from rekindle's community index")
                && !text.contains(THIRD_PARTY_WARNING),
            "{text}"
        );
    }

    #[test]
    fn asking_for_more_is_a_new_capability_or_host() {
        use Capability::*;
        let rec = InstallRecord::new(&manifest(&[Ui, Network], &["a.example"]), &third(), true);
        assert!(!asks_for_more(&rec, &manifest(&[Ui], &["A.example"])));
        assert!(asks_for_more(&rec, &manifest(&[Ui, Storage], &[])));
        assert!(asks_for_more(
            &rec,
            &manifest(&[Ui, Network], &["b.example"])
        ));
    }

    #[test]
    fn updates_mark_what_is_new() {
        use Capability::*;
        let old = manifest(&[Ui, Storage], &[]);
        let rec = InstallRecord::new(&old, &third(), true);
        let mut new = manifest(&[Ui, Network], &["a.example"]);
        new.version = "1.1.0".into();
        let s = screen(&new, &third(), Some(&rec));
        let net = s.abilities.iter().find(|a| a.cap == Network).unwrap();
        assert!(net.new);
        assert!(!s.abilities.iter().find(|a| a.cap == Ui).unwrap().new);
        assert!(s.hosts[0].new);
        assert_eq!(s.update.as_ref().unwrap().dropped, vec![Storage]);
        assert_eq!(
            s.update_lines(),
            [
                "Updating from 1.0.0. NEW marks what it didn't do before.",
                "It no longer asks for: storage"
            ]
        );
        assert!(render(&s).contains("[NEW]"));
    }
}
