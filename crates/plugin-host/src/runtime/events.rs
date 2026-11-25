//! Events: Discord's events run past the `event-filter`s before the client
//! processes them, and every event reaches the `event-handler`s after.
//! Timers fire from here too.

use super::state::{self, TimerEntry};
use super::{Runtime, Status, convert};
use crate::api;
use crate::bindings::ex;
use crate::model;

/// The kind an event is subscribed and intercepted by (`None` for `dropped`).
pub fn event_kind(ev: &api::Event) -> Option<api::EventKind> {
    convert::event_kind(ev)
}

/// The kinds an `event-filter` may see: events that arrive from Discord.
/// The client's own (selection, window, the user's own state, the
/// composer) and `raw` can't be intercepted.
pub const INTERCEPTABLE: [api::EventKind; 13] = {
    use api::EventKind as K;
    [
        K::Message,
        K::Reaction,
        K::PollVote,
        K::Pins,
        K::Typing,
        K::Presence,
        K::VoiceState,
        K::Member,
        K::Relationship,
        K::Guild,
        K::Channel,
        K::Role,
        K::Emoji,
    ]
};

pub(super) fn interceptable(kind: api::EventKind) -> bool {
    INTERCEPTABLE.contains(&kind)
}

impl Runtime {
    /// Run an event from Discord past every `event-filter` registered for
    /// its kind, before the client processes it. Plugins see it in load
    /// order, each getting what the one before returned. `None`: a plugin
    /// dropped it, and the client must not process it.
    ///
    /// Filters run on the client's hot path, so each call gets the
    /// per-render budget; running out traps the plugin like any other trap,
    /// and the event passes.
    pub fn intercept(&mut self, ev: api::Event) -> Option<api::Event> {
        let Some(kind) = convert::event_kind(&ev).filter(|k| interceptable(*k)) else {
            return Some(ev);
        };
        let mut ev = ev;
        for idx in 0..self.plugins.len() {
            if !self.intercepts(idx, kind) {
                continue;
            }
            let fuel = self.config.limits.fuel_per_render;
            let verdict = self.call_with_fuel(idx, fuel, |h, store| match &h.filter {
                Some(f) => f.call_on_intercept(store, &ev),
                None => Ok(ex::event_filter::Verdict::Pass),
            });
            match verdict {
                Some(ex::event_filter::Verdict::Drop) => return None,
                Some(ex::event_filter::Verdict::Replace(new)) => {
                    if convert::event_kind(&new) == Some(kind) {
                        ev = new;
                    } else {
                        let id = self.plugins[idx].manifest.id.clone();
                        let msg =
                            "event-filter returned a different kind of event; passing the original"
                                .to_string();
                        self.host_log(&id, model::LogLevel::Warn, &msg);
                    }
                }
                Some(ex::event_filter::Verdict::Patch(patches)) => {
                    if apply_patches(&mut ev, &patches) > 0 {
                        let id = self.plugins[idx].manifest.id.clone();
                        let msg = "event-filter patched fields this kind of event doesn't have, or \
                                   text that isn't allowed; those were ignored";
                        self.host_log(&id, model::LogLevel::Warn, msg);
                    }
                }
                Some(ex::event_filter::Verdict::Pass) | None => {}
            }
        }
        Some(ev)
    }

    /// Whether any active plugin's `event-filter` sees events of `kind`. A
    /// backend checks this before building the event to show it.
    pub fn filters_kind(&self, kind: api::EventKind) -> bool {
        interceptable(kind) && (0..self.plugins.len()).any(|idx| self.intercepts(idx, kind))
    }

    /// The kinds some active plugin's `event-filter` sees.
    pub fn intercepted_kinds(&self) -> Vec<api::EventKind> {
        INTERCEPTABLE
            .into_iter()
            .filter(|k| self.filters_kind(*k))
            .collect()
    }

    /// Whether plugin `idx` is active and its `event-filter` sees `kind`.
    fn intercepts(&self, idx: usize, kind: api::EventKind) -> bool {
        let p = &self.plugins[idx];
        p.status == Status::Active
            && p.hooks.filter.is_some()
            && p.store.data().intercepts.contains(&(kind as u8))
    }

    /// Deliver a client event to every active plugin that may see it and is
    /// subscribed to its kind.
    pub fn dispatch_event(&mut self, ev: &api::Event) {
        self.dispatch_events(std::slice::from_ref(ev));
    }

    /// Deliver a batch (one host tick) of events: each plugin gets one
    /// `on-events` call with the events it may see, in order.
    pub fn dispatch_events(&mut self, events: &[api::Event]) {
        for idx in 0..self.plugins.len() {
            let p = &self.plugins[idx];
            if p.status != Status::Active || p.hooks.events.is_none() {
                continue;
            }
            let st = p.store.data();
            let batch: Vec<api::Event> = events
                .iter()
                .filter(|e| {
                    let kind = convert::event_kind(e);
                    let cap_ok = kind.is_none_or(|k| st.grants.has(convert::kind_capability(k)));
                    let subscribed = kind.is_none()
                        || st
                            .subscriptions
                            .as_ref()
                            .is_none_or(|s| kind.is_some_and(|k| s.contains(&(k as u8))));
                    cap_ok && subscribed
                })
                .cloned()
                .collect();
            if batch.is_empty() {
                continue;
            }
            let limit = self.config.limits.event_time;
            self.plugins[idx].store.data_mut().wall = Some(state::WallClock::new(limit));
            self.call(idx, |h, store| match &h.events {
                Some(e) => e.call_on_events(store, &batch),
                None => Ok(()),
            });
            self.plugins[idx].store.data_mut().wall = None;
        }
    }

    /// Deliver a cursor position to every active plugin granted `pointer`.
    /// Hosts throttle before calling; `pointer-moved` is an ordinary event.
    pub fn dispatch_pointer(&mut self, x: f32, y: f32) {
        self.dispatch_event(&api::Event::PointerMoved((x, y)));
    }

    /// Fire every timer whose deadline has passed, calling `on-timer`. Repeat
    /// timers are rescheduled; one-shots are removed. Drive this from the host's
    /// frame/tick loop (`rekindle dev` does, on its tick).
    pub fn fire_due_timers(&mut self) {
        if self.ledger_flushed.elapsed() >= super::LEDGER_FLUSH {
            self.flush_activity();
        }
        let now = self.start.elapsed().as_millis() as u64;
        for idx in 0..self.plugins.len() {
            if self.plugins[idx].status != Status::Active {
                continue;
            }
            let mut due: Vec<String> = Vec::new();
            {
                let timers = &mut self.plugins[idx].store.data_mut().timers;
                for (id, e) in timers.iter() {
                    if e.next_ms <= now {
                        due.push(id.clone());
                    }
                }
                for id in &due {
                    match timers.get(id).and_then(|e| e.interval) {
                        Some(iv) => {
                            timers.insert(
                                id.clone(),
                                TimerEntry {
                                    next_ms: now.saturating_add(iv),
                                    interval: Some(iv),
                                },
                            );
                        }
                        None => {
                            timers.remove(id);
                        }
                    }
                }
            }
            for id in due {
                self.call(idx, |h, store| match &h.timer {
                    Some(t) => t.call_on_timer(store, &id),
                    None => Ok(()),
                });
            }
        }
    }
}

/// Apply an `event-filter`'s patches to `ev`. Returns how many couldn't be
/// applied: the kind has no such field, or the text failed the compose
/// checks.
fn apply_patches(ev: &mut api::Event, patches: &[api::EventPatch]) -> usize {
    let (api::Event::MessageCreated(m) | api::Event::MessageEdited((m, _))) = ev else {
        return patches.len();
    };
    let mut ignored = 0;
    for p in patches {
        match p {
            api::EventPatch::Content(c) => {
                if super::compose::check_compose(c).is_none() {
                    m.content.clone_from(c);
                } else {
                    ignored += 1;
                }
            }
            api::EventPatch::FlagBits(f) => m.flag_bits = *f,
            api::EventPatch::RemoveEmbeds => m.embeds.clear(),
            api::EventPatch::RemoveAttachments => m.attachments.clear(),
            api::EventPatch::RemoveStickers => m.stickers.clear(),
        }
    }
    ignored
}
