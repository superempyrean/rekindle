//! [`Rekindle`]: the plugin [`Runtime`] and rekindle's own work around it.
//!
//! That is loading installed and dev plugins, first-party updates and the
//! blocklist (checked in the background, applied in [`Rekindle::poll`]),
//! rekindle's own settings panel ([`rekindle_plugin_host::manager`]: turning
//! plugins on and off, installing from a file or the store, reviews), the
//! network guard and fault toasts.
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use anyhow::{Context as _, Result, bail};
use rekindle_plugin_host::api::{Target, UserRef};
use rekindle_plugin_host::home::{Home, InstallState};
use rekindle_plugin_host::manager::{self, DevPlugin, Effect, Manager, Synced};
use rekindle_plugin_host::model::{LogLevel, LogRecord, Mods, ToastKind, UiEvent};
use rekindle_plugin_host::net::{
    HttpError, HttpRequest, HttpResponse, NetClient, NetLimits, UreqClient,
};
use rekindle_plugin_host::runtime::{LoadTimes, PluginInfo, Status, StorageLocation};
use rekindle_plugin_host::store::Download;
use rekindle_plugin_host::updates::{self, Outcome, Updater};
use rekindle_plugin_host::{ClientBackend as _, Runtime, SharedBackend, Trust};

use crate::ExtArgs;
use crate::backend::DetourBackend;
use crate::picker::{Picked, Picker};

/// Where a plugin came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// The user's home.
    Installed,
    /// A file the host named, loaded as first-party.
    Dev,
}

/// A plugin this session knows of, loaded or not.
struct Source {
    origin: Origin,
    path: PathBuf,
    /// Known once the plugin has loaded.
    id: Option<String>,
    /// Why it didn't load.
    error: Option<String>,
    /// False once the user turned it off for the session.
    enabled: bool,
}

impl Source {
    fn new(origin: Origin, path: PathBuf, id: Option<String>, error: Option<String>) -> Self {
        Self {
            origin,
            path,
            id,
            error,
            enabled: true,
        }
    }
}

/// The plugin runtime in the client, with rekindle's settings panel and
/// updates.
pub struct Rekindle {
    runtime: Runtime,
    backend: Arc<Mutex<DetourBackend>>,
    home: Home,
    sources: Vec<Source>,
    last_status: HashMap<String, Status>,
    updater: Option<Updater>,
    manager: Manager,
    picker: Option<Picker>,
    /// A download running off the UI thread: (plugin id, result).
    download: Option<(String, mpsc::Receiver<Result<Download>>)>,
    /// Third-party updates already announced in a toast: (id, version).
    announced: BTreeSet<(String, String)>,
    /// The user pressed "Check for updates now".
    check_asked: bool,
    /// Plugins to start once the client is up ([`Rekindle::start_when_ready`]):
    /// whether to load the installed ones, and the dev paths.
    waiting: Option<(bool, Vec<PathBuf>)>,
}

impl Rekindle {
    /// Start the runtime on `backend` and load the plugins `opts` names.
    /// Fails if the home or the storage directory can't be opened or the
    /// runtime can't start. A plugin that doesn't load is reported, not an
    /// error.
    pub(crate) fn new(backend: DetourBackend, opts: ExtArgs) -> Result<Self> {
        let home = match &opts.home {
            Some(root) => Home::new(root),
            None => Home::from_env()?,
        };
        let backend = Arc::new(Mutex::new(backend));
        let shared: SharedBackend = backend.clone();
        let mut config = home.runtime_config();
        config.storage = if opts.memory_storage {
            StorageLocation::Memory
        } else {
            let dir = home.storage_dir();
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating storage dir {}", dir.display()))?;
            StorageLocation::Dir(dir)
        };
        config.net = if opts.allow_network {
            Arc::new(GuardedNet(UreqClient))
        } else {
            Arc::new(OfflineNet)
        };
        let runtime = Runtime::new(shared, config)?;

        // rekindle's own panel goes first on the settings page. A session
        // that doesn't use the installed plugins (the self-test) gets an
        // empty home of its own, so nothing it does touches the user's.
        let manager_home = if opts.load_installed {
            home.clone()
        } else {
            Home::new(std::env::temp_dir().join(format!("rekindle-session-{}", std::process::id())))
        };
        let manager = Manager::new(manager_home);
        backend
            .lock()
            .unwrap()
            .show_panel(manager::OWNER, manager.panel(&[]));

        // First-party updates and the blocklist, and third-party updates
        // for review (developer mode), checked off this thread. With no
        // release channel and developer mode off, nothing is contacted.
        let updater = opts
            .load_installed
            .then(|| {
                let location = home.channel_location().unwrap_or_else(|e| {
                    say!("rekindle: release channel: {e:#}");
                    None
                });
                Updater::start(home.clone(), location, updates::CHECK_INTERVAL)
                    .inspect_err(|e| say!("rekindle: updates won't be checked: {e}"))
                    .ok()
            })
            .flatten();
        let mut p = Rekindle {
            runtime,
            backend,
            home,
            sources: Vec::new(),
            last_status: HashMap::new(),
            updater,
            manager,
            picker: None,
            download: None,
            announced: BTreeSet::new(),
            check_asked: false,
            waiting: Some((opts.load_installed, opts.dev_paths)),
        };
        if !opts.wait_for_client {
            p.start_plugins();
        }
        Ok(p)
    }

    /// Start the plugins once the client is up, so their `activate` can
    /// query it, as they would on a client that loaded them at start-up.
    /// True once they're started.
    pub(crate) fn start_when_ready(&mut self) -> bool {
        if self.waiting.is_some() && rekindle_ui_hook::client::ready() {
            self.start_plugins();
        }
        self.waiting.is_none()
    }

    /// Load the plugins `new` was given.
    fn start_plugins(&mut self) {
        let Some((load_installed, dev_paths)) = self.waiting.take() else {
            return;
        };
        if load_installed {
            self.sources = installed_sources(&self.home, &mut self.runtime);
        }
        if !dev_paths.is_empty() {
            say!(
                "rekindle: the plugins the host named run as first-party, with everything their manifests request"
            );
        }
        let first_dev = self.sources.len();
        self.sources.extend(
            dev_paths
                .into_iter()
                .map(|path| Source::new(Origin::Dev, path, None, None)),
        );
        let p = self;
        for i in first_dev..p.sources.len() {
            p.load_dev(i);
        }
        p.check_faults();
        p.show_manager();
        // Once per session: some plugins' features have nowhere to go on
        // this client build. Their pages on rekindle's settings page say which.
        if p.runtime
            .plugins()
            .iter()
            .any(|i| !i.missing_slots.is_empty())
        {
            let text = "Some plugin features aren't available on this version of Discord. rekindle's Plugins page says which.";
            p.backend
                .lock()
                .unwrap()
                .show_toast(manager::OWNER, text, ToastKind::Info);
        }
    }

    fn load_dev(&mut self, idx: usize) {
        let path = self.sources[idx].path.clone();
        let result = std::fs::read(&path)
            .with_context(|| format!("reading {}", path.display()))
            .and_then(|bytes| self.runtime.reload(&bytes, Trust::FirstParty));
        let src = &mut self.sources[idx];
        match result {
            Ok(id) => {
                let verb = if src.id.is_some() {
                    "reloaded"
                } else {
                    "loaded"
                };
                say!(
                    "rekindle: {verb} `{id}` from {} (your own plugin, first-party)",
                    path.display()
                );
                src.id = Some(id.clone());
                src.error = None;
                self.host_log(
                    &id,
                    LogLevel::Info,
                    &format!("{verb} from {}", path.display()),
                );
            }
            Err(e) => {
                let msg = format!("{e:#}");
                say!("rekindle: failed to load {}: {msg}", path.display());
                let who = src.id.clone().unwrap_or_else(|| path.display().to_string());
                src.error = Some(msg.clone());
                self.host_log(&who, LogLevel::Error, &format!("load failed: {msg}"));
            }
        }
    }

    fn load_installed(&mut self, idx: usize) {
        let Some(id) = self.sources[idx].id.clone() else {
            return;
        };
        let result = self.try_load_installed(&id);
        self.sources[idx].error = result.as_ref().err().map(|e| format!("{e:#}"));
        match result {
            Ok(()) => self.host_log(&id, LogLevel::Info, "loaded (installed)"),
            Err(e) => self.host_log(&id, LogLevel::Error, &format!("not loaded: {e:#}")),
        }
    }

    fn try_load_installed(&mut self, id: &str) -> Result<()> {
        let inst = self.home.get(id)?.context("no longer installed")?;
        if inst.state != InstallState::Ready {
            bail!("{}", inst.state.describe());
        }
        let (bytes, trust) = inst.read_verified()?;
        self.runtime.load(&bytes, trust)?;
        Ok(())
    }

    /// Turn a plugin on or off for this session.
    fn set_enabled(&mut self, idx: usize, on: bool) -> Result<()> {
        let src = self.sources.get_mut(idx).context("no such plugin")?;
        src.enabled = on;
        let id = src.id.clone();
        if on {
            match src.origin {
                Origin::Dev => self.load_dev(idx),
                Origin::Installed => {
                    if let Some(id) = &id
                        && self.runtime.plugin(id).is_some()
                    {
                        self.runtime.unload(id)?;
                    }
                    self.load_installed(idx);
                }
            }
        } else if let Some(id) = id {
            if self.runtime.plugin(&id).is_some() {
                self.runtime.unload(&id)?;
            }
            self.host_log(&id, LogLevel::Info, "disabled for this session");
        }
        self.check_faults();
        self.show_manager();
        Ok(())
    }

    /// Take the work that finished off this thread: the file picker's
    /// choice, a store download, an update check.
    pub fn poll(&mut self) {
        self.take_pick();
        self.take_download();
        self.apply_updates();
        self.check_faults();
        if self.manager.console_open() {
            self.show_manager();
        }
    }
    // ---- rekindle's own settings panel ----

    fn show_manager(&mut self) {
        let dev: Vec<DevPlugin> = self
            .sources
            .iter()
            .filter(|s| s.origin == Origin::Dev)
            .filter_map(|s| {
                let id = s.id.clone()?;
                let name = self
                    .runtime
                    .plugin(&id)
                    .map_or_else(|| id.clone(), |i| i.name);
                Some(DevPlugin {
                    id,
                    name,
                    path: s.path.display().to_string(),
                    on: s.enabled,
                })
            })
            .collect();
        let missing = self
            .runtime
            .plugins()
            .into_iter()
            .map(|p| (p.id, p.missing_slots))
            .collect();
        self.manager.set_missing(missing);
        if self.manager.console_open() {
            let logs = self.runtime.logs().snapshot();
            let from = logs.len().saturating_sub(300);
            self.manager.set_logs(logs[from..].to_vec());
        }
        let panel = self.manager.panel(&dev);
        // Only the plugin whose page is open has its panels drawn.
        rekindle_ui_hook::set_shown_plugin(self.manager.detail_of().map(String::from));
        self.backend
            .lock()
            .unwrap()
            .show_panel(manager::OWNER, panel);
    }

    /// The user used rekindle's panel.
    fn manager_event(&mut self, ev: &UiEvent) {
        let effects = self.manager.handle(ev);
        self.run_effects(effects);
        self.show_manager();
    }

    fn run_effects(&mut self, effects: Vec<Effect>) {
        for e in effects {
            match e {
                Effect::Sync { ids, reload } => {
                    for id in ids {
                        let r = manager::sync(self.manager.home(), &mut self.runtime, &id, reload);
                        self.synced(&id, &r);
                    }
                }
                Effect::SyncAll => {
                    let loaded: Vec<String> = self
                        .sources
                        .iter()
                        .filter(|s| s.origin == Origin::Installed)
                        .filter_map(|s| s.id.clone())
                        .collect();
                    let home = self.manager.home();
                    for (id, r) in manager::sync_all(home, &mut self.runtime, &loaded) {
                        self.synced(&id, &r);
                    }
                }
                Effect::PickFile if self.picker.is_none() => match Picker::open() {
                    Ok(p) => self.picker = Some(p),
                    Err(e) => self
                        .manager
                        .set_notice(format!("Couldn't open the file dialog: {e}")),
                },
                Effect::PickFile => {}
                Effect::Dev { id, on } => {
                    let idx = self.sources.iter().position(|s| {
                        s.origin == Origin::Dev && s.id.as_deref() == Some(id.as_str())
                    });
                    if let Some(idx) = idx
                        && let Err(e) = self.set_enabled(idx, on)
                    {
                        self.manager.set_notice(format!("{e:#}"));
                    }
                }
                Effect::Download { id } => self.start_download(id),
                Effect::Reload { id } => {
                    let dev = self.sources.iter().position(|s| {
                        s.origin == Origin::Dev && s.id.as_deref() == Some(id.as_str())
                    });
                    if let Some(idx) = dev {
                        self.load_dev(idx);
                    } else {
                        let r = manager::sync(self.manager.home(), &mut self.runtime, &id, true);
                        self.synced(&id, &r);
                    }
                }
                Effect::Eval { command } => {
                    let result = manager::console::run(&mut self.runtime, &command);
                    self.manager.set_console_result(result);
                }
                Effect::CheckUpdates => match &self.updater {
                    Some(u) => {
                        u.check_now();
                        self.check_asked = true;
                    }
                    None => self.manager.set_notice(
                        "This session isn't using your installed plugins, so there is nothing to update.",
                    ),
                },
            }
        }
        self.check_faults();
    }

    /// Download a plugin from the store on a thread of its own, unless a
    /// download is running.
    fn start_download(&mut self, id: String) {
        if self.download.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let home = self.manager.home().clone();
        let for_thread = id.clone();
        std::thread::spawn(move || {
            // The receiver is gone only when rekindle is, and then nothing
            // waits for the download.
            let _ = tx.send(rekindle_plugin_host::store::download(&home, &for_thread));
        });
        self.download = Some((id, rx));
    }

    /// Record what [`manager::sync`] did, and keep the installed sources
    /// in step with the home.
    fn synced(&mut self, id: &str, result: &Result<Synced>) {
        let installed = self.manager.home().get(id).ok().flatten().is_some();
        let idx = self
            .sources
            .iter()
            .position(|s| s.origin == Origin::Installed && s.id.as_deref() == Some(id));
        match (installed, idx) {
            (true, None) => self.sources.push(Source::new(
                Origin::Installed,
                self.manager.home().plugin_dir(id),
                Some(id.to_string()),
                None,
            )),
            (false, Some(i)) => {
                self.sources.remove(i);
            }
            _ => {}
        }
        let error = result.as_ref().err().map(|e| format!("{e:#}"));
        if let Some(src) = self
            .sources
            .iter_mut()
            .find(|s| s.origin == Origin::Installed && s.id.as_deref() == Some(id))
        {
            src.error.clone_from(&error);
        }
        match result {
            Ok(Synced::Started) => self.host_log(id, LogLevel::Info, "turned on"),
            Ok(Synced::Restarted) => {
                self.host_log(id, LogLevel::Info, "installed and restarted");
            }
            Ok(Synced::Stopped) => self.host_log(id, LogLevel::Info, "turned off"),
            Ok(Synced::Unchanged) => {}
            Err(_) => {
                let why = error.unwrap_or_default();
                self.host_log(id, LogLevel::Error, &format!("didn't start: {why}"));
                self.manager.set_notice(format!("{id} didn't start: {why}"));
            }
        }
    }

    /// A store download finished: show its review screen.
    fn take_download(&mut self) {
        let Some((id, rx)) = &self.download else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!("the download stopped")),
        };
        let id = id.clone();
        self.download = None;
        let effects = self.manager.downloaded(&id, result);
        self.run_effects(effects);
        self.show_manager();
    }

    /// The file picker closed: review or install the choice.
    fn take_pick(&mut self) {
        let Some(choice) = self.picker.as_ref().and_then(Picker::try_take) else {
            return;
        };
        self.picker = None;
        if let Picked::File(path) = choice {
            let effects = self.manager.open_file(&path);
            self.run_effects(effects);
        }
        self.show_manager();
    }

    /// Take a finished background check, if any: swap updated plugins in
    /// and stop blocked ones, and tell the user in a toast.
    fn apply_updates(&mut self) {
        let Some(report) = self.updater.as_ref().and_then(Updater::try_recv) else {
            return;
        };
        // Offline or no channel reachable: try again next interval.
        for e in &report.errors {
            say!("rekindle: checking for updates: {e}");
        }
        for u in &report.reviews {
            if self.announced.insert((u.id.clone(), u.to.clone())) {
                let text = format!(
                    "{} {} is ready for review in rekindle's settings",
                    u.name, u.to
                );
                self.host_log(&u.id, LogLevel::Info, &text);
                self.backend
                    .lock()
                    .unwrap()
                    .show_toast(manager::OWNER, &text, ToastKind::Info);
            }
        }
        self.manager.refresh_reviews();
        if std::mem::take(&mut self.check_asked) {
            self.manager.set_notice(if !report.errors.is_empty() {
                "Couldn't check for updates. rekindle will try again later."
            } else if report.updated.is_empty()
                && report.reviews.is_empty()
                && report.available.is_empty()
            {
                "Everything is up to date."
            } else {
                "Checked for updates."
            });
        }
        for (id, why) in &report.failed {
            say!("rekindle: could not update `{id}`: {why}");
        }
        let outcomes = updates::apply(&self.home, &mut self.runtime, &report);
        for o in &outcomes {
            let (id, text, kind) = match o {
                Outcome::Updated { id, to } => (id, format!("updated to {to}"), ToastKind::Success),
                Outcome::RolledBack { id, to, why } => {
                    say!("rekindle: `{id}` {to} didn't start, back on the previous version: {why}");
                    (
                        id,
                        format!("{to} didn't start, so the previous version is back"),
                        ToastKind::Warning,
                    )
                }
                Outcome::Stopped { id, why } => (
                    id,
                    format!("turned off by rekindle: {why}"),
                    ToastKind::Error,
                ),
            };
            let name = self
                .home
                .get(id)
                .ok()
                .flatten()
                .and_then(|i| i.manifest)
                .map_or_else(|| id.clone(), |m| m.name);
            self.host_log(id, LogLevel::Info, &text);
            self.backend
                .lock()
                .unwrap()
                .show_toast(id, &format!("{name} {text}"), kind);
        }
        // A test of plugin updates from inside the client waits for this.
        if let Ok(path) = std::env::var("REKINDLE_HOST_PLUGINS_UPDATED") {
            let updated: Vec<String> = report
                .updated
                .iter()
                .map(|u| format!("{} {}->{}", u.id, u.from, u.to))
                .collect();
            if let Err(e) = std::fs::write(
                &path,
                format!(
                    "updated={updated:?} failed={:?} errors={:?} outcomes={outcomes:?}\n",
                    report.failed, report.errors
                ),
            ) {
                say!("rekindle: writing {path}: {e}");
            }
        }
        self.show_manager();
    }
    /// Raise a client toast for every plugin that newly faulted. The runtime
    /// has already dropped its contributions via `plugin_unloaded`.
    fn check_faults(&mut self) {
        let mut faults = Vec::new();
        for info in self.runtime.plugins() {
            let prev = self
                .last_status
                .insert(info.id.clone(), info.status.clone());
            if let Status::Faulted(why) = &info.status
                && prev.as_ref() != Some(&info.status)
            {
                say!("rekindle: plugin `{}` faulted: {why}", info.id);
                faults.push((info.id.clone(), info.name.clone(), why.clone()));
            }
        }
        for (id, name, why) in faults {
            self.backend.lock().unwrap().show_toast(
                &id,
                &format!("{name} stopped: {why}"),
                ToastKind::Error,
            );
        }
    }

    fn host_log(&self, plugin: &str, level: LogLevel, message: &str) {
        self.runtime.logs().push(LogRecord {
            plugin: plugin.to_string(),
            level,
            message: format!("[rekindle] {message}"),
            at_ms: 0,
        });
    }

    // ---- calls into plugins ----

    /// Fire due plugin timers.
    pub(crate) fn tick_timers(&mut self) {
        self.runtime.fire_due_timers();
        self.check_faults();
    }

    /// Deliver the cursor (client-relative logical pixels) to `pointer` plugins.
    pub(crate) fn pointer_moved(&mut self, x: f32, y: f32) {
        self.runtime.dispatch_pointer(x, y);
        self.check_faults();
    }

    /// Run the user's outgoing message through `compose` plugins.
    pub(crate) fn compose(
        &mut self,
        channel: u64,
        text: &str,
    ) -> rekindle_plugin_host::model::Composed {
        let out = self.runtime.compose_text(channel, text);
        for who in &out.rewritten_by {
            say!("rekindle: `{who}` rewrote a message you sent");
        }
        self.check_faults();
        out
    }

    pub(crate) fn run_command(&mut self, plugin: &str, id: &str) {
        if let Err(e) = self.runtime.run_command(plugin, id) {
            self.host_log(
                plugin,
                LogLevel::Warn,
                &format!("command `{id}` failed: {e:#}"),
            );
        }
        self.check_faults();
    }

    pub(crate) fn slot_click(&mut self, plugin: &str, slot: &str, item: &str, target: Target) {
        if let Err(e) = self
            .runtime
            .slot_click(plugin, slot, item, target, Mods::default())
        {
            self.host_log(
                plugin,
                LogLevel::Warn,
                &format!("slot click `{item}` dropped: {e:#}"),
            );
        }
        self.check_faults();
    }

    pub(crate) fn open_profile(&mut self, user: &UserRef) {
        self.runtime.open_profile(user);
        self.check_faults();
    }

    pub(crate) fn ui_event(&mut self, plugin: &str, ev: &UiEvent) {
        if plugin == manager::OWNER {
            self.manager_event(ev);
            return;
        }
        if let Err(e) = self.runtime.ui_event(plugin, ev) {
            self.host_log(plugin, LogLevel::Warn, &format!("ui event dropped: {e:#}"));
        }
        self.check_faults();
    }

    /// The user dismissed a plugin's modal: drop it from the client backend.
    pub(crate) fn close_modal(&mut self, plugin: &str) {
        self.backend.lock().unwrap().close_modal(plugin);
    }

    /// The plugins loaded now.
    pub fn infos(&self) -> Vec<PluginInfo> {
        self.runtime.plugins()
    }

    /// Where the last plugin load spent its time.
    pub fn last_load(&self) -> LoadTimes {
        self.runtime.last_load
    }

    /// The plugins' log records after cursor `since`, and the cursor to pass
    /// next time.
    pub fn logs_since(&self, since: u64) -> (Vec<LogRecord>, u64) {
        self.runtime.logs().since(since)
    }

    /// The plugins' renderers over `batch` (`Runtime::render`).
    pub(crate) fn render(
        &mut self,
        batch: &[rekindle_plugin_host::api::RenderInput],
    ) -> Vec<Vec<rekindle_plugin_host::runtime::Rendered>> {
        self.runtime.render(batch)
    }

    /// A loaded plugin's name, or its id.
    pub(crate) fn plugin_name(&self, id: &str) -> String {
        self.runtime
            .plugin(id)
            .map_or_else(|| id.to_string(), |p| p.name.clone())
    }

    /// The colour theme token `token` has now, as `0xRRGGBBAA`.
    pub(crate) fn token_rgba(&self, token: &str) -> Option<u32> {
        use rekindle_plugin_host::backend::ClientBackend;
        match self.backend.lock().unwrap().get_value(token)? {
            rekindle_plugin_host::model::ThemeValue::Color(c) => {
                Some(u32::from_be_bytes([c.r, c.g, c.b, c.a]))
            }
            _ => None,
        }
    }

    /// The plugin `id`, if it is loaded.
    pub fn info(&self, id: &str) -> Option<PluginInfo> {
        self.runtime.plugin(id)
    }

    /// The top of rekindle's panel as widgets, for a client that shows the
    /// panel a page at a time (`Manager::menu`). Its controls are the
    /// panel's, so a press on one goes to the panel like any other.
    pub fn menu(&self) -> Vec<rekindle_ui_hook::Widget> {
        crate::convert::widgets_of(&self.manager.menu().elements)
    }

    /// The plugin whose page rekindle's panel shows, if one is.
    pub fn detail_of(&self) -> Option<String> {
        self.manager.detail_of().map(String::from)
    }
}

/// Load the user's installed plugins into `runtime`, as sources. A plugin
/// that doesn't load is a source with its error.
fn installed_sources(home: &Home, runtime: &mut Runtime) -> Vec<Source> {
    let reports = match home.load_into(runtime) {
        Ok(reports) => reports,
        Err(e) => {
            say!(
                "rekindle: could not read installed plugins from {}: {e:#}",
                home.root().display()
            );
            return Vec::new();
        }
    };
    let third_party = reports
        .iter()
        .filter(|r| r.result.is_ok() && r.trust == Some(Trust::ThirdParty))
        .count();
    if third_party > 0 {
        say!(
            "rekindle: developer mode is on: {third_party} plugin(s) from outside rekindle are running"
        );
    }
    reports
        .into_iter()
        .map(|r| {
            if let Err(e) = &r.result {
                say!("rekindle: installed plugin `{}` not loaded: {e}", r.id);
            }
            Source::new(
                Origin::Installed,
                home.plugin_dir(&r.id),
                Some(r.id),
                r.result.err(),
            )
        })
        .collect()
}

/// The network of a session without one: every request fails.
struct OfflineNet;

impl NetClient for OfflineNet {
    fn send(&self, req: &HttpRequest, _limits: &NetLimits) -> Result<HttpResponse, HttpError> {
        Err(HttpError::Transport(format!(
            "this session has no network (refused {} {})",
            req.method.as_str(),
            req.url
        )))
    }
}

/// HTTPS to the hosts a plugin's manifest lists (the runtime checks them),
/// except Discord's own.
struct GuardedNet(UreqClient);

const BLOCKED_SUFFIXES: &[&str] = &[
    "discord.com",
    "discordapp.com",
    "discord.gg",
    "discordapp.net",
    "discord.media",
    "discord.new",
];

impl NetClient for GuardedNet {
    fn send(&self, req: &HttpRequest, limits: &NetLimits) -> Result<HttpResponse, HttpError> {
        let host = url::Url::parse(&req.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .ok_or_else(|| HttpError::InvalidUrl(req.url.clone()))?;
        if BLOCKED_SUFFIXES
            .iter()
            .any(|s| host == *s || host.ends_with(&format!(".{s}")))
        {
            return Err(HttpError::HostNotAllowed(format!(
                "{host}: plugins can't reach Discord's own hosts"
            )));
        }
        self.0.send(req, limits)
    }
}
