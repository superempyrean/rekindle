//! Type-free UI contributions, driven by recovered layout. Depends only on
//! `rekindle-hook` and `rekindle-native`; names no client type.
//!
//! Two layers:
//!
//! - **the client's own registries** ([`registry`]): rekindle registers as one
//!   more provider (switcher, chat bar, context menus, profile popout, modals)
//!   and adds its own entries (a settings page, an overlay layer) when the
//!   client builds them, so the client lays out, draws and routes rekindle's
//!   contributions exactly as it does its own;
//! - **the framework layer** (gpui's `Window::draw_roots`): rekindle's own
//!   elements (toasts, sprites, rekindle's window; chat-bar buttons found by
//!   shape when the client's registries aren't available) in the client's
//!   element tree, and theming by recolouring the finished scene.
//!
//! Every value handed to the client is the client's own type, built as bytes
//! at recovered offsets: no client type is named.
//!
//! The detours run on the client's UI thread; the plugin runtime sets what
//! they show from its own thread through the `set_*` functions and drains
//! what the user did with [`drain_clicks`].

mod chat_bar;
pub mod client;
mod clipboard;
mod compose;
mod discovery;
/// Test harness: drive rekindle's UI in a live client from inside.
pub mod drive;
mod framework;
mod insert;
mod inspect_link;
pub mod inspector;
mod overlay;
mod panel;
mod picker;
mod provenance;
pub mod registry;
pub mod render;
mod theme;
mod toast;
pub mod window;

use std::sync::Mutex;

pub(crate) use chat_bar::BUTTON_CLICKS;
pub use chat_bar::{button_clicks, chat_bar_frames, last_frame_geometry, set_framework_buttons};
pub use clipboard::{pending_clipboard, queue_clipboard, take_clipboard};
pub use compose::{ComposeLayout, install_compose, serve_compose};
pub use framework::{
    FrameHook, FrameworkLayout, framework_failed, framework_frames, install_framework_layer,
    native_painted,
};
pub use insert::{queue_insert, take_inserts};
pub use overlay::{
    OverlayItem, OverlayPicture, OverlayRect, OverlayView, overlay_view, set_overlay,
};
pub use panel::{
    CardItem, PanelItem, UiInput, Widget, panels, report_ui, set_panels, set_shown_plugin,
    shown_plugin,
};
pub use registry::{
    Button, Command, MenuItem, MenuKind, MenuTargetRef, Modal, ProfileSectionItem, menu_rows,
    modal_panel, modal_plugin, set_menu_items, set_modal, set_profiles,
};
pub use theme::{ThemePalette, detected_theme, recolored_last_frame, set_theme_palettes};
pub use toast::{
    LiveToast, MAX_TOASTS, ToastStyle, dismiss_toast, live_toasts, push_toast, push_toast_for, toast_cards,
    toast_texts,
};

/// Where the three machine words of a `Vec` sit, as byte offsets. rustc
/// reorders them; here they are recovered (`[cap, ptr, len]` on the pinned
/// toolchain), not assumed.
#[derive(Clone, Copy, Debug, Default)]
pub struct VecLayout {
    pub cap_off: usize,
    pub ptr_off: usize,
    pub len_off: usize,
}

/// A client interaction rekindle's detours routed back. `plugin` and `id` are
/// the two halves of a rekindle key.
pub enum Click {
    /// A switcher command was confirmed.
    Command { plugin: String, id: String },
    /// A chat-bar button was clicked.
    Button { plugin: String, item: String },
    /// An item of another slot whose items have no target was clicked: a
    /// tab in the emoji picker.
    Slot {
        plugin: String,
        slot: String,
        item: String,
    },
    /// A modal was dismissed.
    Modal { plugin: String },
    /// A profile popout opened.
    Profile { user: u64 },
    /// One of rekindle's context-menu rows was clicked.
    Menu {
        plugin: String,
        item: String,
        target: MenuTargetRef,
    },
    /// The user changed an element of a plugin panel on the settings page.
    Ui {
        plugin: String,
        panel: String,
        element: String,
        value: UiInput,
    },
}

static QUEUE: Mutex<Vec<Click>> = Mutex::new(Vec::new());

/// The plugin and item id a rekindle key joins (see the extension's `key`).
pub(crate) fn split_key(key: &str) -> (String, String) {
    match key.split_once('\u{1f}') {
        Some((p, i)) => (p.to_string(), i.to_string()),
        None => (key.to_string(), String::new()),
    }
}

/// The interactions the client routed to rekindle since the last drain.
pub fn drain_clicks() -> Vec<Click> {
    std::mem::take(&mut QUEUE.lock().unwrap())
}

/// Queue an interaction for [`drain_clicks`]. The host queues a press the
/// wire engine kept for a plugin here, so it reaches the plugin as any
/// other click on its controls does.
pub fn queue_click(click: Click) {
    QUEUE.lock().unwrap().push(click);
}

/// Installed detours. Dropping it restores the client's functions.
pub struct Hook {
    _detours: Vec<rekindle_hook::Installed>,
}

impl Hook {
    pub(crate) fn new(detours: Vec<rekindle_hook::Installed>) -> Self {
        Self { _detours: detours }
    }
}
