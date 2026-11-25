//! The capability model: what a plugin's manifest declares it uses, and the
//! three trust levels.
//!
//! Capabilities are not something users pick (owner decision 2026-11-07):
//! a loaded plugin holds exactly what its manifest requests. They are the
//! sandbox's shape (every import must map to a requested capability, every
//! host call is checked against it) and what a reviewer reads to know what
//! a plugin can reach. Whether a plugin may load at all is decided by
//! [`Trust`] and developer mode (`home`), not here.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Capabilities, mirrored from `rekindle:plugin/types.capability`. Kept as a
/// plain host enum so policy code doesn't depend on generated bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    // seeing
    #[serde(alias = "observe-events")]
    ReadClient,
    /// Other users. `read-people`, its earlier name, still parses.
    #[serde(alias = "read-people")]
    ReadUsers,
    ReadMessages,
    RenderMessages,
    Pointer,
    // showing
    Ui,
    Theming,
    Overlay,
    Audio,
    Notifications,
    // keeping and talking
    Storage,
    Network,
    Clipboard,
    // acting as the user
    Compose,
    ActLocal,
    React,
    Act,
    Presence,
    Tweaks,
    Files,
    // changing what the client receives
    Intercept,
    // added in 0.9.0
    Moderate,
    History,
    View,
    ReadAccount,
    // added in 0.10.0
    InterceptHttp,
    // added in 0.11.0
    WireRaw,
}

impl Capability {
    pub const ALL: [Capability; 27] = [
        Capability::ReadClient,
        Capability::ReadUsers,
        Capability::ReadMessages,
        Capability::RenderMessages,
        Capability::Pointer,
        Capability::Ui,
        Capability::Theming,
        Capability::Overlay,
        Capability::Audio,
        Capability::Notifications,
        Capability::Storage,
        Capability::Network,
        Capability::Clipboard,
        Capability::Compose,
        Capability::ActLocal,
        Capability::React,
        Capability::Act,
        Capability::Presence,
        Capability::Tweaks,
        Capability::Files,
        Capability::Intercept,
        Capability::Moderate,
        Capability::History,
        Capability::View,
        Capability::ReadAccount,
        Capability::InterceptHttp,
        Capability::WireRaw,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::ReadClient => "read-client",
            Capability::ReadUsers => "read-users",
            Capability::ReadMessages => "read-messages",
            Capability::RenderMessages => "render-messages",
            Capability::Pointer => "pointer",
            Capability::Ui => "ui",
            Capability::Theming => "theming",
            Capability::Overlay => "overlay",
            Capability::Audio => "audio",
            Capability::Notifications => "notifications",
            Capability::Storage => "storage",
            Capability::Network => "network",
            Capability::Clipboard => "clipboard",
            Capability::Compose => "compose",
            Capability::ActLocal => "act-local",
            Capability::React => "react",
            Capability::Act => "act",
            Capability::Presence => "presence",
            Capability::Tweaks => "tweaks",
            Capability::Files => "files",
            Capability::Intercept => "intercept",
            Capability::Moderate => "moderate",
            Capability::History => "history",
            Capability::View => "view",
            Capability::ReadAccount => "read-account",
            Capability::InterceptHttp => "intercept-http",
            Capability::WireRaw => "wire-raw",
        }
    }

    /// Parse a manifest name. `observe-events` (0.7.0) is an alias of
    /// `read-client`, and `read-people` (the name until 2026-11-08) of
    /// `read-users`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        match s {
            "observe-events" => return Some(Capability::ReadClient),
            "read-people" => return Some(Capability::ReadUsers),
            _ => {}
        }
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// One-line, user-facing explanation, for the install screen.
    pub fn describe(self) -> &'static str {
        match self {
            Capability::ReadClient => "see which servers and channels you have and where you are",
            Capability::ReadUsers => {
                "see other people's status, activity and voice, and your friends list"
            }
            Capability::ReadMessages => {
                "read the messages you receive (it could keep them on this device)"
            }
            Capability::RenderMessages => {
                "change how messages look (it can't read or send them anywhere)"
            }
            Capability::Pointer => "see where your mouse cursor is",
            Capability::Ui => "add menu items, buttons, badges, panels, commands and keybinds",
            Capability::Theming => "change how the client looks",
            Capability::Overlay => "draw its own images on a layer over the client",
            Capability::Audio => "play its own sounds and replace the client's",
            Capability::Notifications => "show desktop notifications and filter the client's",
            Capability::Storage => "save its own data on this device",
            Capability::Network => "make HTTPS requests to the hosts listed below",
            Capability::Clipboard => {
                "put things on your clipboard when you click its items (it can't read the clipboard)"
            }
            Capability::Compose => "change the messages you send, just before they're sent",
            Capability::ActLocal => {
                "mark things read, start replies, set per-user volume and notes when you click its items"
            }
            Capability::React => "react to messages as you when you click its items",
            Capability::Act => {
                "do visible things as you (pin, delete, edit, vote, join voice) when you click its items"
            }
            Capability::Presence => "change your status and activity",
            Capability::Tweaks => "change client behaviour switches",
            Capability::Files => "save and open files you choose",
            Capability::Intercept => {
                "hide or change messages and other events before the client shows them"
            }
            Capability::Moderate => {
                "kick, ban, time out, move or server-mute people and bulk-delete messages when you click its items"
            }
            Capability::History => "read further back in a channel's messages than what's loaded",
            Capability::View => {
                "change how this client shows people, you included, and your settings (only on this device)"
            }
            Capability::ReadAccount => "see your email address, phone number and account country",
            Capability::InterceptHttp => {
                "see and change what the client asks Discord for and what comes back, for what it can read (never your login)"
            }
            Capability::WireRaw => {
                "change any part of the client's screen directly, including what the client draws itself"
            }
        }
    }

    /// The WIT interface whose import requires this capability, if any.
    /// (`pointer` and `compose` have no import: `pointer` gates an event,
    /// `compose` the `compose-handler` export.)
    pub fn interface(self) -> Option<&'static str> {
        match self {
            Capability::ReadClient => Some("client-state"),
            Capability::ReadUsers => Some("users"),
            Capability::ReadMessages => Some("messages"),
            Capability::RenderMessages => Some("render-control"),
            Capability::Ui => Some("ui"),
            Capability::Theming => Some("theming"),
            Capability::Overlay => Some("overlay"),
            Capability::Audio => Some("audio"),
            Capability::Notifications => Some("notify"),
            Capability::Storage => Some("storage"),
            Capability::Network => Some("http"),
            Capability::Clipboard => Some("clipboard"),
            Capability::ActLocal => Some("local-actions"),
            Capability::React => Some("reactions"),
            Capability::Act => Some("actions"),
            Capability::Presence => Some("presence"),
            Capability::Tweaks => Some("tweaks"),
            Capability::Files => Some("files"),
            Capability::Moderate => Some("moderation"),
            Capability::History => Some("history"),
            Capability::View => Some("view"),
            Capability::WireRaw => Some("wire-raw"),
            // `intercept` gates an export (`event-filter`) and `intercept-http`
            // another (`http-filter`); `host-events.intercept` and
            // `intercept-http` need no import of their own.
            // `read-account` gates fields of `client-state.me`.
            Capability::Pointer
            | Capability::Compose
            | Capability::Intercept
            | Capability::InterceptHttp
            | Capability::ReadAccount => None,
        }
    }

    /// The capability an imported interface requires. `inspect` needs
    /// `read-client` (and more per target, checked at call time).
    pub fn for_interface(name: &str) -> Option<Self> {
        if name == "inspect" {
            return Some(Capability::ReadClient);
        }
        // `compose` is an export first; this import only adds `split`.
        if name == "compose-control" {
            return Some(Capability::Compose);
        }
        // The client's own elements by name are UI too.
        if name == "elements" {
            return Some(Capability::Ui);
        }
        Self::ALL.into_iter().find(|c| c.interface() == Some(name))
    }

    /// Capabilities that include this one: `read-messages` implies
    /// `read-users`, `act` implies `react`.
    pub fn implied_by(self) -> &'static [Capability] {
        match self {
            Capability::ReadUsers => &[Capability::ReadMessages],
            Capability::React => &[Capability::Act],
            _ => &[],
        }
    }
}

/// The interfaces every plugin may import: logging, clocks, timers,
/// features (`host`), event subscription (`host-events`), its own schema
/// settings (`settings`), and the type-only interfaces, which grant nothing.
pub const ALWAYS_IMPORTABLE: [&str; 11] = [
    "host",
    "host-events",
    "settings",
    "types",
    "records",
    "events",
    "action-types",
    "compose-types",
    "command-types",
    "render-types",
    "notification-types",
];

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a plugin comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trust {
    /// Not from this project. Loads only with developer mode on, and only the
    /// exact file the user reviewed when installing it.
    ThirdParty,
    /// Listed in the release channel's community index
    /// ([`crate::channel::Channel::community`]): written by someone else and
    /// reviewed against the index's policy. Installs without developer mode,
    /// after the same review screen as a third-party plugin.
    Community,
    /// Signed with a project key ([`crate::signing::first_party_keys`]), or
    /// the developer's own plugin under `rekindle dev` and the test harness.
    /// Nothing to approve: the user just turns it on.
    FirstParty,
}

impl Trust {
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::ThirdParty => "third-party",
            Trust::Community => "community",
            Trust::FirstParty => "first-party",
        }
    }
}

/// What a loaded plugin holds: everything its manifest requested.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub granted: BTreeSet<Capability>,
    /// The hosts `network` may reach: exactly the declared ones.
    pub network_allowlist: BTreeSet<String>,
}

impl Grants {
    /// Granted directly, or implied by a granted capability (see
    /// [`Capability::implied_by`]).
    pub fn has(&self, cap: Capability) -> bool {
        self.granted.contains(&cap) || cap.implied_by().iter().any(|c| self.granted.contains(c))
    }
}

/// The grants for a manifest: what it requested, and its declared hosts if
/// it requested `network` (the manifest refuses one without the other).
pub fn resolve_grants(requested: &[Capability], declared_hosts: &[String]) -> Grants {
    let granted: BTreeSet<Capability> = requested.iter().copied().collect();
    let network_allowlist = if granted.contains(&Capability::Network) {
        declared_hosts
            .iter()
            .map(|h| h.to_ascii_lowercase())
            .collect()
    } else {
        BTreeSet::new()
    };
    Grants {
        granted,
        network_allowlist,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Capability::*;

    #[test]
    fn grants_are_what_was_requested() {
        let g = resolve_grants(
            &[Ui, ReadMessages, Network],
            &["API.example.com".to_string()],
        );
        assert!(g.has(Ui) && g.has(ReadMessages) && g.has(Network));
        assert!(!g.has(Theming));
        assert_eq!(
            g.network_allowlist,
            BTreeSet::from(["api.example.com".to_string()])
        );
        // No `network`, no hosts, whatever was declared.
        assert!(
            resolve_grants(&[Ui], &["a.example".to_string()])
                .network_allowlist
                .is_empty()
        );
    }

    #[test]
    fn implications() {
        let g = resolve_grants(&[ReadMessages, Act], &[]);
        assert!(g.has(ReadUsers), "read-messages implies read-users");
        assert!(g.has(React), "act implies react");
        assert!(
            !g.granted.contains(&ReadUsers),
            "implied, not granted separately"
        );
    }

    #[test]
    fn observe_events_is_an_alias() {
        assert_eq!(Capability::parse("observe-events"), Some(ReadClient));
        let c: Capability = serde_json::from_str("\"observe-events\"").unwrap();
        assert_eq!(c, ReadClient);
        assert_eq!(
            serde_json::to_string(&ReadClient).unwrap(),
            "\"read-client\""
        );
    }

    #[test]
    fn read_people_is_an_alias() {
        assert_eq!(Capability::parse("read-people"), Some(ReadUsers));
        let c: Capability = serde_json::from_str("\"read-people\"").unwrap();
        assert_eq!(c, ReadUsers);
        assert_eq!(serde_json::to_string(&ReadUsers).unwrap(), "\"read-users\"");
    }

    #[test]
    fn interface_mapping_roundtrips() {
        for cap in Capability::ALL {
            if let Some(i) = cap.interface() {
                assert_eq!(Capability::for_interface(i), Some(cap));
            }
        }
        assert_eq!(Capability::for_interface("host"), None);
        assert_eq!(Capability::for_interface("inspect"), Some(ReadClient));
        assert_eq!(Capability::for_interface("compose-control"), Some(Compose));
        assert_eq!(Capability::for_interface("elements"), Some(Ui));
    }
}
