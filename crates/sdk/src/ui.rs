//! Panels, modals, settings-page extras, palette commands, keybinds,
//! toasts, notices, images and navigation (requires `ui`).
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! Panel::new("settings", "My plugin")
//!     .heading("Appearance")
//!     .toggle("compact", "Compact mode", true)
//!     .color_picker("accent", "Accent", Color::rgb(88, 101, 242))
//!     .show();
//! ```

use crate::raw::rekindle::plugin::types as t;
use crate::raw::rekindle::plugin::ui as raw;
use crate::{Color, Icon, Span, Target};
pub use raw::{Notice, ProfileArt, ToastKind, UiError};
pub use t::Element;

/// The [`Plugin::on_ui`](crate::Plugin::on_ui) surface of the elements
/// under the plugin's settings page.
pub const SETTINGS_PAGE: &str = "settings-page";
/// The [`Plugin::on_ui`](crate::Plugin::on_ui) surface of the notice's
/// button.
pub const NOTICE: &str = "notice";

/// A panel the host renders with native widgets, built up with the
/// [`Elements`] methods.
#[derive(Debug, Clone)]
pub struct Panel(raw::Panel);

impl Panel {
    /// An empty panel. `id` is the surface its events carry.
    pub fn new(id: &str, title: &str) -> Self {
        Self(raw::Panel {
            id: id.into(),
            title: title.into(),
            elements: Vec::new(),
        })
    }

    /// Shows the panel, replacing the one with the same id.
    pub fn show(&self) {
        raw::show_panel(&self.0);
    }

    /// Shows the panel as a modal dialog over the client. A plugin has one
    /// modal at a time, and the user can always dismiss it.
    pub fn show_modal(&self) {
        raw::show_modal(&self.0);
    }

    /// The contract's record for the panel.
    pub fn raw(&self) -> &raw::Panel {
        &self.0
    }
}

/// Element builders, shared by panels, modals, profile sections, message
/// accessories and the settings page's extras. A `Vec<Element>` has them
/// too.
pub trait Elements: Sized {
    /// Adds an element.
    fn element(self, e: Element) -> Self;

    /// Adds a heading.
    fn heading(self, text: &str) -> Self {
        self.element(Element::Heading(text.into()))
    }

    /// Adds a paragraph of text.
    fn text(self, text: &str) -> Self {
        self.element(Element::Text(text.into()))
    }

    /// Adds styled text, built from [`span`]s.
    fn rich_text(self, spans: Vec<Span>) -> Self {
        self.element(Element::RichText(spans))
    }

    /// Adds a horizontal rule.
    fn separator(self) -> Self {
        self.element(Element::Separator)
    }

    /// Adds a switch. Changes arrive as `UiValue::Toggled`.
    fn toggle(self, id: &str, label: &str, value: bool) -> Self {
        self.element(Element::Toggle(t::Toggle {
            id: id.into(),
            label: label.into(),
            value,
        }))
    }

    /// Adds a slider over `range` in steps of `step`. Changes arrive as
    /// `UiValue::Number`.
    fn slider(
        self,
        id: &str,
        label: &str,
        range: std::ops::RangeInclusive<f64>,
        step: f64,
        value: f64,
    ) -> Self {
        self.element(Element::Slider(t::Slider {
            id: id.into(),
            label: label.into(),
            min: *range.start(),
            max: *range.end(),
            step,
            value,
        }))
    }

    /// Adds a colour picker. Changes arrive as `UiValue::Color`.
    fn color_picker(self, id: &str, label: &str, value: Color) -> Self {
        self.element(Element::ColorPicker(t::ColorPicker {
            id: id.into(),
            label: label.into(),
            value,
        }))
    }

    /// Adds a one-line text field. Its value arrives as `UiValue::Text` when
    /// the user commits it, not on every keystroke.
    fn text_input(self, id: &str, label: &str, value: &str, placeholder: &str) -> Self {
        self.element(Element::TextInput(t::TextInput {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            placeholder: placeholder.into(),
            multiline: false,
        }))
    }

    /// [`text_input`](Elements::text_input) with several lines.
    fn text_area(self, id: &str, label: &str, value: &str, placeholder: &str) -> Self {
        self.element(Element::TextInput(t::TextInput {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            placeholder: placeholder.into(),
            multiline: true,
        }))
    }

    /// Adds a button. Clicks arrive as `UiValue::Clicked`.
    fn button(self, id: &str, label: &str) -> Self {
        self.element(Element::Button(t::Button {
            id: id.into(),
            label: label.into(),
            icon: None,
            danger: false,
        }))
    }

    /// [`button`](Elements::button) with one of the client's icons.
    fn icon_button(self, id: &str, icon: Icon, label: &str) -> Self {
        self.element(Element::Button(t::Button {
            id: id.into(),
            label: label.into(),
            icon: Some(icon),
            danger: false,
        }))
    }

    /// [`button`](Elements::button) in the destructive style.
    fn danger_button(self, id: &str, label: &str) -> Self {
        self.element(Element::Button(t::Button {
            id: id.into(),
            label: label.into(),
            icon: None,
            danger: true,
        }))
    }

    /// Adds a drop-down of `options` with `selected` chosen. Changes arrive
    /// as `UiValue::Selected`.
    fn select(self, id: &str, label: &str, options: &[&str], selected: u32) -> Self {
        self.element(Element::Select(t::Select {
            id: id.into(),
            label: label.into(),
            options: options.iter().map(|o| (*o).to_string()).collect(),
            selected,
        }))
    }

    /// Adds an editable table: rows of cells, one per column. Edits arrive
    /// as `UiValue::Rows`.
    fn table(self, id: &str, label: &str, columns: &[&str], rows: Vec<Vec<String>>) -> Self {
        self.element(Element::Table(t::Table {
            id: id.into(),
            label: label.into(),
            columns: columns.iter().map(|c| (*c).to_string()).collect(),
            rows,
        }))
    }

    /// Adds an editable list of strings. Edits arrive as `UiValue::Items`.
    fn string_list(self, id: &str, label: &str, items: Vec<String>, placeholder: &str) -> Self {
        self.element(Element::StringList(t::StringList {
            id: id.into(),
            label: label.into(),
            items,
            placeholder: placeholder.into(),
        }))
    }

    /// Adds one of the plugin's images, registered with [`register_image`].
    fn image(self, id: &str, width: u32, height: u32) -> Self {
        self.element(Element::Image((id.into(), width, height)))
    }

    /// Adds a user, drawn the client's way with avatar and name.
    fn user(self, id: u64) -> Self {
        self.element(Element::User(id))
    }

    /// Adds a key/value row, such as ("Created", "3 years ago").
    fn field(self, key: &str, value: &str) -> Self {
        self.element(Element::Field((key.into(), value.into())))
    }
}

impl Elements for Panel {
    fn element(mut self, e: Element) -> Self {
        self.0.elements.push(e);
        self
    }
}

impl Elements for Vec<Element> {
    fn element(mut self, e: Element) -> Self {
        self.push(e);
        self
    }
}

/// A plain text run, for [`Elements::rich_text`] and the other places that
/// take spans. [`SpanExt`] styles it.
pub fn span(text: &str) -> Span {
    Span {
        text: text.into(),
        bold: false,
        italic: false,
        code: false,
        color: None,
        target: None,
    }
}

/// Styling for a [`Span`]: `span("x").bold().color_token("text-muted")`.
pub trait SpanExt {
    /// Makes it bold.
    fn bold(self) -> Self;
    /// Makes it italic.
    fn italic(self) -> Self;
    /// Shows it as inline code.
    fn code(self) -> Self;
    /// Colours it with a theme token, such as `"text-muted"`.
    fn color_token(self, token: &str) -> Self;
    /// Colours it with a fixed colour.
    fn color(self, c: Color) -> Self;
    /// Makes clicking it go to `t`. The host handles the click.
    fn target(self, t: Target) -> Self;
}

impl SpanExt for Span {
    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    fn code(mut self) -> Self {
        self.code = true;
        self
    }

    fn color_token(mut self, token: &str) -> Self {
        self.color = Some(t::SpanColor::Token(token.into()));
        self
    }

    fn color(mut self, c: Color) -> Self {
        self.color = Some(t::SpanColor::Rgba(c));
        self
    }

    fn target(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }
}

/// Removes one of the plugin's panels.
pub fn remove_panel(id: &str) {
    raw::remove_panel(id);
}

/// Closes the plugin's modal.
pub fn close_modal() {
    raw::close_modal();
}

/// Elements under the plugin's generated settings page, for what the
/// schema can't express (a preview, a "test" button). Their events carry
/// the surface [`SETTINGS_PAGE`].
pub fn set_settings_extra(elements: &[Element]) {
    raw::set_settings_extra(elements);
}

/// Adds a command-palette entry. Picking it calls
/// [`Plugin::on_command`](crate::Plugin::on_command) with `id`.
pub fn register_command(id: &str, title: &str) {
    raw::register_command(id, title);
}

/// Removes a command-palette entry.
pub fn unregister_command(id: &str) {
    raw::unregister_command(id);
}

/// Adds a keybind to the client's keybind settings, where the user can
/// rebind it. It works only while the client window has focus. `default`
/// is written like `"Ctrl+Shift+K"`. Presses call
/// [`Plugin::on_keybind`](crate::Plugin::on_keybind) with `id`.
pub fn register_keybind(id: &str, label: &str, default: Option<&str>) -> Result<(), UiError> {
    raw::register_keybind(id, label, default)
}

/// Removes a keybind.
pub fn unregister_keybind(id: &str) {
    raw::unregister_keybind(id);
}

/// A transient message of kind `info`. The host shows at most 3 per 10 s.
pub fn toast(text: &str) {
    raw::show_toast(text, ToastKind::Info);
}

/// A transient message of another kind: `success`, `warning` or `error`.
pub fn toast_kind(text: &str, kind: ToastKind) {
    raw::show_toast(text, kind);
}

/// A banner across the top of the client, one per plugin, with an optional
/// button. The button's clicks carry the surface [`NOTICE`].
pub fn notice(text: &str, kind: ToastKind, button: Option<&str>) {
    raw::show_notice(&Notice {
        text: text.into(),
        kind,
        button: button.map(Into::into),
    });
}

/// Removes the plugin's notice.
pub fn dismiss_notice() {
    raw::dismiss_notice();
}

/// Registers a PNG of at most 256 KiB and 256x256 under `id`, for icons,
/// badges, panel images and rich-presence assets. Images come with the
/// plugin and are never fetched.
pub fn register_image(id: &str, png: &[u8]) -> Result<(), UiError> {
    raw::register_image(id, png)
}

/// Goes to a channel, message, guild or profile, as clicking it would. Only
/// during a user gesture.
pub fn navigate(to: &Target) -> Result<(), UiError> {
    raw::navigate(to)
}

/// Opens an https URL in the browser, after the client's "leaving Discord"
/// confirmation. Only during a user gesture.
pub fn open_url(url: &str) -> Result<(), UiError> {
    raw::open_url(url)
}

/// Opens the client's media viewer on an image the client has. Only during
/// a user gesture.
pub fn open_media(url: &str) -> Result<(), UiError> {
    raw::open_media(url)
}

/// Shows another avatar, banner or both for `user`, from images registered
/// with [`register_image`]. Only this client shows them; everyone else
/// still sees the user's own.
pub fn set_profile_art(
    user: u64,
    avatar: Option<&str>,
    banner: Option<&str>,
) -> Result<(), UiError> {
    raw::set_profile_art(
        user,
        Some(&ProfileArt {
            avatar: avatar.map(String::from),
            banner: banner.map(String::from),
        }),
    )
}

/// Shows `user`'s own avatar and banner again.
pub fn clear_profile_art(user: u64) -> Result<(), UiError> {
    raw::set_profile_art(user, None)
}

/// Keeps the client window above other windows, until the plugin unloads.
pub fn set_window_on_top(on_top: bool) -> Result<(), UiError> {
    raw::set_window_on_top(on_top)
}

/// Sets the client window's opacity, from 0.3 to 1, until the plugin
/// unloads.
pub fn set_window_opacity(opacity: f32) -> Result<(), UiError> {
    raw::set_window_opacity(opacity)
}
