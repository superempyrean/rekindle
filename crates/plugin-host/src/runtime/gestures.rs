//! The user's own gestures, delivered to the plugin that owns what was
//! clicked or typed: panels, slot items, commands, keybinds, slash commands
//! and message clicks. Gesture-bound host functions work only while one is
//! being delivered (design doc §5.1).

use anyhow::{Result, bail};
use wasmtime::Store;

use super::state::{self, PluginState};
use super::{Hooks, Runtime, Status, convert};
use crate::api;
use crate::backend::ClientBackend;
use crate::capability::Capability;
use crate::model::{self, ClickGesture, Mods};

impl Runtime {
    /// Route a panel / modal / settings-page / notice / accessory interaction
    /// to the plugin that owns the surface. A gesture.
    pub fn ui_event(&mut self, plugin: &str, ev: &model::UiEvent) -> Result<()> {
        self.ui_event_with(plugin, ev, Mods::default())
    }

    pub fn ui_event_with(&mut self, plugin: &str, ev: &model::UiEvent, mods: Mods) -> Result<()> {
        let idx = self.active(plugin)?;
        let wit_ev = convert::ui_event_to_wit(ev);
        self.call_gesture(idx, mods, |h, store| match &h.ui {
            Some(u) => u.call_on_ui(store, &wit_ev),
            None => Ok(()),
        });
        Ok(())
    }

    /// Deliver a slot interaction to the plugin that owns the item. Clicks
    /// (`clicked`, `decoration-clicked`, `message-clicked`) are gestures.
    pub fn slot_event(&mut self, plugin: &str, ev: &api::SlotEvent) -> Result<()> {
        let idx = self.active(plugin)?;
        let mods = match ev {
            api::SlotEvent::Clicked(c) => convert::mods_from_wit(c.modifiers),
            _ => Mods::default(),
        };
        let f = |h: &Hooks, store: &mut Store<PluginState>| match &h.ui {
            Some(u) => u.call_on_slot(store, ev),
            None => Ok(()),
        };
        if matches!(ev, api::SlotEvent::ProfileOpened(_)) {
            self.call(idx, f);
        } else {
            self.call_gesture(idx, mods, f);
        }
        Ok(())
    }

    /// Convenience: a click on one of `plugin`'s slot items.
    pub fn slot_click(
        &mut self,
        plugin: &str,
        slot: &str,
        item: &str,
        target: api::Target,
        mods: Mods,
    ) -> Result<()> {
        let ev = api::SlotEvent::Clicked(api::SlotClick {
            slot: slot.into(),
            item: item.into(),
            target,
            modifiers: convert::mods_to_wit(mods),
        });
        self.slot_event(plugin, &ev)
    }

    /// The user opened a profile: ask every active plugin with a
    /// `profile.section` item to fill it for this user.
    pub fn open_profile(&mut self, user: &api::UserRef) {
        for idx in 0..self.plugins.len() {
            let p = &self.plugins[idx];
            if p.status != Status::Active {
                continue;
            }
            let sections: Vec<String> = p
                .store
                .data()
                .slot_items
                .iter()
                .filter(|(s, _)| s == model::SLOT_PROFILE_SECTION)
                .map(|(_, id)| id.clone())
                .collect();
            for section in sections {
                let ev = api::SlotEvent::ProfileOpened((section, user.clone()));
                self.call(idx, |h, store| match &h.ui {
                    Some(u) => u.call_on_slot(store, &ev),
                    None => Ok(()),
                });
            }
        }
    }

    /// Run a palette command registered by `plugin`. A gesture. Fails if
    /// `plugin` isn't active or never registered `id`.
    pub fn run_command(&mut self, plugin: &str, id: &str) -> Result<()> {
        let idx = self.active(plugin)?;
        if !self.plugins[idx].store.data().commands.contains(id) {
            bail!("plugin `{plugin}` has no command `{id}`");
        }
        self.call_gesture(idx, Mods::default(), |h, store| match &h.ui {
            Some(u) => u.call_on_command(store, id),
            None => Ok(()),
        });
        Ok(())
    }

    /// A plugin keybind fired (the client window has focus). A gesture.
    pub fn keybind(&mut self, plugin: &str, id: &str) -> Result<()> {
        let idx = self.active(plugin)?;
        if !self.plugins[idx].store.data().keybinds.contains(id) {
            bail!("plugin `{plugin}` has no keybind `{id}`");
        }
        self.call_gesture(idx, Mods::default(), |h, store| match &h.ui {
            Some(u) => u.call_on_keybind(store, id),
            None => Ok(()),
        });
        Ok(())
    }

    /// The user ran one of `plugin`'s slash commands. A gesture. A `send`
    /// reply from a plugin without `compose` becomes `insert`; the client
    /// runs a `send` through [`Self::compose`] like any send.
    pub fn slash_command(
        &mut self,
        plugin: &str,
        inv: &api::Invocation,
    ) -> Result<api::CommandReply> {
        let idx = self.active(plugin)?;
        if !self.plugins[idx]
            .store
            .data()
            .slash_commands
            .contains(&inv.command)
        {
            bail!("plugin `{plugin}` has no slash command `/{}`", inv.command);
        }
        let reply = self
            .call_gesture(idx, Mods::default(), |h, store| match &h.ui {
                Some(u) => u.call_on_slash_command(store, inv),
                None => Ok(api::CommandReply::None),
            })
            .unwrap_or(api::CommandReply::None);
        Ok(match reply {
            api::CommandReply::Send(text) if !self.plugins[idx].grants.has(Capability::Compose) => {
                self.host_log(plugin, model::LogLevel::Warn, "slash command tried to send without `compose`; inserted into the composer instead");
                api::CommandReply::Insert(text)
            }
            api::CommandReply::Local(spans) => {
                api::CommandReply::Local(spans.into_iter().take(200).collect())
            }
            other => other,
        })
    }

    /// Suggestions for an `autocomplete` option, on the render budget.
    pub fn autocomplete(
        &mut self,
        plugin: &str,
        command: &str,
        option: &str,
        partial: &str,
    ) -> Result<Vec<(String, String)>> {
        let idx = self.active(plugin)?;
        let fuel = self.config.limits.fuel_per_render;
        let out = self
            .call_with_fuel(idx, fuel, |h, store| match &h.ui {
                Some(u) => u.call_on_autocomplete(store, command, option, partial),
                None => Ok(vec![]),
            })
            .unwrap_or_default();
        Ok(out
            .into_iter()
            .filter_map(|(l, v)| Some((state::clean(&l, 100)?, state::clean(&v, 100)?)))
            .take(25)
            .collect())
    }

    /// A message click (`area`, click count, modifiers) the client doesn't
    /// use itself: deliver it to the plugin that owns that gesture. Returns
    /// the plugin it went to, if any. A gesture.
    pub fn message_click(
        &mut self,
        gesture: ClickGesture,
        message: &api::MessageRef,
    ) -> Option<String> {
        let (idx, binding) = self.click_owner(gesture, message)?;
        let ev = api::SlotEvent::MessageClicked((binding, *message));
        self.call_gesture(idx, gesture.mods, |h, store| match &h.ui {
            Some(u) => u.call_on_slot(store, &ev),
            None => Ok(()),
        });
        Some(self.plugins[idx].manifest.id.clone())
    }

    /// The first binding of `gesture` whose `when` predicates hold for
    /// `message`, trying active plugins in the order they were enabled: the
    /// plugin's index and the binding's id.
    fn click_owner(
        &self,
        gesture: ClickGesture,
        message: &api::MessageRef,
    ) -> Option<(usize, String)> {
        let b = self.backend.lock().unwrap();
        self.plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.status == Status::Active)
            .find_map(|(idx, p)| {
                p.store
                    .data()
                    .click_bindings
                    .values()
                    .find(|c| {
                        c.gesture == gesture
                            && c.when.iter().all(|w| message_predicate(&*b, *w, message))
                    })
                    .map(|c| (idx, c.id.clone()))
            })
    }
}

/// Evaluate a menu/click predicate against a message, from what the client
/// holds (the host's job, so menus open without calling into plugins).
pub fn message_predicate(
    b: &dyn ClientBackend,
    p: model::MenuPredicate,
    m: &api::MessageRef,
) -> bool {
    use model::MenuPredicate as P;
    const ADD_REACTIONS: u64 = 1 << 6;
    const SEND_MESSAGES: u64 = 1 << 11;
    const MANAGE_MESSAGES: u64 = 1 << 13;
    const PIN_MESSAGES: u64 = 1 << 51;
    let me = b.me().map(|u| u.id);
    let own = me == Some(m.author);
    let perms = || b.my_permissions(m.channel);
    let loaded = || b.message(m.channel, m.id);
    match p {
        P::OwnMessage | P::TargetIsSelf | P::CanEdit => own,
        P::OthersMessage => !own,
        P::HasAttachments => loaded().is_some_and(|x| !x.attachments.is_empty()),
        P::HasEmbeds => loaded().is_some_and(|x| !x.embeds.is_empty()),
        P::HasContent => loaded().is_some_and(|x| !x.content.trim().is_empty()),
        P::InGuild => m.guild.is_some(),
        P::InDm => m.guild.is_none(),
        P::TargetIsBot => b.user(m.author).is_some_and(|u| u.bot),
        P::TargetIsFriend => b
            .relationships()
            .iter()
            .any(|r| r.user == m.author && matches!(r.kind, api::RelationshipKind::Friend)),
        P::CanManageMessages => perms() & MANAGE_MESSAGES != 0,
        P::CanPin => perms() & (PIN_MESSAGES | MANAGE_MESSAGES) != 0,
        P::CanReply => perms() & SEND_MESSAGES != 0,
        P::CanReact => perms() & ADD_REACTIONS != 0,
        P::CanDelete => own || perms() & MANAGE_MESSAGES != 0,
    }
}
