//! Test harness for plugin authors.
//!
//! Add `plugin-host` as a dev-dependency and write ordinary `cargo test`s that
//! build your plugin, load it into a mock client, poke it, and assert:
//!
//! ```ignore
//! use rekindle_plugin_host::testing::Harness;
//! use rekindle_plugin_host::model::Color;
//!
//! #[test]
//! fn applies_background() {
//!     let mut h = Harness::for_crate(env!("CARGO_MANIFEST_DIR")).unwrap();
//!     assert_eq!(h.color("background-primary"), Some(Color::hex(0x0b0d12)));
//!     h.toggle("settings", "dim", true);
//!     assert_eq!(h.stored("dim").as_deref(), Some("true"));
//! }
//! ```
//!
//! The plugin holds everything its manifest requests (as every plugin
//! does) and runs against in-memory storage, a [`MockNet`] with no routes (tests
//! never touch the real network), and the mock's sample client (two guilds,
//! a DM, a few messages; see [`crate::backend::sample_world`]). Use
//! [`Harness::builder`] to seed storage or settings, add
//! canned HTTP responses, or start from another client world.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use anyhow::Result;

use crate::api;
use crate::backend::{MockBackend, World};
use crate::builder::{self, BuildOptions, BuildOutput};
use crate::capability::Trust;
use crate::model::{
    self, Action, ClickArea, ClickGesture, Color, LogLevel, LogRecord, Mods, Panel, UiEvent,
    UiValue,
};
use crate::net::{HttpRequest, Method, MockNet};
use crate::runtime::{PluginInfo, Runtime, RuntimeConfig, SettingsPage, Status};
use crate::settings::SettingValue;
use crate::sourcemap::SourceMap;

/// A build, and a JS plugin's source map.
type Built = (Arc<BuildOutput>, Option<Arc<SourceMap>>);

/// Builds are cached per crate for the life of the test process. The map's
/// lock is held only to find a crate's slot, so tests of different plugins
/// build at the same time, and tests of the same plugin wait for one build.
fn build_cached(crate_dir: &Path) -> Result<Built> {
    type Slot = Arc<OnceLock<std::result::Result<Built, String>>>;
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Slot>>> = OnceLock::new();
    let slot = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(crate_dir.to_path_buf())
        .or_default()
        .clone();
    let built = slot.get_or_init(|| {
        let out = builder::build(&BuildOptions {
            path: crate_dir.to_path_buf(),
            release: true,
            quiet: true,
            typecheck: true,
        })
        .map_err(|e| format!("{e:#}"))?;
        let map = out
            .source_map
            .as_deref()
            .and_then(|p| SourceMap::read(p).ok())
            .map(Arc::new);
        Ok((Arc::new(out), map))
    });
    built.clone().map_err(|e| anyhow::anyhow!(e))
}

/// Build the crate at `dir` as [`Harness::builder`] does (release, type
/// checked), once per test process.
pub fn build(dir: impl AsRef<Path>) -> Result<Arc<BuildOutput>> {
    Ok(build_cached(dir.as_ref())?.0)
}

/// A plugin to start in a [`Harness`], and the client it starts in. Made by
/// [`Harness::builder`] or [`Harness::from_wasm`].
pub struct HarnessBuilder {
    wasm: Result<Vec<u8>>,
    source_map: Option<Arc<SourceMap>>,
    /// Compiled code kept beside the build, so a plugin compiles once for
    /// all its tests (and runs).
    compile_cache: Option<PathBuf>,
    net: Arc<MockNet>,
    seed: Vec<(String, String)>,
    settings: Vec<(String, SettingValue)>,
    backend: MockBackend,
    real_network: bool,
    account: Option<u64>,
}

impl HarnessBuilder {
    /// Use real HTTPS instead of the mock (still limited to the declared hosts). For
    /// `#[ignore]`d live tests; [`Harness::requests`] stays empty.
    pub fn real_network(mut self) -> Self {
        self.real_network = true;
        self
    }

    /// Canned response for `GET url`.
    pub fn http_get(self, url: &str, status: u16, body: impl Into<Vec<u8>>) -> Self {
        self.net.route(Method::Get, url, status, body);
        self
    }

    /// Canned response for `method url`.
    pub fn http(self, method: Method, url: &str, status: u16, body: impl Into<Vec<u8>>) -> Self {
        self.net.route(method, url, status, body);
        self
    }

    /// Pre-populate the plugin's storage before it activates.
    pub fn stored(mut self, key: &str, value: &str) -> Self {
        self.seed.push((key.into(), value.into()));
        self
    }

    /// Set one of the plugin's schema settings before it activates.
    pub fn setting(mut self, key: &str, value: SettingValue) -> Self {
        self.settings.push((key.into(), value));
        self
    }

    /// Start from a custom backend (base theme, slots, world).
    pub fn backend(mut self, backend: MockBackend) -> Self {
        self.backend = backend;
        self
    }

    /// Start from another client world.
    pub fn world(mut self, world: World) -> Self {
        self.backend.world = world;
        self
    }

    /// Log in as `account` before loading (per-account storage).
    pub fn account(mut self, account: u64) -> Self {
        self.account = Some(account);
        self
    }

    /// Load and activate the plugin.
    pub fn start(self) -> Result<Harness> {
        let wasm = self.wasm?;
        let backend = Arc::new(Mutex::new(self.backend));
        let net: Arc<dyn crate::net::NetClient> = if self.real_network {
            Arc::new(crate::net::UreqClient)
        } else {
            self.net.clone()
        };
        let config = RuntimeConfig {
            net,
            compile_cache: self.compile_cache,
            ..Default::default()
        };
        let mut runtime = Runtime::new(backend.clone(), config)?;
        runtime.set_account(self.account)?;
        let insp = crate::runtime::inspect_artifact(runtime.engine(), &wasm)?;
        let id = insp.manifest.id.clone();
        if self.source_map.is_some() {
            runtime.logs().set_source_map(&id, self.source_map);
        }
        {
            let storage = runtime.storage(&id)?;
            let mut s = storage.lock().unwrap();
            for (k, v) in &self.seed {
                s.set(k, v)
                    .map_err(|e| anyhow::anyhow!("seeding storage: {e}"))?;
            }
        }
        for (k, v) in self.settings {
            runtime.seed_setting(&insp.manifest, &k, v)?;
        }
        runtime.load(&wasm, Trust::FirstParty)?;
        Ok(Harness {
            runtime,
            backend,
            net: self.net,
            id,
            wasm,
            fault_expected: false,
        })
    }
}

/// A running plugin inside a mock client.
pub struct Harness {
    runtime: Runtime,
    backend: Arc<Mutex<MockBackend>>,
    net: Arc<MockNet>,
    id: String,
    wasm: Vec<u8>,
    /// Inside `expect_fault`: a fault is the point, not a failure.
    fault_expected: bool,
}

impl Harness {
    /// Build the crate at `dir` (usually `env!("CARGO_MANIFEST_DIR")`) and
    /// configure it before starting.
    pub fn builder(dir: impl AsRef<Path>) -> HarnessBuilder {
        match build_cached(dir.as_ref()) {
            Ok((out, map)) => HarnessBuilder {
                source_map: map,
                compile_cache: out.wasm.parent().map(|d| d.join("compiled")),
                ..Self::from_result(Ok(out.bytes.clone()))
            },
            Err(e) => Self::from_result(Err(e)),
        }
    }

    /// Use an already-built component.
    pub fn from_wasm(bytes: Vec<u8>) -> HarnessBuilder {
        Self::from_result(Ok(bytes))
    }

    fn from_result(wasm: Result<Vec<u8>>) -> HarnessBuilder {
        HarnessBuilder {
            wasm,
            source_map: None,
            compile_cache: None,
            net: Arc::new(MockNet::new()),
            seed: vec![],
            settings: vec![],
            backend: MockBackend::new(),
            real_network: false,
            account: None,
        }
    }

    /// Build, load and activate with defaults.
    pub fn for_crate(dir: impl AsRef<Path>) -> Result<Self> {
        Self::builder(dir).start()
    }

    /// The plugin's id, from its manifest.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The runtime the plugin runs in, for calls the harness doesn't wrap.
    pub fn runtime(&mut self) -> &mut Runtime {
        &mut self.runtime
    }

    /// The mock client. Drop the guard before the next call into the
    /// harness, which locks it too.
    pub fn backend(&self) -> MutexGuard<'_, MockBackend> {
        self.backend.lock().unwrap()
    }

    /// Change the client world (then deliver the matching event yourself,
    /// or use the helpers below that do both).
    pub fn with_world<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.backend().world)
    }

    /// The plugin's status, grants and hooks. Panics if it was unloaded.
    pub fn info(&self) -> PluginInfo {
        self.runtime.plugin(&self.id).expect("plugin was unloaded")
    }

    /// Panics with the fault reason if the plugin has trapped.
    #[track_caller]
    pub fn assert_active(&self) {
        if let Status::Faulted(why) = self.info().status {
            panic!("plugin faulted: {why}\nlogs:\n{}", self.log_text());
        }
    }

    /// Effective color of a theme token (aliases accepted).
    pub fn color(&self, token: &str) -> Option<Color> {
        use crate::backend::ClientBackend;
        self.backend().get_color(token)
    }

    /// Effective value of any theme token (aliases accepted).
    pub fn value(&self, token: &str) -> Option<crate::model::ThemeValue> {
        use crate::backend::ClientBackend;
        self.backend().get_value(token)
    }

    /// This plugin's panel `id`, if it is showing.
    pub fn panel(&self, id: &str) -> Option<Panel> {
        self.backend().panel(&self.id, id).cloned()
    }

    /// This plugin's items in a slot (e.g. `model::SLOT_MESSAGE_MENU`).
    pub fn slot_items(&self, slot: &str) -> Vec<crate::model::SlotItem> {
        self.backend()
            .slot_items(slot)
            .into_iter()
            .filter(|(p, _)| *p == self.id)
            .map(|(_, i)| i.clone())
            .collect()
    }

    /// (slot, item) for this plugin's item `id`. Ids are per slot, so the
    /// same id may be in several: prefer the slot `target` belongs to.
    fn find_item(&self, id: &str, target: &api::Target) -> Option<(String, model::SlotItem)> {
        let b = self.backend();
        let found: Vec<(String, model::SlotItem)> = model::ALL_SLOTS
            .iter()
            .filter_map(|(s, _)| {
                b.slot_items(s)
                    .into_iter()
                    .find(|(p, i)| *p == self.id && i.id() == id)
                    .map(|(_, i)| (s.to_string(), i.clone()))
            })
            .collect();
        let kind = crate::validate::target_kind(target);
        found
            .iter()
            .find(|(s, _)| crate::validate::slot_target_kind(s) == Some(kind))
            .or(found.first())
            .cloned()
    }

    /// Simulate the user clicking one of this plugin's slot items. Panics if
    /// the plugin has no such item, or it couldn't be clicked with `target`
    /// (a user-menu item on a message, say), as a real client never would.
    #[track_caller]
    pub fn click_slot(&mut self, item: &str, target: api::Target) {
        self.click_slot_with(item, target, Mods::default());
    }

    /// [`Self::click_slot`] with modifier keys held.
    #[track_caller]
    pub fn click_slot_with(&mut self, item: &str, target: api::Target, mods: Mods) {
        let Some((slot, found)) = self.find_item(item, &target) else {
            let b = self.backend();
            let ids: Vec<String> = model::ALL_SLOTS
                .iter()
                .flat_map(|(s, _)| b.slot_items(s))
                .filter(|(p, _)| *p == self.id)
                .map(|(_, i)| i.id().to_string())
                .collect();
            drop(b);
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            panic!(
                "`{}` has no slot item `{item}` (it has: {})",
                self.id,
                list(&ids)
            );
        };
        if let Err(e) = crate::validate::check_slot_target(&slot, &found, &target) {
            panic!("{e}");
        }
        // The client only shows a menu item whose predicates hold.
        if let (model::SlotItem::MenuItem { when, .. }, api::Target::Message(m)) = (&found, &target)
        {
            let b = self.backend();
            if let Some(p) = when
                .iter()
                .find(|p| !crate::runtime::message_predicate(&*b, **p, m))
            {
                panic!("`{item}` isn't shown on that message: its `{p:?}` predicate doesn't hold");
            }
        }
        self.watch(&format!("click_slot({item:?})"), |h| {
            ok(h.runtime.slot_click(&h.id, &slot, item, target, mods));
        });
    }

    /// A target for a loaded message. Panics if `channel` has no message `id`.
    #[track_caller]
    pub fn message_target(&self, channel: u64, id: u64) -> api::Target {
        api::Target::Message(self.message_ref(channel, id))
    }

    /// A reference to a loaded message. Panics if `channel` has no message `id`.
    #[track_caller]
    pub fn message_ref(&self, channel: u64, id: u64) -> api::MessageRef {
        let b = self.backend();
        let m = b
            .world
            .find_message(channel, id)
            .unwrap_or_else(|| panic!("no loaded message {id} in channel {channel}"));
        api::MessageRef {
            id: m.id,
            channel: m.channel,
            guild: m.guild,
            author: m.author.id,
        }
    }

    /// A user target, as shown in `guild` (if any).
    pub fn user_target(&self, id: u64, guild: Option<u64>) -> api::Target {
        let name = self.backend().name_of(id);
        api::Target::User((api::UserRef { id, name }, guild))
    }

    /// A channel target. Panics if the client has no channel `id`.
    #[track_caller]
    pub fn channel_target(&self, id: u64) -> api::Target {
        api::Target::Channel(self.channel_ref(id))
    }

    #[track_caller]
    fn channel_ref(&self, id: u64) -> api::ChannelRef {
        use crate::backend::ClientBackend;
        let c = self
            .backend()
            .channel(id)
            .unwrap_or_else(|| panic!("no channel {id}"));
        api::ChannelRef {
            id: c.id,
            name: c.name,
            guild: c.guild,
            kind: c.kind,
        }
    }

    /// A guild target. Panics if the client has no guild `id`.
    #[track_caller]
    pub fn guild_target(&self, id: u64) -> api::Target {
        use crate::backend::ClientBackend;
        let g = self
            .backend()
            .guild(id)
            .unwrap_or_else(|| panic!("no guild {id}"));
        api::Target::Guild(api::GuildRef {
            id: g.id,
            name: g.name,
        })
    }

    /// The channel the user is looking at, as a target (for chat-bar and
    /// toolbar buttons). Panics if no channel is selected.
    #[track_caller]
    pub fn current_channel(&self) -> api::Target {
        api::Target::Channel(self.selected_channel())
    }

    #[track_caller]
    fn selected_channel(&self) -> api::ChannelRef {
        let ch = self
            .backend()
            .world
            .selection
            .channel
            .expect("no channel selected");
        self.channel_ref(ch)
    }

    /// Simulate the user opening someone's profile.
    #[track_caller]
    pub fn open_profile(&mut self, id: u64, name: &str) {
        let user = api::UserRef {
            id,
            name: name.into(),
        };
        self.watch("open_profile", |h| h.runtime.open_profile(&user));
    }

    /// What this plugin's profile section `section` shows for `user`.
    pub fn profile_section(&self, section: &str, user: u64) -> Option<Vec<crate::model::Element>> {
        self.backend()
            .profile_section(&self.id, section, user)
            .map(<[model::Element]>::to_vec)
    }

    /// This plugin's toasts, oldest first.
    pub fn toasts(&self) -> Vec<crate::model::Toast> {
        self.backend()
            .toasts()
            .iter()
            .filter(|t| t.plugin == self.id)
            .cloned()
            .collect()
    }

    /// What this plugin last put on the clipboard.
    pub fn clipboard(&self) -> Option<String> {
        self.backend()
            .clipboard()
            .filter(|(p, _)| *p == self.id)
            .map(|(_, t)| t.to_string())
    }

    /// Simulate the user sending `text` to `channel`: runs it through every
    /// `compose` plugin and returns what would be sent.
    #[track_caller]
    pub fn send_message(&mut self, channel: u64, text: &str) -> crate::model::Composed {
        self.watch("send_message", |h| h.runtime.compose_text(channel, text))
    }

    /// Run a full draft (a reply, an edit, attachments) through compose.
    #[track_caller]
    pub fn compose(&mut self, draft: &api::Draft) -> crate::model::Composed {
        self.watch("compose", |h| h.runtime.compose(draft))
    }

    /// A reply to a loaded message, as the composer would build it.
    #[track_caller]
    pub fn reply_draft(&self, channel: u64, to: u64, text: &str, mention: bool) -> api::Draft {
        let r = self.message_ref(channel, to);
        let name = self.backend().name_of(r.author);
        api::Draft {
            channel,
            guild: r.guild,
            kind: api::ComposeKind::Send,
            editing: None,
            text: text.into(),
            reply: Some(api::ReplyInfo {
                message: r,
                author: api::UserRef { id: r.author, name },
                mention,
            }),
            attachments: vec![],
            sticker_count: 0,
            silent: false,
        }
    }

    /// Offer an attachment to `compose-uploads` plugins.
    #[track_caller]
    pub fn upload(
        &mut self,
        filename: &str,
        content_type: Option<&str>,
        data: Vec<u8>,
    ) -> crate::model::UploadOutcome {
        let file = api::Upload {
            filename: filename.into(),
            content_type: content_type.map(Into::into),
            data,
        };
        self.watch("upload", |h| h.runtime.upload(&file))
    }

    /// This plugin's modal, if one is open.
    pub fn modal(&self) -> Option<Panel> {
        self.backend().modal(&self.id).cloned()
    }

    /// The ids of this plugin's palette commands.
    pub fn commands(&self) -> Vec<String> {
        self.backend()
            .commands()
            .iter()
            .filter(|c| c.plugin == self.id)
            .map(|c| c.id.clone())
            .collect()
    }

    /// A value in the plugin's storage, for the current account.
    pub fn stored(&mut self, key: &str) -> Option<String> {
        let s = self.runtime.storage(&self.id).ok()?;

        s.lock().unwrap().get(key).map(str::to_string)
    }

    /// This plugin's log records, oldest first.
    pub fn logs(&self) -> Vec<LogRecord> {
        self.runtime
            .logs()
            .snapshot()
            .into_iter()
            .filter(|r| r.plugin == self.id)
            .collect()
    }

    /// This plugin's log as text, one `[Level] message` line per record.
    pub fn log_text(&self) -> String {
        self.logs()
            .iter()
            .map(|r| format!("[{:?}] {}", r.level, r.message))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Did the plugin log a message containing `needle` at `level` or above?
    pub fn logged(&self, level: LogLevel, needle: &str) -> bool {
        self.logs()
            .iter()
            .any(|r| r.level >= level && r.message.contains(needle))
    }

    /// Requests the plugin sent to the (mock) network.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.net.requests()
    }

    /// Simulate the user changing an element on one of this plugin's
    /// surfaces: a panel or modal (by id), `settings-page` (the generated
    /// settings page's extra elements), or `message:<id>` (an accessory). Panics if the
    /// surface or element doesn't exist, or the element doesn't take that
    /// kind of value: a real client can only send events for controls the
    /// plugin showed.
    #[track_caller]
    pub fn ui(&mut self, surface: &str, element: &str, value: UiValue) {
        self.ui_with(surface, element, value, Mods::default());
    }

    /// [`Self::ui`] with modifier keys held.
    #[track_caller]
    pub fn ui_with(&mut self, surface: &str, element: &str, value: UiValue, mods: Mods) {
        {
            let b = self.backend();
            // A panel or modal by id first, then the reserved surfaces.
            let elements: Option<Vec<crate::model::Element>> = b
                .panel(&self.id, surface)
                .or_else(|| b.modal(&self.id).filter(|m| m.id == surface))
                .map(|p| p.elements.clone())
                .or_else(|| {
                    (surface == model::SURFACE_SETTINGS_PAGE)
                        .then(|| b.settings_extra(&self.id).to_vec())
                })
                .or_else(|| {
                    let m = surface.strip_prefix("message:")?.parse().ok()?;
                    b.accessory(&self.id, m).map(<[_]>::to_vec)
                });
            let Some(elements) = elements else {
                let ids: Vec<&str> = b
                    .panels()
                    .filter(|(p, _)| *p == self.id)
                    .map(|(_, x)| x.id.as_str())
                    .chain(b.modal(&self.id).map(|m| m.id.as_str()))
                    .collect();
                panic!(
                    "`{}` has no surface `{surface}` showing (panels: {})",
                    self.id,
                    list(&ids)
                );
            };
            if let Err(e) = crate::validate::check_elements(surface, &elements, element, &value) {
                panic!("{e}");
            }
        }
        let ev = UiEvent {
            surface: surface.into(),
            element: element.into(),
            value,
        };
        self.watch(&format!("ui({surface:?}, {element:?})"), |h| {
            ok(h.runtime.ui_event_with(&h.id, &ev, mods));
        });
    }

    /// Click the button on this plugin's notice.
    #[track_caller]
    pub fn click_notice(&mut self) {
        let has_button = self
            .backend()
            .notice(&self.id)
            .is_some_and(|n| n.button.is_some());
        assert!(
            has_button,
            "`{}` has no notice with a button showing",
            self.id
        );
        let ev = UiEvent {
            surface: model::SURFACE_NOTICE.into(),
            element: "button".into(),
            value: UiValue::Clicked,
        };
        self.watch("click_notice", |h| ok(h.runtime.ui_event(&h.id, &ev)));
    }

    /// Flip a toggle on one of this plugin's surfaces.
    #[track_caller]
    pub fn toggle(&mut self, panel: &str, element: &str, on: bool) {
        self.ui(panel, element, UiValue::Toggled(on));
    }

    /// Click a button on one of this plugin's surfaces.
    #[track_caller]
    pub fn click(&mut self, panel: &str, element: &str) {
        self.ui(panel, element, UiValue::Clicked);
    }

    /// Type into a text input (delivered as one change, as the client does on
    /// commit, not per keystroke).
    #[track_caller]
    pub fn type_text(&mut self, panel: &str, element: &str, text: &str) {
        self.ui(panel, element, UiValue::Text(text.into()));
    }

    /// Choose option `index` of a select.
    #[track_caller]
    pub fn select(&mut self, panel: &str, element: &str, index: u32) {
        self.ui(panel, element, UiValue::Selected(index));
    }

    /// Move a slider to `value`.
    #[track_caller]
    pub fn slide(&mut self, panel: &str, element: &str, value: f64) {
        self.ui(panel, element, UiValue::Number(value));
    }

    /// Pick a color in a color input.
    #[track_caller]
    pub fn pick_color(&mut self, panel: &str, element: &str, c: Color) {
        self.ui(panel, element, UiValue::Color(c));
    }

    /// Run one of this plugin's palette commands. Panics if it never
    /// registered `id`: the client only offers registered commands.
    #[track_caller]
    pub fn command(&mut self, id: &str) {
        let commands = self.commands();
        if !commands.iter().any(|c| c == id) {
            let ids: Vec<&str> = commands.iter().map(String::as_str).collect();
            panic!(
                "`{}` has no command `{id}` registered (it has: {})",
                self.id,
                list(&ids)
            );
        }
        self.watch(&format!("command({id:?})"), |h| {
            ok(h.runtime.run_command(&h.id, id));
        });
    }

    /// Press one of this plugin's keybinds.
    #[track_caller]
    pub fn keybind(&mut self, id: &str) {
        let known = self
            .backend()
            .keybinds()
            .iter()
            .any(|k| k.plugin == self.id && k.id == id);
        assert!(known, "`{}` has no keybind `{id}` registered", self.id);
        self.watch(&format!("keybind({id:?})"), |h| {
            ok(h.runtime.keybind(&h.id, id));
        });
    }

    /// This plugin's keybinds.
    pub fn keybinds(&self) -> Vec<model::Keybind> {
        self.backend()
            .keybinds()
            .iter()
            .filter(|k| k.plugin == self.id)
            .cloned()
            .collect()
    }

    /// This plugin's slash commands.
    pub fn slash_commands(&self) -> Vec<api::SlashCommand> {
        self.backend()
            .slash_commands()
            .filter(|(p, _)| *p == self.id)
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// Run `/command` with typed options in the selected channel.
    #[track_caller]
    pub fn slash(
        &mut self,
        command: &str,
        options: Vec<(&str, api::OptionValue)>,
    ) -> api::CommandReply {
        let known = self.slash_commands().iter().any(|c| c.name == command);
        assert!(known, "`{}` has no slash command `/{command}`", self.id);
        let channel = self.selected_channel();
        let inv = api::Invocation {
            command: command.into(),
            options: options
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            channel,
        };
        self.watch(&format!("slash(/{command})"), |h| {
            ok(h.runtime.slash_command(&h.id, &inv))
        })
    }

    /// Ask for autocomplete suggestions for `option` of `/command`, with
    /// `partial` typed so far: (label, value) pairs.
    #[track_caller]
    pub fn autocomplete(
        &mut self,
        command: &str,
        option: &str,
        partial: &str,
    ) -> Vec<(String, String)> {
        self.watch(&format!("autocomplete(/{command} {option})"), |h| {
            ok(h.runtime.autocomplete(&h.id, command, option, partial))
        })
    }

    /// Simulate a click on a message (area, click count, modifiers). Returns
    /// the plugin that handled it, if any.
    #[track_caller]
    pub fn message_click(
        &mut self,
        area: ClickArea,
        clicks: u8,
        mods: Mods,
        message: api::MessageRef,
    ) -> Option<String> {
        self.watch("message_click", |h| {
            h.runtime
                .message_click(ClickGesture { area, clicks, mods }, &message)
        })
    }

    /// Click one of this plugin's clickable decorations.
    #[track_caller]
    pub fn click_decoration(&mut self, kind: &str, key: u64) {
        let clickable = self
            .backend()
            .decoration(&self.id, kind, key)
            .is_some_and(|d| d.clickable);
        assert!(
            clickable,
            "`{}` has no clickable `{kind}` decoration on {key}",
            self.id
        );
        let ev = api::SlotEvent::DecorationClicked((kind.to_string(), key));
        self.watch("click_decoration", |h| ok(h.runtime.slot_event(&h.id, &ev)));
    }

    /// This plugin's decoration of `kind` on `key`.
    pub fn decoration(&self, kind: &str, key: u64) -> Option<model::Decoration> {
        self.backend().decoration(&self.id, kind, key).cloned()
    }

    /// This plugin's decorations of a kind, by key.
    pub fn decorations(&self, kind: &str) -> Vec<(u64, model::Decoration)> {
        self.backend()
            .decorations(kind)
            .into_iter()
            .filter(|(p, _, _)| *p == self.id)
            .map(|(_, k, d)| (k, d.clone()))
            .collect()
    }

    /// This plugin's accessory on `message`.
    pub fn accessory(&self, message: u64) -> Option<Vec<model::Element>> {
        self.backend()
            .accessory(&self.id, message)
            .map(<[_]>::to_vec)
    }

    /// This plugin's ghost messages in `channel`.
    pub fn ghosts(&self, channel: u64) -> Vec<model::GhostMessage> {
        self.backend()
            .ghosts(channel)
            .into_iter()
            .filter(|(p, _)| *p == self.id)
            .map(|(_, g)| g.clone())
            .collect()
    }

    /// This plugin's notice, if one is showing.
    pub fn notice(&self) -> Option<model::Notice> {
        self.backend().notice(&self.id).cloned()
    }

    /// This plugin's sections in the DM list.
    pub fn dm_sections(&self) -> Vec<model::DmSection> {
        self.backend().dm_sections(&self.id).to_vec()
    }

    /// This plugin's voice-panel status `id`: its text, and the time it
    /// counts from (`since-ms`), if any.
    pub fn voice_status(&self, id: &str) -> Option<(String, Option<u64>)> {
        self.backend()
            .voice_status(&self.id, id)
            .map(|(t, s)| (t.to_string(), s))
    }

    /// Actions this plugin performed as the user, in order.
    pub fn performed(&self) -> Vec<Action> {
        self.backend()
            .performed
            .iter()
            .filter(|(p, _)| *p == self.id)
            .map(|(_, a)| a.clone())
            .collect()
    }

    /// Desktop notifications this plugin showed, in order.
    pub fn notifications(&self) -> Vec<model::DesktopNotification> {
        self.backend()
            .notifications
            .iter()
            .filter(|n| n.plugin == self.id)
            .cloned()
            .collect()
    }

    // ---- events against the mock world ----

    /// Deliver a client event (to plugins granted the capability it needs),
    /// after running it past the plugins' `event-filter`s. Returns what the
    /// filters left: `None` if one dropped it.
    #[track_caller]
    pub fn event(&mut self, ev: api::Event) -> Option<api::Event> {
        let ev = self.intercept(ev)?;
        self.watch("event", |h| h.runtime.dispatch_event(&ev));
        Some(ev)
    }

    /// Deliver a batch of events as one host tick, each run past the
    /// `event-filter`s first.
    #[track_caller]
    pub fn events(&mut self, evs: Vec<api::Event>) {
        let evs: Vec<api::Event> = evs.into_iter().filter_map(|e| self.intercept(e)).collect();
        self.watch("events", |h| h.runtime.dispatch_events(&evs));
    }

    /// Run an event past the plugins' `event-filter`s only: what the client
    /// would process (`None`: dropped).
    #[track_caller]
    pub fn intercept(&mut self, ev: api::Event) -> Option<api::Event> {
        self.watch("event-filter", |h| h.runtime.intercept(ev))
    }

    /// The client makes a REST request. It runs past the plugins'
    /// `http-filter`s, the mock answers it
    /// ([`MockBackend::rest`](crate::backend::MockBackend::rest)), and the
    /// response runs past them on the way back. Returns the status and the
    /// body the client reads, or `None` when a filter dropped the request.
    #[track_caller]
    pub fn rest(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Option<(u16, Option<String>)> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let req = crate::runtime::RestRequest {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            method: method.to_string(),
            path: path.to_string(),
            body: body.map(str::to_string),
        };
        let body = match self.watch("http-filter", |h| h.runtime.filter_request(&req)) {
            crate::runtime::RequestOutcome::Drop => return None,
            crate::runtime::RequestOutcome::Send(body) => body,
        };
        let (status, answer) = self.backend().rest(method, path, body.as_deref());
        let answer = self.watch("http-filter", |h| {
            h.runtime.filter_response(&req, status, answer)
        });
        Some((status, answer))
    }

    /// A message arrives in `channel` from `author`: added to the world (and
    /// the screen) and delivered as `message-created`. Returns it.
    #[track_caller]
    pub fn receive_message(&mut self, channel: u64, author: u64, content: &str) -> api::Message {
        let m = {
            let b = self.backend();
            let name = b.name_of(author);
            let guild = b
                .world
                .channels
                .iter()
                .find(|c| c.id == channel)
                .and_then(|c| c.guild);
            let last = b
                .world
                .messages
                .get(&channel)
                .and_then(|v| v.last())
                .map_or(1 << 22, |m| m.id);
            let mut m =
                crate::backend::sample_message(channel, guild, last + 1, (author, &name), content);
            m.bot = b.world.users.get(&author).is_some_and(|u| u.bot);
            m
        };
        // The plugins' `event-filter`s see it before the client keeps it: a
        // dropped message never reaches the world.
        let Some(api::Event::MessageCreated(m)) =
            self.intercept(api::Event::MessageCreated(m.clone()))
        else {
            return m;
        };
        self.backend().world.push_message(m.clone());
        self.watch("event", |h| {
            h.runtime
                .dispatch_event(&api::Event::MessageCreated(m.clone()));
        });
        m
    }

    /// The author edits a loaded message: the world changes and
    /// `message-edited` carries the previous content.
    #[track_caller]
    pub fn edit_message(&mut self, channel: u64, id: u64, content: &str) -> api::Message {
        let (m, before) = {
            let mut b = self.backend();
            let m = b
                .world
                .find_message_mut(channel, id)
                .unwrap_or_else(|| panic!("no loaded message {id}"));
            let before = std::mem::replace(&mut m.content, content.to_string());
            m.edited_ms = Some(m.edited_ms.unwrap_or(0) + 1);
            (m.clone(), before)
        };
        self.event(api::Event::MessageEdited((m.clone(), Some(before))));
        m
    }

    /// A loaded message is deleted: it leaves the world and
    /// `message-deleted` carries the client's cached copy.
    #[track_caller]
    pub fn delete_message(&mut self, channel: u64, id: u64) {
        let cached = {
            let mut b = self.backend();
            let list = b.world.messages.entry(channel).or_default();
            let pos = list.iter().position(|m| m.id == id);
            let cached = pos.map(|p| list.remove(p));
            b.world.visible.retain(|r| r.id != id);
            cached
        };
        let guild = cached.as_ref().and_then(|m| m.guild);
        self.event(api::Event::MessageDeleted(api::DeletedMessage {
            channel,
            id,
            guild,
            cached,
        }));
    }

    /// Users typing in a channel (a snapshot, like the client's).
    #[track_caller]
    pub fn typing(&mut self, channel: u64, users: Vec<u64>) {
        self.backend().world.typing.insert(channel, users.clone());
        self.event(api::Event::Typing((channel, users)));
    }

    /// A presence update.
    #[track_caller]
    pub fn presence(&mut self, p: api::Presence) {
        self.backend().world.presences.insert(p.user, p.clone());
        self.event(api::Event::Presence(p));
    }

    /// The user switches channel.
    #[track_caller]
    pub fn select_channel(&mut self, channel: u64) {
        let ev = {
            let mut b = self.backend();
            let guild = b
                .world
                .channels
                .iter()
                .find(|c| c.id == channel)
                .and_then(|c| c.guild);
            let previous = b.world.selection.channel;
            b.world.selection.channel = Some(channel);
            b.world.selection.guild = guild;
            api::Event::SelectionChanged(api::SelectionChange {
                guild,
                channel: Some(channel),
                previous_channel: previous,
                message: None,
            })
        };
        self.event(ev);
    }

    // ---- rendering ----

    /// What this plugin's renderer does to loaded messages.
    #[track_caller]
    pub fn render(&mut self, channel: u64, ids: &[u64]) -> Vec<Vec<api::Transform>> {
        let inputs: Vec<api::RenderInput> = {
            use crate::backend::ClientBackend;
            let b = self.backend();
            ids.iter()
                .map(|id| {
                    let m = b
                        .world
                        .find_message(channel, *id)
                        .unwrap_or_else(|| panic!("no loaded message {id}"))
                        .clone();
                    let roles = m
                        .guild
                        .and_then(|g| b.member(g, m.author.id))
                        .map(|mb| mb.roles)
                        .unwrap_or_default();
                    api::RenderInput {
                        message: m,
                        author_roles: roles,
                        grouped: false,
                        dark: true,
                    }
                })
                .collect()
        };
        self.watch("render", |h| h.runtime.render(&inputs))
            .into_iter()
            .map(|per| {
                per.into_iter()
                    .filter(|r| r.plugin == self.id)
                    .flat_map(|r| r.transforms)
                    .collect()
            })
            .collect()
    }

    /// [`Self::render`] for one message.
    #[track_caller]
    pub fn render_message(&mut self, channel: u64, id: u64) -> Vec<api::Transform> {
        self.render(channel, &[id]).pop().unwrap_or_default()
    }

    /// Run a client notification through the plugins' notification filters.
    #[track_caller]
    pub fn filter_notification(
        &mut self,
        n: &api::ClientNotification,
    ) -> api::NotificationDecision {
        self.watch("filter_notification", |h| {
            h.runtime.filter_notification(n).0
        })
    }

    // ---- settings ----

    /// The current value of one of the plugin's schema settings.
    pub fn setting(&self, key: &str) -> Option<SettingValue> {
        self.settings_page()?
            .entries
            .into_iter()
            .find(|e| e.spec.key == key)
            .map(|e| e.value)
    }

    /// The plugin's generated settings page, if it declares settings.
    pub fn settings_page(&self) -> Option<SettingsPage> {
        self.runtime.settings_page(&self.id)
    }

    /// The user changes a setting on the generated page.
    #[track_caller]
    pub fn set_setting(&mut self, key: &str, value: SettingValue) {
        self.watch(&format!("set_setting({key:?})"), |h| {
            ok(h.runtime.set_setting(&h.id, key, value));
        });
    }

    /// Log in as another account (per-account storage swaps).
    #[track_caller]
    pub fn switch_account(&mut self, account: u64) {
        self.watch("switch_account", |h| {
            ok(h.runtime.set_account(Some(account)));
        });
    }

    /// Run `f`, which should make the plugin fault (a panic, a throw, running
    /// out of fuel), and return why it faulted. Panics if it didn't. Outside
    /// this, any interaction that faults the plugin fails the test at once.
    #[track_caller]
    pub fn expect_fault(&mut self, f: impl FnOnce(&mut Self)) -> String {
        self.assert_active();
        self.fault_expected = true;
        f(self);
        self.fault_expected = false;
        match self.info().status {
            Status::Faulted(why) => why,
            Status::Active => panic!(
                "expected `{}` to fault, but it's still active\nlogs:\n{}",
                self.id,
                self.log_text()
            ),
        }
    }

    /// Run `f` and return what it returns; if it took the plugin from active
    /// to faulted (and that wasn't expected), fail with the reason and the
    /// plugin's log, which is where a panic message or a script stack trace
    /// is.
    #[track_caller]
    fn watch<R>(&mut self, what: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        let was_active = matches!(self.info().status, Status::Active);
        let out = f(self);
        if was_active
            && !self.fault_expected
            && let Status::Faulted(why) = self.info().status
        {
            panic!(
                "`{}` faulted during {what}: {why}\n--- plugin log ---\n{}",
                self.id,
                self.log_text()
            );
        }
        out
    }

    /// Deactivate and re-activate (storage persists), as on a client restart.
    pub fn restart(&mut self) -> Result<()> {
        self.runtime.unload(&self.id)?;
        self.runtime.load(&self.wasm, Trust::FirstParty)?;
        Ok(())
    }

    /// Deactivate and unload.
    pub fn stop(&mut self) -> Result<()> {
        self.runtime.unload(&self.id)
    }
}

/// The value of a runtime call the harness only makes when it should
/// succeed. Panics with the error otherwise.
#[track_caller]
fn ok<T>(r: Result<T>) -> T {
    r.unwrap_or_else(|e| panic!("{e:#}"))
}

fn list(ids: &[&str]) -> String {
    if ids.is_empty() {
        "none".into()
    } else {
        ids.iter()
            .map(|i| format!("`{i}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}
