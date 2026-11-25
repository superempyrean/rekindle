//! rekindle's own settings panel: what a user does without a terminal.
//!
//! A grid of plugin cards under four tabs: the installed plugins,
//! rekindle's own plugins and the community index (both from the release
//! channel), and settings. A card opens the plugin's page: what it is, on
//! and off, removing it, and, drawn under it by the client, the plugin's
//! own settings. Also here: installing from a file (with the review screen
//! for a community or third-party plugin), reviewing a changed file or an
//! update that asks for more, automatic updates, and the developer-mode
//! switch behind its warning and a typed confirmation.
//!
//! UI-agnostic: [`Manager::panel`] is an ordinary [`Panel`] the client
//! draws like any plugin's, owned by [`OWNER`] (not a valid plugin id, so
//! no plugin can draw or answer it), and the client hands its interactions
//! to [`Manager::handle`]. What the manager can't do itself (the runtime,
//! the OS file picker, the update thread) comes back as [`Effect`]s; bring
//! the runtime in line with [`sync`].
//!
//! Only the user reaches this: plugins have no import that calls it, and
//! the client routes only its own panel's interactions here.
//!
//! A plugin's page links to what it did over the last week ([`activity`]):
//! its calls, the websites it contacted, its actions and the client's
//! elements it changed, with anything its manifest doesn't declare first.
//!
//! With developer mode on, a Developer tab adds a Reload button for each
//! plugin, the plugins' log, and a [`console`].
//!
//! A client with room for one page at a time (a phone's settings screen)
//! starts from [`Manager::menu`] instead of the grid: a row for the plugins,
//! one for the themes, and the developer-mode switch. Each row opens a
//! [`List`] of installed plugins, whose cards open the plugin's page, and
//! `back` goes from the page to its list and from the list to the menu.

mod activity;
pub mod console;

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::capability::{Capability, Trust};
use crate::home::{DEVELOPER_MODE_WARNING, Home, InstallRecord, InstallState, Installed};
use crate::manifest::{self, Manifest};
use crate::model::{
    Card, CardAction, Element, LogLevel, LogRecord, Panel, Switch, UiEvent, UiValue,
};
use crate::review::{self, Screen};
use crate::runtime::{Runtime, ledger};
use crate::signing::{self, Signature};
use crate::store;

/// The owner of the panel: not a valid plugin id (no dot).
pub const OWNER: &str = "rekindle";
/// The panel's id.
pub const PANEL: &str = "rekindle";
/// What the user types to turn developer mode on.
pub const CONFIRM_PHRASE: &str = "I understand";

/// A plugin loaded from a file for this session (`--plugin`, `rekindle
/// dev`), listed so it can be turned off and on.
#[derive(Debug, Clone)]
pub struct DevPlugin {
    pub id: String,
    pub name: String,
    pub path: String,
    pub on: bool,
}

/// Something for the client to do after an interaction.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// These installed plugins changed: [`sync`] each (`reload`: swap a
    /// running one for the file now installed).
    Sync { ids: Vec<String>, reload: bool },
    /// Developer mode changed: [`sync`] every installed plugin.
    SyncAll,
    /// Show the OS file picker for a `.wasm`; pass the choice to
    /// [`Manager::open_file`].
    PickFile,
    /// Turn a session plugin on or off.
    Dev { id: String, on: bool },
    /// Check for updates now.
    CheckUpdates,
    /// Download plugin `id` from the release channel ([`store::download`],
    /// off the UI thread) and pass the result to [`Manager::downloaded`].
    Download { id: String },
    /// Reload plugin `id`: a dev plugin from its file, an installed one
    /// from the installed file.
    Reload { id: String },
    /// Run a console action ([`console::run`]) against the client's runtime
    /// and pass the result to [`Manager::set_console_result`].
    Eval { command: String },
}

/// What [`sync`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Synced {
    Started,
    Restarted,
    Stopped,
    Unchanged,
}

/// A file on the review screen, waiting for the user's yes.
struct Pending {
    screen: Screen,
    wasm: Vec<u8>,
    sig: Option<Signature>,
    /// A downloaded update (dropped once installed or declined).
    update: bool,
}

/// The tabs of the grid, in the order shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Installed,
    Rekindle,
    Community,
    Developer,
    Settings,
}

/// The tabs everyone sees. The Developer tab goes before Settings while
/// developer mode is on ([`Manager::tabs`]).
const TABS: [(Tab, &str); 4] = [
    (Tab::Installed, "Installed"),
    (Tab::Rekindle, "rekindle"),
    (Tab::Community, "Community"),
    (Tab::Settings, "Settings"),
];

/// How many log records the Developer tab shows.
const CONSOLE_ROWS: usize = 200;

/// The installed plugins, split the way a client with one page at a time
/// lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Plugins,
    /// Theme files, and plugins that change how the client looks.
    Themes,
}

impl List {
    pub const ALL: [List; 2] = [List::Plugins, List::Themes];

    /// The list an installed plugin is in.
    pub fn of(inst: &Installed) -> List {
        let theme = inst
            .manifest
            .as_ref()
            .is_some_and(|m| m.theme || m.requests(Capability::Theming));
        if theme { List::Themes } else { List::Plugins }
    }

    /// Its id in the `list:<id>` control.
    pub fn id(self) -> &'static str {
        match self {
            List::Plugins => "plugins",
            List::Themes => "themes",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            List::Plugins => "Plugins",
            List::Themes => "Themes",
        }
    }

    fn parse(id: &str) -> Option<List> {
        List::ALL.into_iter().find(|l| l.id() == id)
    }
}

enum View {
    /// The cards of the current tab.
    Grid,
    /// The installed plugins of one list.
    List(List),
    /// One plugin's page, by id: installed, loaded from a file, or listed
    /// in the channel.
    Detail(String),
    /// What an installed plugin did, by id.
    Activity(String),
    DeveloperMode,
    Review(Box<Pending>),
    Remove {
        id: String,
        name: String,
    },
}

/// The panel's state.
pub struct Manager {
    home: Home,
    view: View,
    tab: Tab,
    /// The list the open page was reached from, which `back` returns to.
    list: Option<List>,
    notice: Option<String>,
    /// Bumped each time the confirmation field is shown, so it starts empty.
    field: u32,
    typed: String,
    /// (id, version) of downloaded updates waiting for review.
    reviews: Vec<(String, String)>,
    /// Running plugins with features this client build can't show: (id,
    /// slots), from the runtime ([`Manager::set_missing`]).
    missing: Vec<(String, Vec<String>)>,
    /// Recent plugin log records, oldest first ([`Manager::set_logs`]).
    logs: Vec<LogRecord>,
    /// The Developer tab: the plugin its log is filtered to, what is typed
    /// in the console, and the last result.
    console_filter: Option<String>,
    console_input: String,
    console_result: Option<String>,
}

/// What a slot is, as the user would say it.
pub fn slot_feature(slot: &str) -> String {
    use crate::model::{
        SLOT_ATTACH_MENU, SLOT_CHANNEL_MENU, SLOT_CHANNEL_TOOLBAR, SLOT_CHAT_BAR_BUTTON,
        SLOT_DM_LIST_BUTTON, SLOT_EXPRESSION_PICKER_TAB, SLOT_GUILD_LIST_BUTTON, SLOT_GUILD_MENU,
        SLOT_LINK_MENU, SLOT_MEDIA_MENU, SLOT_MEDIA_VIEWER_MENU, SLOT_MESSAGE_HOVER_BAR,
        SLOT_MESSAGE_MENU, SLOT_PROFILE_SECTION, SLOT_ROLE_MENU, SLOT_SELECTION_MENU,
        SLOT_SETTINGS_COG_MENU, SLOT_THREAD_MENU, SLOT_USER_AREA_BUTTON, SLOT_USER_MENU,
        SLOT_VOICE_PANEL_BUTTON, SLOT_VOICE_PANEL_STATUS,
    };
    let known = [
        (SLOT_MESSAGE_MENU, "items in the message menu"),
        (SLOT_USER_MENU, "items in the user menu"),
        (SLOT_CHANNEL_MENU, "items in the channel menu"),
        (SLOT_THREAD_MENU, "items in the thread menu"),
        (SLOT_GUILD_MENU, "items in the server menu"),
        (SLOT_ROLE_MENU, "items in the role menu"),
        (SLOT_MEDIA_MENU, "items in the image menu"),
        (SLOT_LINK_MENU, "items in the link menu"),
        (SLOT_SELECTION_MENU, "items in the text selection menu"),
        (SLOT_ATTACH_MENU, "items in the attach menu"),
        (SLOT_SETTINGS_COG_MENU, "items in the settings menu"),
        (SLOT_MESSAGE_HOVER_BAR, "message hover buttons"),
        (SLOT_CHAT_BAR_BUTTON, "chat bar buttons"),
        (SLOT_CHANNEL_TOOLBAR, "channel toolbar buttons"),
        (SLOT_USER_AREA_BUTTON, "buttons by your name"),
        (SLOT_VOICE_PANEL_BUTTON, "voice panel buttons"),
        (SLOT_VOICE_PANEL_STATUS, "voice panel status"),
        (SLOT_GUILD_LIST_BUTTON, "server list buttons"),
        (SLOT_DM_LIST_BUTTON, "DM list buttons"),
        (SLOT_PROFILE_SECTION, "profile sections"),
        (SLOT_MEDIA_VIEWER_MENU, "items in the image viewer menu"),
        (SLOT_EXPRESSION_PICKER_TAB, "tabs in the emoji picker"),
    ];
    known
        .iter()
        .find(|(s, _)| *s == slot)
        .map_or_else(|| slot.to_string(), |(_, f)| f.to_string())
}

impl Manager {
    pub fn new(home: Home) -> Self {
        let mut m = Self {
            home,
            view: View::Grid,
            tab: Tab::Installed,
            list: None,
            notice: None,
            field: 0,
            typed: String::new(),
            reviews: Vec::new(),
            missing: Vec::new(),
            logs: Vec::new(),
            console_filter: None,
            console_input: String::new(),
            console_result: None,
        };
        m.refresh_reviews();
        m
    }

    /// The home this panel manages.
    pub fn home(&self) -> &Home {
        &self.home
    }

    /// Re-read which downloaded updates are waiting (after an update check).
    pub fn refresh_reviews(&mut self) {
        let installed = self.home.installed().unwrap_or_default();
        self.reviews = installed
            .into_iter()
            .filter_map(|i| match self.home.pending_review(&i.id) {
                Ok(Some(w)) => Some((i.id, w.manifest.version)),
                _ => None,
            })
            .collect();
    }

    /// Which running plugins have features this client build can't show:
    /// (id, slots), as the runtime reports them (`PluginInfo::missing_slots`).
    pub fn set_missing(&mut self, missing: Vec<(String, Vec<String>)>) {
        self.missing = missing
            .into_iter()
            .filter(|(_, slots)| !slots.is_empty())
            .collect();
    }

    /// The client's recent plugin log records, oldest first, for the
    /// Developer tab.
    pub fn set_logs(&mut self, logs: Vec<LogRecord>) {
        self.logs = logs;
    }

    /// The result of an [`Effect::Eval`], shown under the console.
    pub fn set_console_result(&mut self, result: impl Into<String>) {
        self.console_result = Some(result.into());
    }

    /// Whether the Developer tab is showing, so the client can refresh it
    /// as logs arrive.
    pub fn console_open(&self) -> bool {
        matches!(self.view, View::Grid)
            && self.tab == Tab::Developer
            && self.home.developer_mode().unwrap_or(false)
    }

    /// The tabs shown now.
    fn tabs(&self) -> Vec<(Tab, &'static str)> {
        let mut tabs = TABS.to_vec();
        if self.home.developer_mode().unwrap_or(false) {
            tabs.insert(3, (Tab::Developer, "Developer"));
        }
        tabs
    }

    /// Show a line at the top of the panel (until the next interaction).
    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    /// The panel as it stands.
    pub fn panel(&self, dev: &[DevPlugin]) -> Panel {
        let mut el = Vec::new();
        if let Some(n) = &self.notice {
            el.push(text(n.clone()));
        }
        match &self.view {
            View::Grid => {
                let tabs = self.tabs();
                let tab = if tabs.iter().any(|(t, _)| *t == self.tab) {
                    self.tab
                } else {
                    Tab::Installed
                };
                let selected = tabs.iter().position(|(t, _)| *t == tab).unwrap_or(0) as u32;
                el.push(Element::Tabs {
                    id: "tab".into(),
                    tabs: tabs.iter().map(|(_, name)| name.to_string()).collect(),
                    selected,
                });
                match tab {
                    Tab::Installed => self.installed(dev, &mut el),
                    Tab::Rekindle => self.rekindle(&mut el),
                    Tab::Community => self.community(&mut el),
                    Tab::Developer => self.developer(dev, &mut el),
                    Tab::Settings => self.settings(&mut el),
                }
            }
            View::List(list) => self.list_view(*list, &mut el),
            View::Detail(id) => self.detail(id, dev, &mut el),
            View::Activity(id) => self.activity(id, &mut el),
            View::DeveloperMode => {
                el.push(Element::Heading {
                    text: "Turn on developer mode?".into(),
                });
                el.extend(
                    DEVELOPER_MODE_WARNING
                        .split("\n\n")
                        .map(|p| text(p.replace('\n', " "))),
                );
                el.push(Element::TextInput {
                    id: format!("confirm-{}", self.field),
                    label: format!("Type \"{CONFIRM_PHRASE}\" to turn it on"),
                    value: String::new(),
                    placeholder: CONFIRM_PHRASE.into(),
                    multiline: false,
                });
                el.push(button(
                    "developer-mode-confirm",
                    "Turn on developer mode",
                    true,
                ));
                el.push(button("cancel", "Cancel", false));
            }
            View::Review(p) => review_elements(&p.screen, &mut el),
            View::Remove { name, .. } => {
                el.push(Element::Heading {
                    text: format!("Remove {name}?"),
                });
                el.push(text("Its settings and saved data are kept, so reinstalling it brings them back, unless you remove them too."));
                el.push(button("remove-confirm", "Remove", true));
                el.push(button("remove-purge", "Remove it and its saved data", true));
                el.push(button("cancel", "Cancel", false));
            }
        }
        Panel {
            id: PANEL.into(),
            title: "rekindle".into(),
            elements: el,
        }
    }

    /// The top of the panel, for a client that shows the panel a page at a
    /// time: a card for each [`List`], saying how many of its plugins are
    /// installed and on, and the developer-mode switch. A card's id is
    /// `list:<id>`, which opens the list, and the switch is
    /// `developer-mode`. Pages opened from here are [`Manager::panel`]'s.
    pub fn menu(&self) -> Panel {
        let installed = self.home.installed().unwrap_or_default();
        let cards = List::ALL
            .into_iter()
            .map(|list| {
                let of: Vec<&Installed> =
                    installed.iter().filter(|i| List::of(i) == list).collect();
                let on = of.iter().filter(|i| i.is_ready()).count();
                Card {
                    id: format!("list:{}", list.id()),
                    title: list.title().into(),
                    meta: format!("{} installed, {on} on", of.len()),
                    text: String::new(),
                    state: String::new(),
                    action: None,
                }
            })
            .collect();
        Panel {
            id: PANEL.into(),
            title: "rekindle".into(),
            elements: vec![
                Element::Cards { cards },
                Element::Toggle {
                    id: "developer-mode".into(),
                    label: "Developer Mode".into(),
                    value: self.home.developer_mode().unwrap_or(false),
                },
            ],
        }
    }

    /// The plugin whose page is open, if one is: the client draws that
    /// plugin's own panels under rekindle's.
    pub fn detail_of(&self) -> Option<&str> {
        match &self.view {
            View::Detail(id) => Some(id),
            _ => None,
        }
    }

    /// The plugin whose "What it did" page is open, if one is. While it is,
    /// the client calls [`Runtime::flush_activity`] before it draws the
    /// panel, so the page includes the last minute.
    pub fn activity_of(&self) -> Option<&str> {
        match &self.view {
            View::Activity(id) => Some(id),
            _ => None,
        }
    }

    /// Handle an interaction with the panel. Errors are shown in the panel.
    pub fn handle(&mut self, ev: &UiEvent) -> Vec<Effect> {
        if ev.surface != PANEL {
            return Vec::new();
        }
        if ev.element.starts_with("confirm-")
            && let UiValue::Text(t) = &ev.value
        {
            self.typed.clone_from(t);
            return Vec::new();
        }
        match (ev.element.as_str(), &ev.value) {
            ("console-input", UiValue::Text(t)) => {
                self.console_input.clone_from(t);
                return Vec::new();
            }
            ("console-filter", UiValue::Selected(i)) => {
                self.console_filter = i
                    .checked_sub(1)
                    .and_then(|i| self.log_plugins().into_iter().nth(i as usize));
                return Vec::new();
            }
            _ => {}
        }
        self.notice = None;
        match self.act(&ev.element, &ev.value) {
            Ok(effects) => effects,
            Err(e) => {
                self.notice = Some(format!("{e:#}"));
                Vec::new()
            }
        }
    }

    /// The user picked a file to install (its signature, if any, is the
    /// `.sig` next to it). A first-party plugin installs at once (off). A
    /// community plugin goes to the review screen, and a third-party one
    /// does too if developer mode is on.
    pub fn open_file(&mut self, path: &Path) -> Vec<Effect> {
        self.notice = None;
        let result = (|| {
            let wasm =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let sig_path = signing::sig_path_for(path);
            let sig = sig_path
                .exists()
                .then(|| Signature::load(&sig_path))
                .transpose()?;
            let m = manifest::extract(&wasm)?.context("that file isn't a rekindle plugin")?;
            m.validate()?;
            match self.home.trust_of(&wasm, sig.as_ref())?.trust {
                Trust::FirstParty => return self.install_first_party(&m, &wasm, sig.as_ref()),
                Trust::Community => {}
                Trust::ThirdParty if self.home.developer_mode()? => {}
                Trust::ThirdParty => bail!(
                    "{} isn't part of rekindle or its community index. Installing it needs developer mode.",
                    m.name
                ),
            }
            self.start_review(wasm, sig, false)?;
            Ok(Vec::new())
        })();
        self.or_notice(result)
    }

    /// A download ([`Effect::Download`]) finished. One of rekindle's own
    /// plugins is installed, turned off. A community plugin gets its review
    /// screen. Anything else is refused: the channel lists no such file.
    pub fn downloaded(&mut self, id: &str, result: Result<store::Download>) -> Vec<Effect> {
        self.notice = None;
        let result = result.and_then(|(wasm, sig)| {
            let m = manifest::extract(&wasm)?.context("the download isn't a rekindle plugin")?;
            m.validate()?;
            if m.id != id {
                bail!("the download is {}, not {id}", m.id);
            }
            match self.home.trust_of(&wasm, sig.as_ref())?.trust {
                Trust::FirstParty => self.install_first_party(&m, &wasm, sig.as_ref()),
                Trust::Community => self.start_review(wasm, None, false).map(|()| Vec::new()),
                Trust::ThirdParty => {
                    bail!("the download of {id} isn't the file the community index lists")
                }
            }
        });
        self.or_notice(result)
    }

    /// The effects of `result`, or none with its error shown.
    fn or_notice(&mut self, result: Result<Vec<Effect>>) -> Vec<Effect> {
        result.unwrap_or_else(|e| {
            self.notice = Some(format!("{e:#}"));
            Vec::new()
        })
    }

    fn missing_of(&self, id: &str) -> Option<&[String]> {
        self.missing
            .iter()
            .find(|(m, _)| m == id)
            .map(|(_, slots)| slots.as_slice())
    }

    // ---- the grid ----

    fn installed(&self, dev: &[DevPlugin], el: &mut Vec<Element>) {
        let installed = self.home.installed().unwrap_or_default();
        if !self.missing.is_empty() {
            el.push(text("This version of Discord doesn't have every place rekindle can add to. The plugins marked below run without the features that need them."));
        }
        if installed.is_empty() && dev.is_empty() {
            el.push(text(
                "No plugins installed. The rekindle and Community tabs list what you can add.",
            ));
            return;
        }
        let mut cards: Vec<Card> = installed.iter().map(|i| self.installed_card(i)).collect();
        for d in dev {
            let (state, action) = if d.on {
                ("On", (format!("dev-off:{}", d.id), "Turn off"))
            } else {
                ("Off", (format!("dev-on:{}", d.id), "Turn on"))
            };
            let state = if self.missing_of(&d.id).is_some() {
                format!("{state} · some features unavailable")
            } else {
                state.to_string()
            };
            cards.push(card(
                &d.id,
                &d.name,
                "loaded from a file".into(),
                &d.path,
                &state,
                Some(action),
            ));
        }
        el.push(Element::Cards { cards });
    }

    /// An installed plugin's card: its state, and a button that turns it on
    /// or off when it can be.
    fn installed_card(&self, inst: &Installed) -> Card {
        let id = &inst.id;
        let version = inst.manifest.as_ref().map_or("?", |m| m.version.as_str());
        let description = inst
            .manifest
            .as_ref()
            .map_or("", |m| m.description.as_str());
        let (state, action) = match &inst.state {
            InstallState::Ready => ("On", Some((format!("disable:{id}"), "Turn off"))),
            InstallState::Off => ("Off", Some((format!("enable:{id}"), "Turn on"))),
            InstallState::NeedsReview(_) => ("Needs a review", None),
            InstallState::DeveloperModeOff => ("Off: developer mode is off", None),
            InstallState::Refused(_) => ("Can't be loaded", None),
            InstallState::Blocked(_) => ("Turned off by rekindle", None),
        };
        let mut state = state.to_string();
        if self.missing_of(id).is_some() {
            state.push_str(" · some features unavailable");
        }
        if self.reviews.iter().any(|(r, _)| r == id) {
            state.push_str(" · update to review");
        }
        let meta = format!("{version} · {}", tag(inst.trust.as_ref().map(|t| t.trust)));
        card(id, &name_of(inst), meta, description, &state, action)
    }

    /// One [`List`]: a header with a back button, then its plugins' cards.
    fn list_view(&self, list: List, el: &mut Vec<Element>) {
        let installed: Vec<Installed> = self
            .home
            .installed()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| List::of(i) == list)
            .collect();
        let on = installed.iter().filter(|i| i.is_ready()).count();
        el.push(header(
            list.title().into(),
            format!("{} installed, {on} on", installed.len()),
            None,
            None,
        ));
        if installed.is_empty() {
            el.push(text(match list {
                List::Plugins => "No plugins installed.",
                List::Themes => "No themes installed.",
            }));
            return;
        }
        let cards = installed.iter().map(|i| self.installed_card(i)).collect();
        el.push(Element::Cards { cards });
    }

    fn rekindle(&self, el: &mut Vec<Element>) {
        let items = store::first_party(&self.home);
        if items.is_empty() {
            el.push(text(
                "Nothing to show yet. Check for updates in Settings to fetch the list.",
            ));
            return;
        }
        el.push(text(
            "Written and signed by rekindle. They install turned off.",
        ));
        let cards = items
            .iter()
            .map(|(p, installed)| {
                let (state, action) = match installed {
                    Some(_) => ("Installed", None),
                    None => ("", Some((format!("add:{}", p.id), "Install"))),
                };
                card(
                    &p.id,
                    &p.name,
                    format!("{} · rekindle", p.version),
                    &p.description,
                    state,
                    action,
                )
            })
            .collect();
        el.push(Element::Cards { cards });
    }

    fn community(&self, el: &mut Vec<Element>) {
        let items = store::list(&self.home);
        if items.is_empty() {
            el.push(text(
                "Nothing to show yet. Check for updates in Settings to fetch the index.",
            ));
            return;
        }
        el.push(text("Written by people outside rekindle and checked against the community index's rules. You see what each one can do before it's installed."));
        let cards = items
            .iter()
            .map(|item| {
                let p = &item.entry.plugin;
                let (state, action) = match &item.installed {
                    Some(_) if item.is_update() => (
                        "Update available",
                        Some((format!("get:{}", p.id), "Update…")),
                    ),
                    Some(_) => ("Installed", None),
                    None => ("", Some((format!("get:{}", p.id), "Install…"))),
                };
                card(
                    &p.id,
                    &p.name,
                    format!("{} · by {}", p.version, item.entry.author),
                    &p.description,
                    state,
                    action,
                )
            })
            .collect();
        el.push(Element::Cards { cards });
    }

    fn settings(&self, el: &mut Vec<Element>) {
        let developer_mode = self.home.developer_mode().unwrap_or(false);
        el.push(Element::Heading {
            text: "Updates".into(),
        });
        let auto = !self.home.config().is_ok_and(|c| c.manual_updates);
        el.push(Element::Toggle {
            id: "auto-updates".into(),
            label: "Update plugins automatically".into(),
            value: auto,
        });
        if developer_mode {
            el.push(text("Plugins from outside rekindle are never updated automatically: you review each new version."));
        }
        el.push(button("check-updates", "Check for updates now", false));
        el.push(Element::Separator);
        el.push(Element::Heading {
            text: "Install from a file".into(),
        });
        el.push(button("install", "Install a plugin from a file…", false));
        el.push(Element::Separator);
        el.push(Element::Heading {
            text: "Developer mode".into(),
        });
        if developer_mode {
            el.push(text(
                "On: plugins from outside rekindle can be installed and run.",
            ));
            el.push(button(
                "developer-mode-off",
                "Turn off developer mode",
                false,
            ));
        } else {
            el.push(text(
                "Off: only plugins from rekindle and its community index can be installed.",
            ));
            el.push(button(
                "developer-mode-on",
                "Turn on developer mode…",
                false,
            ));
        }
    }

    // ---- a plugin's page ----

    /// One plugin's page: a header row (back, its name and who it is by,
    /// its switch, a button), then a line or two about it.
    fn detail(&self, id: &str, dev: &[DevPlugin], el: &mut Vec<Element>) {
        if let Some(d) = dev.iter().find(|d| d.id == id) {
            el.push(header(
                d.name.clone(),
                "Loaded from a file for this session".into(),
                Some(switch(format!("dev:{id}"), d.on)),
                None,
            ));
            el.push(text(d.path.clone()));
            if let Some(slots) = self.missing_of(id) {
                el.push(text(missing_line(slots, &d.name)));
            }
            return;
        }
        let channel = self.home.cached_channel();
        let wit = manifest::rekindle_wit_version();
        let community = channel.as_ref().and_then(|c| c.latest_community(id, wit));
        if let Ok(Some(inst)) = self.home.get(id) {
            let author = community.map(|c| c.author.as_str());
            self.detail_installed(&inst, author, el);
        } else if let Some(p) = channel.as_ref().and_then(|c| c.latest(id, wit)) {
            el.push(header(
                format!("{} {}", p.name, p.version),
                "Part of rekindle. It installs turned off".into(),
                None,
                Some((format!("add:{id}"), "Install")),
            ));
            if !p.description.is_empty() {
                el.push(text(p.description.clone()));
            }
        } else if let Some(c) = community {
            let p = &c.plugin;
            el.push(header(
                format!("{} {}", p.name, p.version),
                format!("By {}, from the community index", c.author),
                None,
                Some((format!("get:{id}"), "Install…")),
            ));
            if !p.description.is_empty() {
                el.push(text(p.description.clone()));
            }
            el.push(text("You see what it can do before it's installed."));
        } else {
            el.push(header(
                "Not found".into(),
                "That plugin isn't installed or listed any more".into(),
                None,
                None,
            ));
        }
    }

    /// An installed plugin's page. `author` is its author in the community
    /// index, if it is listed there.
    fn detail_installed(&self, inst: &Installed, author: Option<&str>, el: &mut Vec<Element>) {
        let id = &inst.id;
        let name = name_of(inst);
        let version = inst.manifest.as_ref().map_or("?", |m| m.version.as_str());
        let trust = inst.trust.as_ref().map(|t| t.trust);
        let meta = match (trust, author) {
            (Some(Trust::FirstParty), _) => "Part of rekindle".to_string(),
            (Some(Trust::Community), Some(author)) => {
                format!("By {author}, from the community index")
            }
            (Some(Trust::Community), None) => "From the community index".to_string(),
            _ => "Not part of rekindle or its community index".to_string(),
        };
        let toggle = matches!(inst.state, InstallState::Ready | InstallState::Off)
            .then(|| switch(format!("on:{id}"), inst.state == InstallState::Ready));
        el.push(header(
            format!("{name} {version}"),
            meta,
            toggle,
            Some((format!("remove:{id}"), "Remove")),
        ));
        if let Some(m) = inst.manifest.as_ref().filter(|m| !m.description.is_empty()) {
            el.push(text(m.description.clone()));
        }
        if let Some(slots) = self.missing_of(id) {
            el.push(text(missing_line(slots, &name)));
        }
        match &inst.state {
            InstallState::Ready | InstallState::Off => {}
            InstallState::NeedsReview(why) => {
                el.push(text(format!("Off, {why}.")));
                if self.home.developer_mode().unwrap_or(false) || trust == Some(Trust::Community) {
                    el.push(button(
                        format!("review:{id}"),
                        format!("Review {name}"),
                        false,
                    ));
                }
            }
            InstallState::DeveloperModeOff => el.push(text("Off while developer mode is off.")),
            InstallState::Refused(why) => el.push(text(format!("Can't be loaded: {why}"))),
            InstallState::Blocked(why) => {
                el.push(text(format!("Turned off by rekindle: {why}")));
            }
        }
        if let Some((_, to)) = self.reviews.iter().find(|(r, _)| r == id) {
            el.push(button(
                format!("update:{id}"),
                format!("Review the update to {name} {to}"),
                false,
            ));
        }
        if let (Some(m), Some(t)) = (&inst.manifest, &inst.trust) {
            let abilities: Vec<&str> = review::screen(m, t, None)
                .abilities
                .into_iter()
                .map(|a| a.description)
                .collect();
            if !abilities.is_empty() {
                el.push(text(format!("It can: {}", abilities.join(" · "))));
            }
            el.extend(review::element_lines(&m.elements).into_iter().map(text));
        }
        el.push(button(format!("activity:{id}"), "What it did", false));
    }

    /// An installed plugin's "What it did" page.
    fn activity(&self, id: &str, el: &mut Vec<Element>) {
        let inst = self.home.get(id).ok().flatten();
        let name = inst.as_ref().map_or_else(|| id.to_string(), name_of);
        let manifest = inst.as_ref().and_then(|i| i.manifest.as_ref());
        let days = self.home.activity(id);
        activity::page(&name, manifest, &days, ledger::today(), el);
    }

    /// The Developer tab: a Reload button per plugin, the plugins' log, and
    /// the console.
    fn developer(&self, dev: &[DevPlugin], el: &mut Vec<Element>) {
        el.push(Element::Heading {
            text: "Reload".into(),
        });
        let installed = self.home.installed().unwrap_or_default();
        let plugins: Vec<(String, String)> = dev
            .iter()
            .map(|d| (d.id.clone(), d.name.clone()))
            .chain(installed.iter().map(|i| (i.id.clone(), name_of(i))))
            .collect();
        if plugins.is_empty() {
            el.push(text("No plugins are loaded."));
        }
        for (id, name) in plugins {
            el.push(button(
                format!("reload:{id}"),
                format!("Reload {name}"),
                false,
            ));
        }
        el.push(Element::Separator);

        el.push(Element::Heading { text: "Log".into() });
        let ids = self.log_plugins();
        let selected = self
            .console_filter
            .as_ref()
            .and_then(|f| ids.iter().position(|i| i == f))
            .map_or(0, |p| p as u32 + 1);
        let mut options = vec!["All plugins".to_string()];
        options.extend(ids);
        el.push(Element::Select {
            id: "console-filter".into(),
            label: "Show".into(),
            options,
            selected,
        });
        let rows = self
            .logs
            .iter()
            .rev()
            .filter(|r| self.console_filter.as_deref().is_none_or(|f| r.plugin == f))
            .take(CONSOLE_ROWS)
            .map(|r| {
                vec![
                    level_label(r.level).into(),
                    r.plugin.clone(),
                    r.message.clone(),
                ]
            })
            .collect();
        el.push(Element::Table {
            id: "console-log".into(),
            label: String::new(),
            columns: vec!["level".into(), "plugin".into(), "message".into()],
            rows,
        });
        el.push(button("console-clear", "Clear", false));
        el.push(Element::Separator);

        el.push(Element::Heading {
            text: "Console".into(),
        });
        el.push(text(console::HELP));
        el.push(Element::TextInput {
            id: "console-input".into(),
            label: "Action".into(),
            value: self.console_input.clone(),
            placeholder: r#"{"action":"plugins"}"#.into(),
            multiline: true,
        });
        el.push(button("console-run", "Run", false));
        if let Some(r) = &self.console_result {
            el.push(Element::Text { text: r.clone() });
        }
    }

    /// The console's Run and Clear buttons.
    fn console_control(&mut self, element: &str) -> Result<Vec<Effect>> {
        if !self.home.developer_mode()? {
            bail!("the console needs developer mode");
        }
        if element == "console-clear" {
            self.logs.clear();
            self.console_result = None;
            return Ok(Vec::new());
        }
        Ok(vec![Effect::Eval {
            command: self.console_input.clone(),
        }])
    }

    /// The plugins in the log, sorted.
    fn log_plugins(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.logs.iter().map(|r| r.plugin.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    // ---- interactions ----

    fn act(&mut self, element: &str, value: &UiValue) -> Result<Vec<Effect>> {
        let on = matches!(value, UiValue::Toggled(true));
        // The menu's switch does what the Settings tab's buttons do.
        let element = match element {
            "developer-mode" if on => "developer-mode-on",
            "developer-mode" => "developer-mode-off",
            e => e,
        };
        if let ("tab", UiValue::Selected(i)) = (element, value) {
            let (tab, _) = *self.tabs().get(*i as usize).context("no such tab")?;
            self.tab = tab;
            self.view = View::Grid;
            self.list = None;
            return Ok(Vec::new());
        }
        // Plugin ids have no colon.
        if let Some((action, id)) = element.split_once(':') {
            return self.act_on(action, id, on);
        }
        match element {
            "install" => Ok(vec![Effect::PickFile]),
            "console-run" | "console-clear" => self.console_control(element),
            "back" => {
                self.back();
                Ok(Vec::new())
            }
            "check-updates" => {
                self.notice = Some("Checking for updates…".into());
                Ok(vec![Effect::CheckUpdates])
            }
            "auto-updates" => {
                self.home.set_manual_updates(!on)?;
                Ok(Vec::new())
            }
            "developer-mode-on" => {
                self.field += 1;
                self.typed.clear();
                self.view = View::DeveloperMode;
                Ok(Vec::new())
            }
            "developer-mode-confirm" => {
                if !matches!(self.view, View::DeveloperMode) {
                    bail!("developer mode isn't being turned on");
                }
                if !self.typed.trim().eq_ignore_ascii_case(CONFIRM_PHRASE) {
                    bail!("Type \"{CONFIRM_PHRASE}\" in the box to turn developer mode on.");
                }
                self.home.set_developer_mode(true)?;
                self.typed.clear();
                self.view = View::Grid;
                self.tab = Tab::Settings;
                self.notice = Some("Developer mode is on.".into());
                Ok(vec![Effect::SyncAll])
            }
            "developer-mode-off" => {
                self.home.set_developer_mode(false)?;
                self.notice = Some(
                    "Developer mode is off. Plugins from outside rekindle have stopped.".into(),
                );
                Ok(vec![Effect::SyncAll])
            }
            "install-reviewed" => {
                let View::Review(p) = std::mem::replace(&mut self.view, View::Grid) else {
                    bail!("nothing to install")
                };
                let (_, record) = self.home.install(&p.wasm, p.sig.as_ref())?;
                self.view = View::Detail(record.id.clone());
                if p.update {
                    self.home.discard_review(&record.id);
                    self.reviews.retain(|(id, _)| *id != record.id);
                }
                self.notice = Some(installed_notice(&p.screen.manifest.name, &record));
                Ok(sync_one(&record.id, true))
            }
            "remove-confirm" | "remove-purge" => {
                let View::Remove { id, name } = std::mem::replace(&mut self.view, View::Grid)
                else {
                    bail!("nothing to remove")
                };
                self.tab = Tab::Installed;
                self.view = self.list.map_or(View::Grid, View::List);
                self.home.uninstall(&id, element == "remove-purge")?;
                self.reviews.retain(|(r, _)| *r != id);
                self.notice = Some(format!("Removed {name}."));
                Ok(sync_one(&id, false))
            }
            "cancel" => {
                if let View::Review(p) = &self.view
                    && p.update
                {
                    self.home.discard_review(&p.screen.manifest.id);
                    self.reviews.retain(|(id, _)| *id != p.screen.manifest.id);
                }
                // Back to the plugin's page from its removal, else to the
                // list or the grid.
                self.view = match std::mem::replace(&mut self.view, View::Grid) {
                    View::Remove { id, .. } => View::Detail(id),
                    _ => self.list.map_or(View::Grid, View::List),
                };
                Ok(Vec::new())
            }
            other => bail!("unknown control `{other}`"),
        }
    }

    /// From what a plugin did to its page, from a plugin's page to the
    /// list it was opened from, else to the grid.
    fn back(&mut self) {
        if let View::Activity(id) = &self.view {
            self.view = View::Detail(id.clone());
        } else if let (View::Detail(_), Some(list)) = (&self.view, self.list) {
            self.view = View::List(list);
        } else {
            self.list = None;
            self.view = View::Grid;
        }
    }

    /// An interaction with one plugin: `<action>:<id>`. `on` is the value of
    /// a switch.
    fn act_on(&mut self, action: &str, id: &str, on: bool) -> Result<Vec<Effect>> {
        let dev = |on: bool| Ok(vec![Effect::Dev { id: id.into(), on }]);
        match action {
            "open" => {
                self.view = View::Detail(id.into());
                Ok(Vec::new())
            }
            "activity" => {
                self.view = View::Activity(id.into());
                Ok(Vec::new())
            }
            "list" => {
                let list = List::parse(id).with_context(|| format!("no list `{id}`"))?;
                self.list = Some(list);
                self.view = View::List(list);
                Ok(Vec::new())
            }
            // A card's buttons do what the switch on the plugin's page does.
            "on" | "enable" | "disable" => {
                let on = match action {
                    "enable" => true,
                    "disable" => false,
                    _ => on,
                };
                self.home.set_enabled(id, on)?;
                Ok(sync_one(id, false))
            }
            "reload" => {
                if !self.home.developer_mode()? {
                    bail!("reloading needs developer mode");
                }
                Ok(vec![Effect::Reload { id: id.into() }])
            }
            "dev" => dev(on),
            "dev-on" => dev(true),
            "dev-off" => dev(false),
            "review" => {
                let inst = self
                    .home
                    .get(id)?
                    .with_context(|| format!("{id} is not installed"))?;
                let wasm = inst.read_wasm()?;
                let sig = inst.sig.as_deref().map(Signature::load).transpose()?;
                self.start_review(wasm, sig, false)?;
                Ok(Vec::new())
            }
            "update" => {
                let w = self
                    .home
                    .pending_review(id)?
                    .context("that update is no longer available")?;
                self.start_review(w.wasm, w.sig, true)?;
                Ok(Vec::new())
            }
            // `add` is one of rekindle's own plugins and `get` a community
            // one. Both download from the channel; what was downloaded
            // decides the rest.
            "add" | "get" => {
                self.notice = Some("Downloading…".into());
                Ok(vec![Effect::Download { id: id.into() }])
            }
            "remove" => {
                let inst = self
                    .home
                    .get(id)?
                    .with_context(|| format!("{id} is not installed"))?;
                self.view = View::Remove {
                    id: id.into(),
                    name: name_of(&inst),
                };
                Ok(Vec::new())
            }
            _ => bail!("unknown control `{action}:{id}`"),
        }
    }

    /// Install one of rekindle's own plugins (no review) and open its page.
    fn install_first_party(
        &mut self,
        m: &Manifest,
        wasm: &[u8],
        sig: Option<&Signature>,
    ) -> Result<Vec<Effect>> {
        let (_, record) = self.home.install(wasm, sig)?;
        self.notice = Some(installed_notice(&m.name, &record));
        self.view = View::Detail(m.id.clone());
        Ok(sync_one(&m.id, true))
    }

    fn start_review(&mut self, wasm: Vec<u8>, sig: Option<Signature>, update: bool) -> Result<()> {
        let m = manifest::extract(&wasm)?.context("that file isn't a rekindle plugin")?;
        m.validate()?;
        let trust = self.home.trust_of(&wasm, sig.as_ref())?;
        let previous = self.home.record(&m.id)?;
        let screen = review::screen(&m, &trust, previous.as_ref());
        self.view = View::Review(Box::new(Pending {
            screen,
            wasm,
            sig,
            update,
        }));
        Ok(())
    }
}

/// Bring one plugin in the runtime in line with what's installed: start it
/// if it's on and not running, stop it if it's off, blocked, removed or
/// needs review, and with `reload`, swap a running one for the installed
/// file. A plugin that doesn't start is reported, not left half-loaded.
pub fn sync(home: &Home, rt: &mut Runtime, id: &str, reload: bool) -> Result<Synced> {
    let running = rt.plugin(id).is_some();
    match (home.get(id)?.filter(Installed::is_ready), running) {
        (Some(inst), false) => {
            let (wasm, trust) = inst.read_verified()?;
            rt.load(&wasm, trust)?;
            Ok(Synced::Started)
        }
        (Some(inst), true) if reload => {
            let (wasm, trust) = inst.read_verified()?;
            rt.reload(&wasm, trust)?;
            Ok(Synced::Restarted)
        }
        (None, true) => {
            rt.unload(id)?;
            Ok(Synced::Stopped)
        }
        _ => Ok(Synced::Unchanged),
    }
}

/// [`sync`] every installed plugin, and stop running ones no longer
/// installed (`loaded`: the ids the client loaded from the home).
pub fn sync_all(home: &Home, rt: &mut Runtime, loaded: &[String]) -> Vec<(String, Result<Synced>)> {
    let mut ids: Vec<String> = home
        .installed()
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.id)
        .collect();
    for id in loaded {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids.into_iter()
        .map(|id| {
            let r = sync(home, rt, &id, false);
            (id, r)
        })
        .collect()
}

/// The review screen as panel elements (the GUI form of `review::render`).
fn review_elements(s: &Screen, el: &mut Vec<Element>) {
    let m = &s.manifest;
    el.push(Element::Heading {
        text: format!("Install {} {}?", m.name, m.version),
    });
    if !m.description.is_empty() {
        el.push(text(m.description.clone()));
    }
    if !m.authors.is_empty() {
        el.push(text(format!("By {}", m.authors.join(", "))));
    }
    el.push(text(review::header(&s.trust)));
    for h in &s.headline {
        el.push(text(format!("⚠ {h}")));
    }
    if s.theme {
        el.push(text(review::THEME_NOTE));
    }
    el.extend(s.update_lines().into_iter().map(text));
    let new = |new: bool| if new { "  NEW" } else { "" };
    if s.abilities.is_empty() {
        el.push(text("It can't do anything beyond running."));
    } else {
        el.push(Element::Heading {
            text: "It can".into(),
        });
        for a in &s.abilities {
            el.push(text(format!("• {}{}", a.description, new(a.new))));
        }
    }
    // A plugin that may reach any website says so in its abilities.
    if !s.hosts.is_empty() && !s.manifest.any_host() {
        el.push(Element::Heading {
            text: "It can contact".into(),
        });
        for h in &s.hosts {
            let warn = if h.discord {
                format!(" ({})", review::DISCORD_WARNING)
            } else {
                String::new()
            };
            el.push(text(format!("• {}{warn}{}", h.host, new(h.new))));
        }
    }
    el.extend(s.elements.iter().cloned().map(text));
    if let Some(from) = &s.updates_from {
        el.push(text(review::updates_line(from)));
    }
    el.push(text(format!("Trust: {}", s.trust.describe())));
    el.push(button("install-reviewed", "Install", true));
    el.push(button("cancel", "Cancel", false));
}

fn sync_one(id: &str, reload: bool) -> Vec<Effect> {
    vec![Effect::Sync {
        ids: vec![id.to_string()],
        reload,
    }]
}

/// The notice after an install.
fn installed_notice(name: &str, record: &InstallRecord) -> String {
    let state = if record.enabled {
        "It's on."
    } else {
        "It's off: turn it on below."
    };
    format!("Installed {name} {}. {state}", record.version)
}

/// What a plugin's page says about features this client build can't show.
fn missing_line(slots: &[String], name: &str) -> String {
    let features: Vec<String> = slots.iter().map(|s| slot_feature(s)).collect();
    format!(
        "This version of Discord has no {}, so {name} runs without them.",
        review::and_list(&features)
    )
}

fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    }
}

fn name_of(inst: &Installed) -> String {
    inst.manifest
        .as_ref()
        .map_or_else(|| inst.id.clone(), |m| m.name.clone())
}

/// Where an installed plugin is from, for a card's second line.
fn tag(trust: Option<Trust>) -> &'static str {
    match trust {
        Some(Trust::FirstParty) => "rekindle",
        Some(Trust::Community) => "community",
        Some(Trust::ThirdParty) => "not part of rekindle",
        None => "unreadable",
    }
}

fn text(t: impl Into<String>) -> Element {
    Element::Text { text: t.into() }
}

fn button(id: impl Into<String>, label: impl Into<String>, danger: bool) -> Element {
    Element::Button {
        id: id.into(),
        label: label.into(),
        icon: None,
        danger,
    }
}

/// The row at the top of a plugin's page.
fn header(
    title: String,
    meta: String,
    toggle: Option<Switch>,
    action: Option<(String, &str)>,
) -> Element {
    Element::Header {
        back: "back".into(),
        title,
        meta,
        toggle,
        action: action.map(|(id, label)| CardAction {
            id,
            label: label.into(),
        }),
    }
}

fn switch(id: String, value: bool) -> Switch {
    Switch {
        id,
        label: "On".into(),
        value,
    }
}

/// Characters of a description a card shows.
const CARD_TEXT: usize = 90;

/// A card's text: the description's first [`CARD_TEXT`] characters.
fn brief(description: &str) -> String {
    let one_line = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= CARD_TEXT {
        return one_line;
    }
    let cut: String = one_line.chars().take(CARD_TEXT - 1).collect();
    format!("{}…", cut.trim_end())
}

fn card(
    id: &str,
    title: &str,
    meta: String,
    description: &str,
    state: &str,
    action: Option<(String, &str)>,
) -> Card {
    Card {
        id: format!("open:{id}"),
        title: title.into(),
        meta,
        text: brief(description),
        state: state.into(),
        action: action.map(|(id, label)| CardAction {
            id,
            label: label.into(),
        }),
    }
}
