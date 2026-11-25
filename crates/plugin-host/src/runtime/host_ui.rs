//! `ui` and `render-control` (design doc §8, §6).

use crate::api;
use crate::bindings::wit;
use crate::capability::Capability;
use crate::model::{self, Action, ClickGesture, DECORATION_KINDS, UiError, UiOp, slot_kind};

use super::convert::{self, ui_error_to_wit};
use super::host_services::image_dims;
use super::state::{Budget, PluginState, clean, valid_id};

type UiResult = wasmtime::Result<Result<(), wit::ui::UiError>>;

/// Per-plugin limits (design doc §3.5).
const MAX_SLOT_ITEMS: usize = 25;
const MAX_LABEL: usize = 48;
const MAX_DECORATIONS: usize = 5000;
const MAX_ACCESSORIES: usize = 200;
const MAX_GHOSTS_PER_CHANNEL: usize = 500;
const MAX_KEYBINDS: usize = 10;
const MAX_SLASH_COMMANDS: usize = 25;
const MAX_CLICK_BINDINGS: usize = 8;
const MAX_IMAGES: usize = 32;
const MAX_IMAGE_BYTES: usize = 256 * 1024;
const MAX_IMAGE_DIM: u32 = 256;
const TOAST_BURST: usize = 3;
const TOAST_WINDOW_MS: u64 = 10_000;

fn valid_label(l: &str) -> bool {
    let n = l.chars().count();
    n > 0 && n <= MAX_LABEL && !l.trim().is_empty() && !l.chars().any(char::is_control)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "returns the host function's own type, so a call site is a plain `return`"
)]
fn err(e: UiError) -> UiResult {
    Ok(Err(ui_error_to_wit(e)))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "returns the host function's own type, so a call site is a plain `return`"
)]
fn reply(r: Result<(), UiError>) -> UiResult {
    Ok(r.map_err(ui_error_to_wit))
}

impl PluginState {
    fn ui_op(&mut self, op: UiOp) -> Result<(), UiError> {
        self.backend.lock().unwrap().ui_update(&self.id, op)
    }

    /// A user gesture is being delivered and its navigation budget allows
    /// one more (rate-limited too).
    fn navigation_ok(&mut self) -> bool {
        let now = self.now();
        self.spend(Budget::Navigation) && self.rates.nav.take(now, 10, 10_000)
    }
}

/// Valid keybind notation: `Ctrl+Shift+K` (modifiers then one key).
fn valid_keybind(k: &str) -> bool {
    let Some((mods, key)) = k.rsplit_once('+') else {
        return false;
    };
    !key.is_empty()
        && key.len() <= 12
        && key.chars().all(|c| c.is_ascii_alphanumeric())
        && mods
            .split('+')
            .all(|m| matches!(m, "Ctrl" | "Shift" | "Alt" | "Meta" | "Cmd"))
}

fn valid_command_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 32
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

impl wit::ui::Host for PluginState {
    fn register_image(&mut self, id: String, png: Vec<u8>) -> UiResult {
        self.require(Capability::Ui)?;
        if !valid_id(&id) {
            return err(UiError::Invalid);
        }
        if png.len() > MAX_IMAGE_BYTES {
            return err(UiError::Limit);
        }
        let Some((w, h)) = image_dims(&png) else {
            return err(UiError::Invalid);
        };
        if w > MAX_IMAGE_DIM || h > MAX_IMAGE_DIM {
            return err(UiError::Limit);
        }
        {
            let mut sh = self.shared.lock().unwrap();
            if !sh.images.contains_key(&id) && sh.images.len() >= MAX_IMAGES {
                return err(UiError::Limit);
            }
            sh.images.insert(id.clone(), (w, h));
        }
        // A backend that can't draw images still loads the plugin.
        let _ = self.ui_op(UiOp::RegisterImage {
            id,
            png,
            width: w,
            height: h,
        });
        Ok(Ok(()))
    }

    fn show_panel(&mut self, panel: wit::ui::Panel) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        let panel = convert::panel_from_wit(panel);
        self.backend.lock().unwrap().show_panel(&self.id, panel);
        Ok(())
    }

    fn remove_panel(&mut self, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.backend.lock().unwrap().remove_panel(&self.id, &id);
        Ok(())
    }

    fn show_modal(&mut self, panel: wit::ui::Panel) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        let panel = convert::panel_from_wit(panel);
        self.backend.lock().unwrap().show_modal(&self.id, panel);
        Ok(())
    }

    fn close_modal(&mut self) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.backend.lock().unwrap().close_modal(&self.id);
        Ok(())
    }

    fn set_settings_extra(&mut self, elements: Vec<wit::types::Element>) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        let elements = convert::elements_from_wit(elements);
        let _ = self.ui_op(UiOp::SetSettingsExtra { elements });
        Ok(())
    }

    fn register_command(&mut self, id: String, title: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        let title = clean(&title, 100).unwrap_or_else(|| id.clone());
        self.backend
            .lock()
            .unwrap()
            .register_command(&self.id, &id, &title);
        self.commands.insert(id);
        Ok(())
    }

    fn unregister_command(&mut self, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.backend
            .lock()
            .unwrap()
            .unregister_command(&self.id, &id);
        self.commands.remove(&id);
        Ok(())
    }

    fn register_slash_command(&mut self, command: api::SlashCommand) -> UiResult {
        self.require(Capability::Ui)?;
        let ok = valid_command_name(&command.name)
            && clean(&command.description, 100).is_some()
            && command.options.len() <= 25
            && command
                .options
                .iter()
                .all(|o| valid_command_name(&o.name) && o.choices.len() <= 25)
            && (command.in_guilds || command.in_dms);
        if !ok {
            return err(UiError::Invalid);
        }
        if !self.slash_commands.contains(&command.name)
            && self.slash_commands.len() >= MAX_SLASH_COMMANDS
        {
            return err(UiError::Limit);
        }
        let name = command.name.clone();
        let r = self.ui_op(UiOp::RegisterSlashCommand { command });
        if r.is_ok() {
            self.slash_commands.insert(name);
        }
        reply(r)
    }

    fn unregister_slash_command(&mut self, name: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.slash_commands.remove(&name);
        let _ = self.ui_op(UiOp::UnregisterSlashCommand { name });
        Ok(())
    }

    fn register_keybind(&mut self, id: String, label: String, default: Option<String>) -> UiResult {
        self.require(Capability::Ui)?;
        if !valid_id(&id)
            || !valid_label(&label)
            || default.as_deref().is_some_and(|d| !valid_keybind(d))
        {
            return err(UiError::Invalid);
        }
        if !self.keybinds.contains(&id) && self.keybinds.len() >= MAX_KEYBINDS {
            return err(UiError::Limit);
        }
        let r = self.ui_op(UiOp::RegisterKeybind {
            id: id.clone(),
            label,
            default,
        });
        if r.is_ok() {
            self.keybinds.insert(id);
        }
        reply(r)
    }

    fn unregister_keybind(&mut self, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.keybinds.remove(&id);
        let _ = self.ui_op(UiOp::UnregisterKeybind { id });
        Ok(())
    }

    fn available_slots(&mut self) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::Ui)?;
        Ok(self.backend.lock().unwrap().available_slots())
    }

    fn add_item(&mut self, slot: String, item: wit::ui::SlotItem) -> UiResult {
        self.require(Capability::Ui)?;
        let item = convert::slot_item_from_wit(item);
        let Some(kind) = slot_kind(&slot) else {
            return err(UiError::Unavailable);
        };
        if kind != item.kind() || !valid_id(item.id()) || !valid_label(item.label()) {
            return err(UiError::Invalid);
        }
        if let model::SlotItem::IconButton {
            image: Some(img), ..
        } = &item
            && !self.image_registered(img)
        {
            return err(UiError::Invalid);
        }
        let key = (slot.clone(), item.id().to_string());
        let used = self.slot_items.iter().filter(|(s, _)| *s == slot).count();
        if !self.slot_items.contains(&key) && used >= MAX_SLOT_ITEMS {
            return err(UiError::Limit);
        }
        let r = self
            .backend
            .lock()
            .unwrap()
            .add_slot_item(&self.id, &slot, item);
        if matches!(r, Err(model::SlotError::Unavailable)) {
            self.missing_slots.insert(slot.clone());
        }
        if r.is_ok() {
            self.ledger.slot(&slot);
            self.slot_items.insert(key);
        }
        reply(r.map_err(UiError::from))
    }

    fn remove_item(&mut self, slot: String, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.slot_items.remove(&(slot.clone(), id.clone()));
        self.backend
            .lock()
            .unwrap()
            .remove_slot_item(&self.id, &slot, &id);
        Ok(())
    }

    fn set_active(&mut self, slot: String, id: String, active: bool) -> UiResult {
        self.require(Capability::Ui)?;
        if !self.slot_items.contains(&(slot.clone(), id.clone())) {
            return err(UiError::NotFound);
        }
        reply(self.ui_op(UiOp::SetActive { slot, id, active }))
    }

    fn set_profile_section(
        &mut self,
        section_id: String,
        user: u64,
        elements: Vec<wit::types::Element>,
    ) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        if !self
            .slot_items
            .contains(&(model::SLOT_PROFILE_SECTION.to_string(), section_id.clone()))
        {
            self.warn(format!(
                "set-profile-section for unknown section `{section_id}` ignored"
            ));
            return Ok(());
        }
        let elements = convert::elements_from_wit(elements);
        self.backend
            .lock()
            .unwrap()
            .set_profile_section(&self.id, &section_id, user, elements);
        Ok(())
    }

    fn set_profile_art(&mut self, user: u64, art: Option<wit::ui::ProfileArt>) -> UiResult {
        self.require(Capability::Ui)?;
        self.main_only("ui.set-profile-art")?;
        let (avatar, banner) = art.map(|a| (a.avatar, a.banner)).unwrap_or_default();
        if [&avatar, &banner]
            .into_iter()
            .flatten()
            .any(|id| !self.image_registered(id))
        {
            return err(UiError::Invalid);
        }
        reply(self.ui_op(UiOp::SetProfileArt {
            user,
            avatar,
            banner,
        }))
    }

    fn set_window_on_top(&mut self, on_top: bool) -> UiResult {
        self.require(Capability::Ui)?;
        self.main_only("ui.set-window-on-top")?;
        reply(self.ui_op(UiOp::SetWindowOnTop { on_top }))
    }

    fn set_window_opacity(&mut self, opacity: f32) -> UiResult {
        self.require(Capability::Ui)?;
        self.main_only("ui.set-window-opacity")?;
        if !opacity.is_finite() {
            return err(UiError::Invalid);
        }
        reply(self.ui_op(UiOp::SetWindowOpacity {
            opacity: opacity.clamp(0.3, 1.0),
        }))
    }

    fn set_voice_status(&mut self, id: String, text: String, since_ms: Option<u64>) -> UiResult {
        self.require(Capability::Ui)?;
        if !valid_id(&id) {
            return err(UiError::Invalid);
        }
        let text = text.chars().filter(|c| !c.is_control()).take(64).collect();
        reply(self.ui_op(UiOp::SetVoiceStatus { id, text, since_ms }))
    }

    fn register_click(&mut self, binding: wit::ui::ClickBinding) -> UiResult {
        self.require(Capability::Ui)?;
        let mods = convert::mods_from_wit(binding.modifiers);
        let ok = valid_id(&binding.id)
            && valid_label(&binding.label)
            && matches!(binding.clicks, 1 | 2)
            // A plain single click is the client's own (select, open, ...).
            && (binding.clicks == 2 || mods.any());
        if !ok {
            return err(UiError::Invalid);
        }
        if !self.click_bindings.contains_key(&binding.id)
            && self.click_bindings.len() >= MAX_CLICK_BINDINGS
        {
            return err(UiError::Limit);
        }
        let b = model::ClickBinding {
            id: binding.id.clone(),
            label: binding.label,
            gesture: ClickGesture {
                area: convert::click_area_from_wit(binding.area),
                clicks: binding.clicks,
                mods,
            },
            when: binding
                .when
                .into_iter()
                .map(convert::predicate_from_wit)
                .collect(),
        };
        let r = self.ui_op(UiOp::RegisterClick { binding: b.clone() });
        if r.is_ok() {
            self.click_bindings.insert(binding.id, b);
        }
        reply(r)
    }

    fn unregister_click(&mut self, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.click_bindings.remove(&id);
        let _ = self.ui_op(UiOp::UnregisterClick { id });
        Ok(())
    }

    fn available_decorations(&mut self) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::Ui)?;
        Ok(self.backend.lock().unwrap().available_decorations())
    }

    fn set_decoration(
        &mut self,
        kind: String,
        key: u64,
        decoration: Option<wit::ui::Decoration>,
    ) -> UiResult {
        self.require(Capability::Ui)?;
        if !DECORATION_KINDS.iter().any(|(k, _)| *k == kind) {
            return err(UiError::Unavailable);
        }
        let d = decoration.map(convert::decoration_from_wit);
        if let Some(d) = &d {
            if d.image
                .as_deref()
                .is_some_and(|i| !self.image_registered(i))
            {
                return err(UiError::Invalid);
            }
            if !self.decorations.contains(&(kind.clone(), key))
                && self.decorations.len() >= MAX_DECORATIONS
            {
                return err(UiError::Limit);
            }
        }
        let set = d.is_some();
        let r = self.ui_op(UiOp::SetDecoration {
            kind: kind.clone(),
            key,
            decoration: d,
        });
        if r.is_ok() {
            if set {
                self.decorations.insert((kind, key));
            } else {
                self.decorations.remove(&(kind, key));
            }
        }
        reply(r)
    }

    fn set_decorations(
        &mut self,
        kind: String,
        items: Vec<(u64, wit::ui::Decoration)>,
    ) -> UiResult {
        self.require(Capability::Ui)?;
        if !DECORATION_KINDS.iter().any(|(k, _)| *k == kind) {
            return err(UiError::Unavailable);
        }
        let others = self.decorations.iter().filter(|(k, _)| *k != kind).count();
        if others + items.len() > MAX_DECORATIONS {
            return err(UiError::Limit);
        }
        let items: Vec<(u64, model::Decoration)> = items
            .into_iter()
            .map(|(k, d)| (k, convert::decoration_from_wit(d)))
            .collect();
        if items.iter().any(|(_, d)| {
            d.image
                .as_deref()
                .is_some_and(|i| !self.image_registered(i))
        }) {
            return err(UiError::Invalid);
        }
        let keys: Vec<u64> = items.iter().map(|(k, _)| *k).collect();
        let r = self.ui_op(UiOp::SetDecorations {
            kind: kind.clone(),
            items,
        });
        if r.is_ok() {
            self.decorations.retain(|(k, _)| *k != kind);
            self.decorations
                .extend(keys.into_iter().map(|key| (kind.clone(), key)));
        }
        reply(r)
    }

    fn clear_decorations(&mut self, kind: String) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        self.decorations.retain(|(k, _)| *k != kind);
        let _ = self.ui_op(UiOp::ClearDecorations { kind });
        Ok(())
    }

    fn set_message_accessory(
        &mut self,
        message: api::MessageRef,
        elements: Option<Vec<wit::types::Element>>,
    ) -> UiResult {
        self.require(Capability::Ui)?;
        let elements = elements.map(|e| {
            convert::elements_from_wit(e)
                .into_iter()
                .take(64)
                .collect::<Vec<_>>()
        });
        if elements.is_some()
            && !self.accessories.contains(&message.id)
            && self.accessories.len() >= MAX_ACCESSORIES
        {
            return err(UiError::Limit);
        }
        let set = elements.is_some();
        let r = self.ui_op(UiOp::SetMessageAccessory { message, elements });
        if r.is_ok() {
            if set {
                self.accessories.insert(message.id);
            } else {
                self.accessories.remove(&message.id);
            }
        }
        reply(r)
    }

    fn set_ghost_message(&mut self, ghost: wit::ui::GhostMessage) -> UiResult {
        self.require(Capability::Ui)?;
        if !self.grants.has(Capability::ReadMessages) {
            return err(UiError::NotGranted);
        }
        if !matches!(ghost.style.as_str(), "deleted" | "edited" | "note") {
            return err(UiError::Invalid);
        }
        if self
            .ghosts
            .get(&ghost.channel)
            .is_some_and(|ids| !ids.contains(&ghost.id) && ids.len() >= MAX_GHOSTS_PER_CHANNEL)
        {
            return err(UiError::Limit);
        }
        let g = model::GhostMessage {
            channel: ghost.channel,
            id: ghost.id,
            author: ghost.author,
            content: ghost
                .content
                .into_iter()
                .take(200)
                .map(|s| convert::span_from_wit(s, false))
                .collect(),
            style: ghost.style,
        };
        let (ch, id) = (g.channel, g.id);
        let r = self.ui_op(UiOp::SetGhostMessage { ghost: g });
        if r.is_ok() {
            self.ghosts.entry(ch).or_default().insert(id);
        }
        reply(r)
    }

    fn remove_ghost_message(&mut self, channel: u64, id: u64) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        if let Some(ids) = self.ghosts.get_mut(&channel) {
            ids.remove(&id);
        }
        let _ = self.ui_op(UiOp::RemoveGhostMessage { channel, id });
        Ok(())
    }

    fn set_dm_sections(&mut self, sections: Vec<wit::ui::DmSection>) -> UiResult {
        self.require(Capability::Ui)?;
        if sections.len() > 20
            || sections
                .iter()
                .any(|s| s.channels.len() > 100 || !valid_id(&s.id) || clean(&s.name, 48).is_none())
        {
            return err(UiError::Invalid);
        }
        let sections = sections
            .into_iter()
            .map(|s| model::DmSection {
                id: s.id,
                name: clean(&s.name, 48).unwrap_or_default(),
                color: s.color.map(convert::color_from_wit),
                channels: s.channels,
                collapsed: s.collapsed,
            })
            .collect();
        reply(self.ui_op(UiOp::SetDmSections { sections }))
    }

    fn show_toast(&mut self, text: String, kind: wit::ui::ToastKind) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        let now = self.now();
        if !self.rates.toast.take(now, TOAST_BURST, TOAST_WINDOW_MS) {
            self.warn("toast dropped (rate limit)");
            return Ok(());
        }
        let text: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
        let kind = convert::toast_from_wit(kind);
        self.backend
            .lock()
            .unwrap()
            .show_toast(&self.id, &text, kind);
        Ok(())
    }

    fn show_notice(&mut self, notice: wit::ui::Notice) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        let Some(text) = clean(&notice.text, 200) else {
            return Ok(());
        };
        let notice = model::Notice {
            text,
            kind: convert::toast_from_wit(notice.kind),
            button: notice.button.and_then(|b| clean(&b, 32)),
        };
        let _ = self.ui_op(UiOp::ShowNotice { notice });
        Ok(())
    }

    fn dismiss_notice(&mut self) -> wasmtime::Result<()> {
        self.require(Capability::Ui)?;
        let _ = self.ui_op(UiOp::DismissNotice);
        Ok(())
    }

    fn navigate(&mut self, to: api::Target) -> UiResult {
        self.require(Capability::Ui)?;
        if !self.navigation_ok() {
            return err(UiError::NoGesture);
        }
        self.ledger.action("navigate");
        let r = self
            .backend
            .lock()
            .unwrap()
            .perform(&self.id, Action::Navigate { to });
        Ok(r.map_err(|e| match e {
            model::ActionError::NotFound => wit::ui::UiError::NotFound,
            model::ActionError::NoGesture => wit::ui::UiError::NoGesture,
            _ => wit::ui::UiError::Unavailable,
        }))
    }

    fn open_url(&mut self, url: String) -> UiResult {
        self.require(Capability::Ui)?;
        if !valid_url(&url) {
            return err(UiError::Invalid);
        }
        if !self.navigation_ok() {
            return err(UiError::NoGesture);
        }
        self.ledger.action("open-url");
        let r = self
            .backend
            .lock()
            .unwrap()
            .perform(&self.id, Action::OpenUrl { url });
        Ok(r.map_err(|_| wit::ui::UiError::Unavailable))
    }

    fn open_media(&mut self, url: String) -> UiResult {
        self.require(Capability::Ui)?;
        if !valid_url(&url) {
            return err(UiError::Invalid);
        }
        if !self.navigation_ok() {
            return err(UiError::NoGesture);
        }
        self.ledger.action("open-media");
        let r = self
            .backend
            .lock()
            .unwrap()
            .perform(&self.id, Action::OpenMedia { url });
        Ok(r.map_err(|_| wit::ui::UiError::Unavailable))
    }
}

/// An `https` URL of at most 2048 bytes, with no whitespace or control
/// characters.
fn valid_url(url: &str) -> bool {
    url.starts_with("https://")
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

impl wit::render_control::Host for PluginState {
    fn declare_actions(&mut self, ids: Vec<String>) -> wasmtime::Result<()> {
        self.require(Capability::RenderMessages)?;
        let ids: std::collections::BTreeSet<String> =
            ids.into_iter().filter(|i| valid_id(i)).take(64).collect();
        self.shared.lock().unwrap().declared_actions = ids;
        Ok(())
    }

    fn invalidate(&mut self, channel: Option<u64>) -> wasmtime::Result<()> {
        self.require(Capability::RenderMessages)?;
        self.pending_invalidate.push(channel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybind_notation() {
        assert!(valid_keybind("Ctrl+Shift+K"));
        assert!(valid_keybind("Alt+F4"));
        assert!(!valid_keybind("K"), "needs a modifier");
        assert!(!valid_keybind("Ctrl+"));
        assert!(!valid_keybind("Hyper+K"));
    }

    #[test]
    fn urls_opened_for_a_plugin_are_plain_https() {
        assert!(valid_url("https://cdn.example.com/a.png"));
        assert!(!valid_url("http://cdn.example.com/a.png"));
        assert!(!valid_url("https://cdn.example.com/a b.png"));
        assert!(!valid_url("https://cdn.example.com/a\u{7}.png"));
        assert!(!valid_url(&format!("https://{}", "a".repeat(2048))));
    }
}
