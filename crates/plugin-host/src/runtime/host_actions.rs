//! `local-actions`, `reactions`, `actions`, `presence`: acting as the user,
//! only while a gesture is being delivered and within its budget (design doc
//! §5). Timers, events, renders and `activate` are never gestures, so none of
//! this works from them, except your own activity: rich presence and
//! custom status may be set at any time, rate-limited (owner decision
//! 2026-11-07), the way the client sets "playing" itself.

use crate::api;
use crate::bindings::wit;
use crate::capability::Capability;
use crate::model::{Action, ActionError, ActionValue};

use super::convert::action_error_to_wit;
use super::state::{Budget, PluginState, clean};

type ActResult = wasmtime::Result<Result<(), wit::action_types::ActionError>>;

/// `actions.send-message`: files per message, and their bytes in all. The
/// `compose-uploads` hook takes files up to the same size. Fixed for now. Discord's real upload limit depends on the user's Nitro
/// and the server's boosts, so with a real client the host should read the
/// limit from the client.
pub(super) const MAX_SEND_FILES: usize = 10;
pub(super) const MAX_SEND_BYTES: usize = 25 * 1024 * 1024;

/// Rich presence / custom status changes allowed per window.
pub(super) const ACTIVITY_MAX: usize = 5;
pub(super) const ACTIVITY_WINDOW_MS: u64 = 20_000;

impl PluginState {
    /// Check budget and rate, then hand the action to the backend.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "returns the host function's own type, so a call site is a plain `return`"
    )]
    fn act(&mut self, budget: Budget, action: Action) -> ActResult {
        if !self.spend(budget) {
            return Ok(Err(action_error_to_wit(ActionError::NoGesture)));
        }
        let now = self.now();
        let rate_ok = match budget {
            Budget::React => self.rates.react.take(now, 10, 10_000),
            Budget::Outward => self.rates.outward.take(now, 5, 10_000),
            Budget::Presence => self.rates.status.take(now, 5, 20_000),
            _ => true,
        };
        if !rate_ok {
            return Ok(Err(action_error_to_wit(ActionError::RateLimited)));
        }
        let r = self.backend.lock().unwrap().perform(&self.id, action);
        Ok(r.map_err(action_error_to_wit))
    }

    /// Your own activity (rich presence, custom status): no gesture needed,
    /// at most [`ACTIVITY_MAX`] changes per [`ACTIVITY_WINDOW_MS`].
    #[expect(
        clippy::unnecessary_wraps,
        reason = "returns the host function's own type, so a call site is a plain `return`"
    )]
    fn set_activity(&mut self, action: Action) -> ActResult {
        let now = self.now();
        if !self
            .rates
            .activity
            .take(now, ACTIVITY_MAX, ACTIVITY_WINDOW_MS)
        {
            return Ok(Err(action_error_to_wit(ActionError::RateLimited)));
        }
        let r = self.backend.lock().unwrap().perform(&self.id, action);
        Ok(r.map_err(action_error_to_wit))
    }

    /// [`Self::act`] for an action that returns something, mapped by `pick`
    /// to the host function's return value.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "returns the host function's own type, so a call site is a plain `return`"
    )]
    fn act_value<T>(
        &mut self,
        budget: Budget,
        action: Action,
        pick: impl FnOnce(ActionValue) -> Option<T>,
    ) -> wasmtime::Result<Result<T, wit::action_types::ActionError>> {
        if !self.spend(budget) {
            return Ok(Err(action_error_to_wit(ActionError::NoGesture)));
        }
        let now = self.now();
        if matches!(budget, Budget::Outward) && !self.rates.outward.take(now, 5, 10_000) {
            return Ok(Err(action_error_to_wit(ActionError::RateLimited)));
        }
        let r = self.backend.lock().unwrap().perform_value(&self.id, action);
        Ok(match r {
            Ok(v) => pick(v).ok_or(action_error_to_wit(ActionError::Failed)),
            Err(e) => Err(action_error_to_wit(e)),
        })
    }

    /// Send this plugin's `view` overrides to the backend.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "returns the host function's own type, so a call site is a plain `return`"
    )]
    fn push_view(&mut self) -> ActResult {
        let ok = self
            .backend
            .lock()
            .unwrap()
            .set_view(&self.id, self.view.clone());
        Ok(if ok {
            Ok(())
        } else {
            Err(action_error_to_wit(ActionError::Unavailable))
        })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "returns the host function's own type, so a call site is a plain `return`"
    )]
    fn invalid() -> ActResult {
        Ok(Err(action_error_to_wit(ActionError::Invalid)))
    }
}

impl wit::local_actions::Host for PluginState {
    fn mark_read(&mut self, channel: u64) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::MarkRead { channel })
    }

    fn mark_guild_read(&mut self, guild: u64) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::MarkGuildRead { guild })
    }

    fn mark_all_read(&mut self) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::MarkAllRead)
    }

    fn mark_unread(&mut self, message: api::MessageRef) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::MarkUnread { message })
    }

    fn start_reply(&mut self, message: api::MessageRef, mention: bool) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::StartReply { message, mention })
    }

    fn start_edit(&mut self, message: api::MessageRef) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::StartEdit { message })
    }

    fn insert_text(&mut self, text: String) -> ActResult {
        self.require(Capability::ActLocal)?;
        if text.is_empty()
            || text.chars().count() > 4000
            || text.chars().any(|c| c.is_control() && c != '\n')
        {
            return Self::invalid();
        }
        self.act(Budget::Local, Action::InsertText { text })
    }

    fn open_dm(
        &mut self,
        user: u64,
    ) -> wasmtime::Result<Result<u64, wit::action_types::ActionError>> {
        self.require(Capability::ActLocal)?;
        self.act_value(Budget::Local, Action::OpenDm { user }, |v| match v {
            ActionValue::Id(id) => Some(id),
            _ => None,
        })
    }

    fn set_user_volume(&mut self, user: u64, percent: f32) -> ActResult {
        self.require(Capability::ActLocal)?;
        if !percent.is_finite() || !(0.0..=200.0).contains(&percent) {
            return Self::invalid();
        }
        self.act(Budget::Local, Action::SetUserVolume { user, percent })
    }

    fn set_user_local_mute(&mut self, user: u64, muted: bool) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::SetUserLocalMute { user, muted })
    }

    fn set_note(&mut self, user: u64, note: String) -> ActResult {
        self.require(Capability::ActLocal)?;
        if note.chars().count() > 256 {
            return Self::invalid();
        }
        self.act(Budget::Local, Action::SetNote { user, note })
    }

    fn save_message(&mut self, message: api::MessageRef, saved: bool) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(Budget::Local, Action::SaveMessage { message, saved })
    }

    fn set_category_collapsed(&mut self, category: u64, collapsed: bool) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(
            Budget::Local,
            Action::SetCategoryCollapsed {
                category,
                collapsed,
            },
        )
    }

    fn set_muted(&mut self, channel_or_guild: u64, muted: bool) -> ActResult {
        self.require(Capability::ActLocal)?;
        self.act(
            Budget::Local,
            Action::SetMuted {
                target: channel_or_guild,
                muted,
            },
        )
    }
}

/// Unicode emoji (one grapheme-ish, ≤ 16 bytes) or `name:id`.
fn valid_emoji(e: &str) -> bool {
    if let Some((name, id)) = e.split_once(':') {
        return !name.is_empty()
            && name.len() <= 32
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && id.parse::<u64>().is_ok();
    }
    !e.is_empty()
        && e.len() <= 32
        && !e
            .chars()
            .any(|c| c.is_ascii() && !c.is_ascii_digit() && c != '#' && c != '*')
}

impl wit::reactions::Host for PluginState {
    fn react(&mut self, message: api::MessageRef, emoji: String, burst: bool) -> ActResult {
        self.require(Capability::React)?;
        if !valid_emoji(&emoji) {
            return Self::invalid();
        }
        self.act(
            Budget::React,
            Action::React {
                message,
                emoji,
                burst,
            },
        )
    }

    fn unreact(&mut self, message: api::MessageRef, emoji: String, burst: bool) -> ActResult {
        self.require(Capability::React)?;
        if !valid_emoji(&emoji) {
            return Self::invalid();
        }
        self.act(
            Budget::React,
            Action::Unreact {
                message,
                emoji,
                burst,
            },
        )
    }
}

impl wit::actions::Host for PluginState {
    fn set_pinned(&mut self, message: api::MessageRef, pinned: bool) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Outward, Action::SetPinned { message, pinned })
    }

    fn delete_message(&mut self, message: api::MessageRef) -> ActResult {
        self.require(Capability::Act)?;
        // The client's own rule: Shift skips the confirmation.
        let confirm = !self.gesture.is_some_and(|g| g.mods.shift);
        self.act(Budget::Outward, Action::DeleteMessage { message, confirm })
    }

    fn edit_message(&mut self, message: api::MessageRef, content: String) -> ActResult {
        self.require(Capability::Act)?;
        if super::compose::check_compose(&content).is_some() {
            return Self::invalid();
        }
        self.act(Budget::Outward, Action::EditMessage { message, content })
    }

    fn suppress_embeds(&mut self, message: api::MessageRef) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Outward, Action::SuppressEmbeds { message })
    }

    fn vote(&mut self, message: api::MessageRef, answers: Vec<u32>) -> ActResult {
        self.require(Capability::Act)?;
        if answers.len() > 10 {
            return Self::invalid();
        }
        self.act(Budget::Outward, Action::Vote { message, answers })
    }

    fn join_voice(&mut self, channel: Option<u64>) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Presence, Action::JoinVoice { channel })
    }

    fn set_self_mute(&mut self, muted: bool) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Presence, Action::SetSelfMute { muted })
    }

    fn set_self_deaf(&mut self, deafened: bool) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Presence, Action::SetSelfDeaf { deafened })
    }

    fn set_thread_joined(&mut self, thread: u64, joined: bool) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Outward, Action::SetThreadJoined { thread, joined })
    }

    fn forward_message(&mut self, message: api::MessageRef, to_channel: u64) -> ActResult {
        self.require(Capability::Act)?;
        self.act(
            Budget::Outward,
            Action::ForwardMessage {
                message,
                to_channel,
            },
        )
    }

    fn set_typing(&mut self, channel: u64) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Outward, Action::SetTyping { channel })
    }

    fn set_friend(&mut self, user: u64, add: bool) -> ActResult {
        self.require(Capability::Act)?;
        self.act(Budget::Outward, Action::SetFriend { user, add })
    }

    fn create_invite(
        &mut self,
        channel: u64,
        max_age_secs: u32,
        max_uses: u32,
    ) -> wasmtime::Result<Result<String, wit::action_types::ActionError>> {
        self.require(Capability::Act)?;
        // Discord's own limits: a week, and 100 uses.
        if max_age_secs > 604_800 || max_uses > 100 {
            return Ok(Err(action_error_to_wit(ActionError::Invalid)));
        }
        let action = Action::CreateInvite {
            channel,
            max_age_secs,
            max_uses,
        };
        self.act_value(Budget::Outward, action, |v| match v {
            ActionValue::Code(c) => Some(c),
            _ => None,
        })
    }

    fn join_guild(
        &mut self,
        invite_code: String,
    ) -> wasmtime::Result<Result<u64, wit::action_types::ActionError>> {
        self.require(Capability::Act)?;
        let ok = !invite_code.is_empty()
            && invite_code.len() <= 64
            && invite_code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return Ok(Err(action_error_to_wit(ActionError::Invalid)));
        }
        self.act_value(
            Budget::Outward,
            Action::JoinGuild { invite_code },
            |v| match v {
                ActionValue::Id(id) => Some(id),
                _ => None,
            },
        )
    }

    fn send_message(
        &mut self,
        channel: u64,
        content: String,
        files: Vec<wit::action_types::OutgoingFile>,
    ) -> ActResult {
        self.require(Capability::Act)?;
        let total: usize = files.iter().map(|f| f.data.len()).sum();
        let text_ok = (content.is_empty() && !files.is_empty())
            || super::compose::check_compose(&content).is_none();
        let names_ok = files.iter().all(|f| {
            clean(&f.filename, 200).is_some_and(|n| n == f.filename) && !f.data.is_empty()
        });
        if !text_ok || files.len() > MAX_SEND_FILES || total > MAX_SEND_BYTES || !names_ok {
            return Self::invalid();
        }
        let files = files
            .into_iter()
            .map(|f| crate::model::OutgoingFile {
                size: f.data.len() as u64,
                filename: f.filename,
                content_type: f.content_type,
                data: f.data,
                spoiler: f.spoiler,
            })
            .collect();
        self.act(
            Budget::Outward,
            Action::SendMessage {
                channel,
                content,
                files,
            },
        )
    }
}

impl wit::presence::Host for PluginState {
    fn set_status(&mut self, status: String) -> ActResult {
        self.require(Capability::Presence)?;
        if !matches!(status.as_str(), "online" | "idle" | "dnd" | "invisible") {
            return Self::invalid();
        }
        self.act(Budget::Presence, Action::SetStatus { status })
    }

    fn set_custom_status(
        &mut self,
        text: Option<String>,
        emoji: Option<String>,
        expires_ms: Option<u64>,
    ) -> ActResult {
        self.require(Capability::Presence)?;
        if text.as_deref().is_some_and(|t| t.chars().count() > 128)
            || emoji.as_deref().is_some_and(|e| !valid_emoji(e))
        {
            return Self::invalid();
        }
        let text = text.and_then(|t| clean(&t, 128));
        self.set_activity(Action::SetCustomStatus {
            text,
            emoji,
            expires_ms,
        })
    }

    fn set_nickname(&mut self, guild: u64, nick: Option<String>) -> ActResult {
        self.require(Capability::Presence)?;
        if nick
            .as_deref()
            .is_some_and(|n| clean(n, 32).as_deref() != Some(n))
        {
            return Self::invalid();
        }
        self.act(Budget::Presence, Action::SetNickname { guild, nick })
    }

    fn set_rich_presence(&mut self, activity: Option<api::RichPresence>) -> ActResult {
        self.require(Capability::Presence)?;
        if let Some(a) = &activity {
            let ok = matches!(a.kind, 0 | 2 | 3 | 5)
                && clean(&a.name, 128).is_some()
                && a.buttons.len() <= 2
                && a.buttons.iter().all(|(l, u)| {
                    clean(l, 32).is_some() && u.starts_with("https://") && u.len() <= 512
                })
                && [&a.large_image, &a.small_image]
                    .into_iter()
                    .flatten()
                    .all(|i| self.image_registered(i));
            if !ok {
                return Self::invalid();
            }
        }
        self.set_activity(Action::SetRichPresence { activity })
    }
}

/// An audit-log reason: up to 512 characters, no control characters.
fn reason_ok(reason: Option<&str>) -> bool {
    reason.is_none_or(|r| r.chars().count() <= 512 && !r.chars().any(char::is_control))
}

impl wit::moderation::Host for PluginState {
    fn kick(&mut self, guild: u64, user: u64, reason: Option<String>) -> ActResult {
        self.require(Capability::Moderate)?;
        if !reason_ok(reason.as_deref()) {
            return Self::invalid();
        }
        self.act(
            Budget::Outward,
            Action::Kick {
                guild,
                user,
                reason,
            },
        )
    }

    fn ban(
        &mut self,
        guild: u64,
        user: u64,
        delete_message_secs: u32,
        reason: Option<String>,
    ) -> ActResult {
        self.require(Capability::Moderate)?;
        if delete_message_secs > 604_800 || !reason_ok(reason.as_deref()) {
            return Self::invalid();
        }
        self.act(
            Budget::Outward,
            Action::Ban {
                guild,
                user,
                delete_message_secs,
                reason,
            },
        )
    }

    fn unban(&mut self, guild: u64, user: u64) -> ActResult {
        self.require(Capability::Moderate)?;
        self.act(Budget::Outward, Action::Unban { guild, user })
    }

    fn timeout(
        &mut self,
        guild: u64,
        user: u64,
        until_ms: Option<u64>,
        reason: Option<String>,
    ) -> ActResult {
        self.require(Capability::Moderate)?;
        // Discord allows up to 28 days.
        let too_far = until_ms.is_some_and(|t| t > super::timefmt::now_ms() + 28 * 86_400_000);
        if too_far || !reason_ok(reason.as_deref()) {
            return Self::invalid();
        }
        self.act(
            Budget::Outward,
            Action::Timeout {
                guild,
                user,
                until_ms,
                reason,
            },
        )
    }

    fn set_voice_mute(&mut self, guild: u64, user: u64, muted: bool) -> ActResult {
        self.require(Capability::Moderate)?;
        self.act(Budget::Outward, Action::SetVoiceMute { guild, user, muted })
    }

    fn set_voice_deaf(&mut self, guild: u64, user: u64, deafened: bool) -> ActResult {
        self.require(Capability::Moderate)?;
        self.act(
            Budget::Outward,
            Action::SetVoiceDeaf {
                guild,
                user,
                deafened,
            },
        )
    }

    fn move_member(&mut self, guild: u64, user: u64, channel: Option<u64>) -> ActResult {
        self.require(Capability::Moderate)?;
        self.act(
            Budget::Outward,
            Action::MoveMember {
                guild,
                user,
                channel,
            },
        )
    }

    fn bulk_delete(&mut self, channel: u64, messages: Vec<u64>) -> ActResult {
        self.require(Capability::Moderate)?;
        if !(2..=100).contains(&messages.len()) {
            return Self::invalid();
        }
        self.act(Budget::Outward, Action::BulkDelete { channel, messages })
    }
}

/// A short single-line string a view override may set.
fn short(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}

fn user_field_ok(f: &crate::api::UserField) -> bool {
    use crate::api::UserField as F;
    let opt = |s: &Option<String>, max| s.as_deref().is_none_or(|s| short(s, max));
    match f {
        F::Username(s) | F::Discriminator(s) => !s.trim().is_empty() && short(s, 32),
        F::GlobalName(s) | F::Pronouns(s) => opt(s, 40),
        F::Avatar(s) | F::Banner(s) | F::Locale(s) => opt(s, 64),
        F::Bio(s) => s.as_deref().is_none_or(|b| b.chars().count() <= 400),
        F::ThemeColors(c) => c.len() <= 2,
        _ => true,
    }
}

fn member_field_ok(f: &crate::api::MemberField) -> bool {
    use crate::api::MemberField as F;
    match f {
        F::Nick(s) => s.as_deref().is_none_or(|s| short(s, 32)),
        F::Avatar(s) | F::Banner(s) => s.as_deref().is_none_or(|s| short(s, 64)),
        F::Bio(s) => s.as_deref().is_none_or(|b| b.chars().count() <= 400),
        F::Roles(r) => r.len() <= 250,
        _ => true,
    }
}

impl wit::view::Host for PluginState {
    fn set_users(&mut self, who: wit::view::Users, fields: Vec<wit::view::UserField>) -> ActResult {
        self.require(Capability::View)?;
        if fields.len() > 32 || !fields.iter().all(user_field_ok) {
            return Self::invalid();
        }
        for f in fields {
            self.view.set_user(who, f);
        }
        self.push_view()
    }

    fn set_members(
        &mut self,
        guild: Option<u64>,
        who: wit::view::Users,
        fields: Vec<wit::view::MemberField>,
    ) -> ActResult {
        self.require(Capability::View)?;
        if fields.len() > 16 || !fields.iter().all(member_field_ok) {
            return Self::invalid();
        }
        for f in fields {
            self.view.set_member(guild, who, f);
        }
        self.push_view()
    }

    fn set_my_setting(&mut self, key: String, value: Option<String>) -> ActResult {
        self.require(Capability::View)?;
        if !super::host_state::setting_allowed(&key)
            || value.as_deref().is_some_and(|v| v.len() > 4096)
        {
            return Self::invalid();
        }
        match value {
            Some(v) => self.view.settings.insert(key, v),
            None => self.view.settings.remove(&key),
        };
        self.push_view()
    }

    fn reset(&mut self) -> wasmtime::Result<()> {
        self.require(Capability::View)?;
        self.view = crate::model::ViewOverrides::default();
        let _ = self.push_view()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::valid_emoji;

    #[test]
    fn emoji() {
        assert!(valid_emoji("👍"));
        assert!(valid_emoji("rekindle:5001"));
        assert!(valid_emoji("1️⃣"));
        assert!(!valid_emoji("hello"));
        assert!(!valid_emoji("x:notanid"));
        assert!(!valid_emoji(""));
    }
}
