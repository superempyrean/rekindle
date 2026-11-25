//! rekindle's plugin runtime inside the client, driven by recovered layout.
//!
//! This crate names no client type and doesn't depend on the client. The
//! host resolves a [`RecoveredProfile`] of addresses, offsets and sizes, and
//! [`Driver::start`] installs rekindle's detours from it: the client's theme
//! function ([`theme`]), its registries, its send path and gpui's frame
//! (through `rekindle-ui-hook`). [`Rekindle`] runs the plugins, rekindle's
//! own settings panel and updates.

/// `eprintln!` that ignores a failed write. Inside the client, stderr can
/// be a pipe nobody reads any more, and `eprintln!` panics on one.
macro_rules! say {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        // A failed write to stderr has nowhere to be reported.
        let _ = writeln!(std::io::stderr(), $($t)*);
    }};
}

mod backend;
mod convert;
mod overlay;
mod picker;
mod rekindle;
pub mod slash;
pub mod theme;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::api::{ChannelRef, MessageRef, Target, UserRef};
use rekindle_plugin_host::model::{
    Color, SLOT_CHANNEL_MENU, SLOT_CHAT_BAR_BUTTON, SLOT_MESSAGE_MENU, SLOT_USER_MENU, UiEvent,
    UiValue,
};
use rekindle_ui_hook::registry::RegistryLayout;
use rekindle_ui_hook::{
    Button, Click, Command, ComposeLayout, FrameworkLayout, Hook as UiHook, MenuTargetRef, UiInput,
};

pub use backend::PickerTab;
use backend::{DetourBackend, Shared};
use overlay::OverlayState;
pub use rekindle::Rekindle;
use theme::{ThemeHook, ThemeProfile, ThemeState};

/// The recovered layout rekindle needs to drive one client build.
pub struct RecoveredProfile {
    pub theme: ThemeProfile,
    /// The client's registries, when they're recovered: rekindle registers as
    /// a provider in each (switcher, chat bar, menus, profile popout, modals)
    /// and adds its settings page and overlay layer.
    pub registry: Option<RegistryLayout>,
    /// The send path, for `compose` plugins.
    pub compose: Option<ComposeLayout>,
    /// gpui's frame, for the framework layer: overlay, toasts, chat-bar
    /// buttons by shape and recolouring.
    pub framework: Option<FrameworkLayout>,
    /// The named surfaces plugins can restyle (`theming.surfaces()`), for a
    /// client whose surfaces the host reaches over its wire. The host reads
    /// the styles back with [`Driver::surface_styles`]. With none,
    /// `set-surface` fails with `unknown-surface`.
    pub surfaces: Vec<String>,
    /// The host runs slash commands typed in the client's composer, which
    /// it reads back with [`Driver::slash_commands`]. Without it,
    /// `register-slash-command` fails with `unavailable`.
    pub slash_commands: bool,
    /// The host draws the plugins' context-menu rows on the client's wire,
    /// and reads them back with [`Driver::menu_items`]. Without it or the
    /// client's registries, `slots.add` on a menu fails with
    /// `unavailable`.
    pub wire_menus: bool,
    /// The host puts the theme's tokens that aren't colours (lengths,
    /// fonts, density) on the client's wire, and reads them back with
    /// [`Driver::theme_values`]. Without it, setting one fails with
    /// `wrong-kind`.
    pub wire_values: bool,
    /// The host draws the plugins' tabs in the client's emoji picker on its
    /// wire, and reads them back with [`Driver::picker_tabs`]. Without it,
    /// adding to `expression-picker.tab` fails with `unavailable`.
    pub wire_picker_tabs: bool,
    /// The client's elements the host reaches on its wire, for the
    /// `elements` interface: each name with its selector as JSON. The host
    /// reads what the plugins did to them, and the `wire-raw` plugins' own
    /// work, back with [`Driver::named_elements`]. With none, every call
    /// to either fails with `unavailable`.
    pub elements: std::collections::BTreeMap<String, String>,
}

/// How long a send waits for `compose` plugins before going out unchanged.
pub const COMPOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// What the host runs: which plugins, and with what storage and network.
#[derive(Debug)]
pub struct ExtArgs {
    /// Plugin files loaded as first-party.
    pub dev_paths: Vec<PathBuf>,
    /// Load the user's installed plugins from the home and check for updates.
    pub load_installed: bool,
    /// Keep plugin storage in memory instead of the home.
    pub memory_storage: bool,
    /// Let plugins reach the hosts their manifests list.
    pub allow_network: bool,
    /// Start the plugins once the client has handed rekindle its model
    /// (`rekindle_ui_hook::client::ready`), instead of at once.
    pub wait_for_client: bool,
    /// The plugin home, when it isn't `$REKINDLE_HOME` or `~/.rekindle`. An
    /// app on Android has no `HOME` and keeps it in its own files.
    pub home: Option<PathBuf>,
}

/// `elements` as the rows of a panel show them: what the host draws for a
/// plugin's insert next to one of the client's elements.
pub fn widgets_of(elements: &[rekindle_plugin_host::model::Element]) -> Vec<rekindle_ui_hook::Widget> {
    convert::widgets_of(elements)
}

/// The routing key of a plugin's item. `\u{1f}` can't appear in a plugin or
/// item id, so it separates the two.
pub(crate) fn key(plugin: &str, id: &str) -> String {
    format!("{plugin}\u{1f}{id}")
}

/// A client theme's name and its token values (HSLA).
type NamedPalette = (String, Vec<(String, [f32; 4])>);

/// rekindle attached to a client: the plugin runtime and the detours that
/// put its plugins' contributions on screen. Dropping it removes the
/// detours.
pub struct Driver {
    rekindle: Rekindle,
    shared: Shared,
    theme_hook: Option<ThemeHook>,
    ui_registry: Option<UiHook>,
    _ui_compose: Option<UiHook>,
    _ui_framework: Option<UiHook>,
    /// Chat-bar buttons go through the framework layer (see
    /// [`Driver::use_framework_chat_bar`]).
    framework_buttons: bool,
    /// Chat-bar buttons go on the client's wire (see
    /// [`Driver::use_wire_chat_bar`]).
    wire_buttons: bool,
    /// Framework theming recolours the scene. Off while the theme goes on
    /// the client's wire (see [`Driver::use_wire_theme`]).
    scene_recolor: bool,
    /// Per client theme: token -> its value (HSLA), for framework theming.
    palettes: Vec<NamedPalette>,
    /// Which theme's values the theme state is seeded with.
    seeded: Option<String>,
    last_pointer: Option<(f32, f32)>,
}

impl Driver {
    /// Start the plugin runtime, load the plugins `args` names, and install
    /// the detours `profile` describes. Fails if the runtime can't start or
    /// a detour can't be installed.
    ///
    /// # Safety
    ///
    /// `profile` describes the client running in this process: every
    /// address is the entry of the function its field names, and every
    /// offset and size is that build's. With a framework layout,
    /// `rekindle_native::configure` has run with this client's spec.
    pub unsafe fn start(args: ExtArgs, profile: RecoveredProfile) -> Result<Driver, String> {
        let shared = Shared {
            theme: Arc::new(Mutex::new({
                let mut theme = ThemeState::new(profile.theme.clone());
                if profile.wire_values {
                    theme.take_values();
                }
                theme
            })),
            commands: Arc::new(Mutex::new(Vec::new())),
            buttons: Arc::new(Mutex::new(Vec::new())),
            picker_tabs: Arc::new(Mutex::new(profile.wire_picker_tabs.then(Vec::new))),
            overlay: Arc::new(Mutex::new(OverlayState::default())),
            surfaces: Arc::new(Mutex::new(backend::SurfaceStyles {
                available: profile.surfaces.clone(),
                ..backend::SurfaceStyles::default()
            })),
            slash: Arc::new(Mutex::new(profile.slash_commands.then(Vec::new))),
            elements: Arc::new(Mutex::new(
                if profile.elements.is_empty() {
                    rekindle_plugin_host::model::NamedElements::default()
                } else {
                    rekindle_plugin_host::model::NamedElements::with_selectors(
                        profile.elements.clone(),
                    )
                },
            )),
        };
        let surfaces = profile
            .registry
            .as_ref()
            .map(|r| r.surfaces)
            .unwrap_or_default();
        // Without a page of rekindle's on the client's settings page, a
        // button in the chat bar opens rekindle's own window.
        rekindle_ui_hook::window::set_button(!surfaces.settings);
        let backend = DetourBackend::new(shared.clone(), surfaces, profile.wire_menus);
        // Gateway dispatches read the client through a backend of their own,
        // so a plugin's call during an event filter never waits on the lock
        // the filter would hold.
        let reader = DetourBackend::new(shared.clone(), surfaces, profile.wire_menus);
        let rekindle = Rekindle::new(backend, reader, args).map_err(|e| format!("{e:#}"))?;

        let theme_hook = profile
            .theme
            .addr
            // SAFETY: the caller guarantees the profile's theme address and
            // offsets are this client's.
            .map(|addr| unsafe { theme::install(addr, &profile.theme) })
            .transpose()
            .map_err(|e| format!("installing the theme detour: {e:#}"))?;
        let ui_registry = profile
            .registry
            // SAFETY: as above, the registries' constructor and layout.
            .map(|layout| unsafe { rekindle_ui_hook::registry::install(layout) })
            .transpose()
            .map_err(|e| format!("registering in the client's registries: {e:#}"))?;
        let ui_compose = profile
            .compose
            // SAFETY: as above, the send method and its channel offset.
            .map(|layout| unsafe { rekindle_ui_hook::install_compose(layout, COMPOSE_TIMEOUT) })
            .transpose()
            .map_err(|e| format!("installing the send detour: {e:#}"))?;
        let ui_framework = profile
            .framework
            // SAFETY: as above, gpui's frame function; the host configured
            // rekindle-native first.
            .map(|layout| unsafe { rekindle_ui_hook::install_framework_layer(layout) })
            .transpose()
            .map_err(|e| format!("installing the framework layer: {e:#}"))?;

        let driver = Driver {
            rekindle,
            shared,
            theme_hook,
            ui_registry,
            _ui_compose: ui_compose,
            _ui_framework: ui_framework,
            framework_buttons: false,
            wire_buttons: false,
            scene_recolor: true,
            palettes: Vec::new(),
            seeded: None,
            last_pointer: None,
        };
        driver.publish_overlay();
        driver.publish_buttons();
        driver.publish_commands();
        Ok(driver)
    }

    /// The plugin runtime.
    pub fn rekindle(&mut self) -> &mut Rekindle {
        &mut self.rekindle
    }

    /// Run the interactions the client routed back since the last call.
    pub fn pump(&mut self) {
        for click in rekindle_ui_hook::drain_clicks() {
            match click {
                Click::Command { plugin, id } => self.rekindle.run_command(&plugin, &id),
                // The chat-bar registry doesn't say which channel the bar is
                // under.
                Click::Button { plugin, item } => {
                    self.rekindle
                        .slot_click(&plugin, SLOT_CHAT_BAR_BUTTON, &item, Target::None);
                }
                Click::Slot { plugin, slot, item } => {
                    self.rekindle
                        .slot_click(&plugin, &slot, &item, Target::None);
                }
                Click::Modal { plugin } => self.rekindle.close_modal(&plugin),
                Click::Profile { user } => self.rekindle.open_profile(&UserRef {
                    id: user,
                    name: String::new(),
                }),
                Click::Menu {
                    plugin,
                    item,
                    target,
                } => {
                    let (slot, target) = menu_target(target);
                    self.rekindle.slot_click(&plugin, slot, &item, target);
                }
                Click::Ui {
                    plugin,
                    panel,
                    element,
                    value,
                } => {
                    let value = match value {
                        UiInput::Toggled(b) => UiValue::Toggled(b),
                        UiInput::Clicked => UiValue::Clicked,
                        UiInput::Text(t) => UiValue::Text(t),
                        UiInput::Selected(i) => UiValue::Selected(i),
                        UiInput::Number(n) => UiValue::Number(n),
                        UiInput::Color(rgba) => {
                            let [r, g, b, a] = rgba.to_be_bytes();
                            UiValue::Color(Color { r, g, b, a })
                        }
                    };
                    self.rekindle.ui_event(
                        &plugin,
                        &UiEvent {
                            surface: panel,
                            element,
                            value,
                        },
                    );
                }
            }
        }
    }

    /// One frame of rekindle's own clock: feed `pointer` plugins the cursor the
    /// client's window last reported (only when it moved), fire due timers,
    /// take work that finished on other threads, and publish what changed.
    /// The host calls it at about 60 Hz; timers keep their own cadence.
    pub fn tick(&mut self) {
        // The plugins may read the client during the tick: the UI thread
        // waits a little for those reads instead of leaving them a frame.
        let _busy = rekindle_ui_hook::client::Busy::begin();
        self.rekindle.start_when_ready();
        self.render_rows();
        if let Some(view) = rekindle_ui_hook::overlay_view() {
            self.shared
                .overlay
                .lock()
                .unwrap()
                .set_viewport(view.viewport);
            if self.last_pointer != Some(view.pointer) {
                self.last_pointer = Some(view.pointer);
                self.rekindle.pointer_moved(view.pointer.0, view.pointer.1);
            }
        }
        self.rekindle.tick_timers();
        self.rekindle.poll();
        self.pump();
        self.publish_overlay();
        self.publish_buttons();
        self.publish_commands();
        self.publish_recolor();
    }

    /// Render the message rows the client drew since the last tick, and
    /// hand their looks back for the client's next frame.
    fn render_rows(&mut self) {
        let rows = rekindle_ui_hook::render::wanted();
        if rows.is_empty() {
            return;
        }
        let inputs: Vec<_> = rows.iter().map(convert::render_input).collect();
        let rendered = self.rekindle.render(&inputs);
        let looks = rows
            .into_iter()
            .zip(rendered)
            .map(|(row, per)| {
                let look = convert::look(
                    &per,
                    |id| self.rekindle.plugin_name(id),
                    |t| self.rekindle.token_rgba(t),
                );
                (row, look)
            })
            .collect();
        rekindle_ui_hook::render::set_looks(looks);
    }

    /// How the plugins' renderers want `messages` to look, each an id and
    /// its content, for a host that reads the client's messages off its
    /// wire. Index `i` of the result is for `messages[i]`.
    pub fn render_messages(
        &mut self,
        messages: &[(u64, String)],
    ) -> Vec<rekindle_ui_hook::render::Look> {
        use rekindle_plugin_host::api::{Message, RenderInput};
        let inputs: Vec<RenderInput> = messages
            .iter()
            .map(|(id, content)| RenderInput {
                message: Message {
                    id: *id,
                    content: content.clone(),
                    ..Default::default()
                },
                author_roles: Vec::new(),
                grouped: false,
                dark: true,
            })
            .collect();
        let rendered = self.rekindle.render(&inputs);
        rendered
            .iter()
            .map(|per| {
                convert::look(
                    per,
                    |id| self.rekindle.plugin_name(id),
                    |t| self.rekindle.token_rgba(t),
                )
            })
            .collect()
    }

    /// Wait up to `wait` for the client to send a message, and run it through
    /// `compose` plugins (the send detour is blocked on the answer). The host
    /// uses it as its tick's sleep, so a send is answered at once. True if a
    /// send was answered.
    pub fn serve_compose(&mut self, wait: std::time::Duration) -> bool {
        let rekindle = &mut self.rekindle;
        rekindle_ui_hook::serve_compose(wait, |channel, text| {
            let out = rekindle.compose(channel, text);
            if out.parts.is_empty() {
                (!out.rewritten_by.is_empty()).then(|| vec![out.text])
            } else {
                Some(out.parts)
            }
        })
    }

    /// Put the plugins' chat-bar buttons in the framework layer (found by
    /// shape, painted and clicked through GPUI) instead of the client's own
    /// chat-bar registry: for a client whose registries aren't recovered.
    pub fn use_framework_chat_bar(&mut self) {
        self.framework_buttons = true;
        rekindle_ui_hook::registry::set_buttons(Vec::new());
        self.publish_buttons();
    }

    /// Take the plugins' chat-bar buttons out of the client's registry and
    /// the framework layer, for a client whose chat bar is reached over its
    /// wire. The host mounts them there from [`Driver::chat_bar_buttons`].
    pub fn use_wire_chat_bar(&mut self) {
        self.wire_buttons = true;
        if self.framework_buttons {
            // rekindle's own button stays in the framework layer.
            rekindle_ui_hook::set_framework_buttons(true, Vec::new());
        } else if self.ui_registry.is_some() {
            rekindle_ui_hook::registry::set_buttons(Vec::new());
        }
    }

    /// The plugins' tabs in the client's emoji picker, in the order the
    /// plugins added them.
    pub fn picker_tabs(&self) -> Vec<PickerTab> {
        self.shared
            .picker_tabs
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
    }

    /// What each plugin did to the client's elements by name, and the
    /// `wire-raw` plugins' own work.
    pub fn named_elements(&self) -> rekindle_plugin_host::model::NamedElements {
        self.shared.elements.lock().unwrap().clone()
    }

    /// The plugins' chat-bar buttons, as (plugin, button), in the order the
    /// plugins added them.
    pub fn chat_bar_buttons(&self) -> Vec<(String, Button)> {
        self.shared.buttons.lock().unwrap().clone()
    }

    /// The value in effect for each theme token that isn't a colour and a
    /// plugin set: density, a length or a font.
    pub fn theme_values(&self) -> Vec<(String, rekindle_plugin_host::model::ThemeValue)> {
        self.shared.theme.lock().unwrap().values()
    }

    /// The plugins' context-menu rows, in the order they were added. Each
    /// row's key joins its plugin and item id.
    pub fn menu_items(&self) -> Vec<rekindle_ui_hook::MenuItem> {
        rekindle_ui_hook::menu_rows()
    }

    /// The plugins' slash commands, as (plugin, command), in the order they
    /// were registered.
    pub fn slash_commands(&self) -> Vec<(String, rekindle_plugin_host::api::SlashCommand)> {
        self.shared
            .slash
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
    }

    /// The style that applies to each named surface a plugin restyled, by
    /// surface: the one set last.
    pub fn surface_styles(&self) -> Vec<(String, rekindle_plugin_host::model::SurfaceStyle)> {
        self.shared.surfaces.lock().unwrap().applied()
    }

    /// Stop recolouring the scene, for a client whose theme goes on its
    /// wire. The palettes stay, so [`Driver::recolors`] still has the
    /// pairs, and the framework layer still detects which theme is shown.
    /// Without this, a colour the wire wrote could be recoloured again in
    /// the scene, when it equals another token's value.
    pub fn use_wire_theme(&mut self) {
        self.scene_recolor = false;
        self.publish_recolor();
    }

    /// Whether framework theming recolours the scene.
    pub fn scene_recolor(&self) -> bool {
        self.scene_recolor
    }

    /// Theme through the framework instead of the client's theme function:
    /// restores the client's function, and from now on recolours each finished
    /// GPUI scene to the plugins' overrides, using whichever of `palettes`
    /// (theme name -> token -> the theme's value, `0xRRGGBB`) the client is
    /// showing.
    pub fn use_framework_theme(&mut self, palettes: Vec<(String, Vec<(String, u32)>)>) {
        self.theme_hook = None;
        self.palettes = palettes
            .into_iter()
            .map(|(name, tokens)| {
                let tokens = tokens
                    .into_iter()
                    .map(|(t, rgb)| (t, rekindle_native::Rgba::hex((rgb << 8) | 0xff).to_hsla()))
                    .collect();
                (name, tokens)
            })
            .collect();
        self.seeded = None;
        self.publish_recolor();
    }

    /// Live overlay sprites across all plugins.
    pub fn overlay_sprites(&self) -> usize {
        self.shared.overlay.lock().unwrap().sprite_count()
    }

    /// The plugin whose override of `token` applies, if any.
    pub fn override_owner(&self, token: &str) -> Option<String> {
        self.shared.theme.lock().unwrap().override_owner(token)
    }

    fn publish_overlay(&self) {
        if let Some(items) = self.shared.overlay.lock().unwrap().take_items() {
            rekindle_ui_hook::set_overlay(items);
        }
    }

    fn publish_buttons(&self) {
        let list: Vec<Button> = self
            .shared
            .buttons
            .lock()
            .unwrap()
            .iter()
            .map(|(_, b)| b.clone())
            .collect();
        if self.wire_buttons {
            return;
        }
        if self.framework_buttons {
            rekindle_ui_hook::set_framework_buttons(true, list);
        } else if self.ui_registry.is_some() {
            rekindle_ui_hook::registry::set_buttons(list);
        }
    }

    /// The plugins' commands, as the client's switcher lists them.
    fn publish_commands(&self) {
        if self.ui_registry.is_none() {
            return;
        }
        let list = self
            .shared
            .commands
            .lock()
            .unwrap()
            .iter()
            .map(|c| Command {
                key: c.key.clone(),
                section: "extensions".into(),
                glyph: ">".into(),
                title: c.title.clone(),
                detail: c.plugin.clone(),
            })
            .collect();
        rekindle_ui_hook::registry::set_commands(list);
    }

    fn publish_recolor(&mut self) {
        if self.palettes.is_empty() {
            return;
        }
        // Plugin reads of a token's current value follow the detected theme.
        let seed = rekindle_ui_hook::detected_theme()
            .or_else(|| self.palettes.first().map(|p| p.0.clone()));
        if seed != self.seeded {
            if let Some((_, tokens)) = self.palettes.iter().find(|p| Some(&p.0) == seed.as_ref()) {
                let state = self.shared.theme.lock().unwrap();
                for (t, v) in tokens {
                    state.seed_base(t, *v);
                }
            }
            self.seeded = seed;
        }
        let mut palettes = self.recolors();
        if !self.scene_recolor {
            for p in &mut palettes {
                p.pairs.clear();
            }
        }
        rekindle_ui_hook::set_theme_palettes(palettes);
    }

    /// The colour each token a plugin sets has, as HSLA: the plugin's that
    /// set it last.
    pub fn theme_overrides(&self) -> std::collections::BTreeMap<String, [f32; 4]> {
        self.shared.theme.lock().unwrap().overrides()
    }

    /// For each client theme of [`Driver::use_framework_theme`], its token
    /// values and the plugins' overrides as (the theme's value, the
    /// override) pairs (HSLA). Framework theming recolours the scene with
    /// them, and a client split across a wire can have its cells recoloured
    /// with them.
    pub fn recolors(&self) -> Vec<rekindle_ui_hook::ThemePalette> {
        let overrides = self.shared.theme.lock().unwrap().overrides();
        self.palettes
            .iter()
            .map(|(name, tokens)| rekindle_ui_hook::ThemePalette {
                name: name.clone(),
                colors: tokens.iter().map(|(_, v)| *v).collect(),
                pairs: tokens
                    .iter()
                    .filter_map(|(t, from)| {
                        overrides
                            .get(t)
                            .filter(|to| to.map(f32::to_bits) != from.map(f32::to_bits))
                            .map(|to| (*from, *to))
                    })
                    .collect(),
            })
            .collect()
    }
}

/// The slot and target a context-menu click goes to.
fn menu_target(target: MenuTargetRef) -> (&'static str, Target) {
    match target {
        MenuTargetRef::Message {
            id,
            channel,
            author,
        } => (
            SLOT_MESSAGE_MENU,
            Target::Message(MessageRef {
                id,
                channel,
                guild: None,
                author,
            }),
        ),
        MenuTargetRef::User { id } => (
            SLOT_USER_MENU,
            Target::User((
                UserRef {
                    id,
                    name: String::new(),
                },
                None,
            )),
        ),
        MenuTargetRef::Channel { id } => (
            SLOT_CHANNEL_MENU,
            Target::Channel(ChannelRef {
                id,
                name: String::new(),
                guild: None,
                kind: 0,
            }),
        ),
    }
}
