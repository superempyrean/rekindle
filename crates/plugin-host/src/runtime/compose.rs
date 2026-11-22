//! Compose: a message the user sends runs past every `compose-handler`, and
//! a file the user attaches is offered to the ones that opted in.

use super::host_actions::MAX_SEND_BYTES;
use super::state;
use super::{Runtime, Status};
use crate::api;
use crate::model;

impl Runtime {
    /// The user is sending (or saving an edit of) a message: run it through
    /// every active plugin granted `compose`, in load order, each seeing the
    /// previous result. Call this from the client's own send path only. A
    /// rewrite that isn't a sendable message is refused and the previous text
    /// kept; a `cancel` stops the send.
    pub fn compose(&mut self, draft: &api::Draft) -> model::Composed {
        let mut draft = draft.clone();
        let mut out = model::Composed {
            text: draft.text.clone(),
            parts: Vec::new(),
            mention: None,
            silent: None,
            cancelled: None,
            rewritten_by: Vec::new(),
            rejected: Vec::new(),
        };
        // The latest split: (plugin, the text it split, the parts).
        let mut split: Option<(String, String, Vec<String>)> = None;
        for idx in 0..self.plugins.len() {
            let p = &self.plugins[idx];
            if p.status != Status::Active || p.hooks.compose.is_none() {
                continue;
            }
            let id = p.manifest.id.clone();
            let sending = draft.kind == api::ComposeKind::Send;
            self.plugins[idx].store.data_mut().composing = Some(state::Composing {
                sending,
                split: None,
            });
            let result = self.call(idx, |h, store| match &h.compose {
                Some(c) => c.call_on_compose(store, &draft),
                None => Ok(api::ComposeResult::Keep),
            });
            let asked = self.plugins[idx]
                .store
                .data_mut()
                .composing
                .take()
                .and_then(|c| c.split);
            let Some(result) = result else { continue };
            if let Some(parts) = asked {
                // A split replaces the text; a rewrite from the same call doesn't count.
                split = Some((id.clone(), draft.text.clone(), parts));
                if !matches!(result, api::ComposeResult::Cancel(_))
                    && !out.rewritten_by.contains(&id)
                {
                    out.rewritten_by.push(id.clone());
                }
            }
            let result = match result {
                api::ComposeResult::Replace(rw)
                    if split.as_ref().is_some_and(|(by, ..)| *by == id) =>
                {
                    api::ComposeResult::Replace(api::Rewrite {
                        text: draft.text.clone(),
                        ..rw
                    })
                }
                other => other,
            };
            match result {
                api::ComposeResult::Keep => {}
                api::ComposeResult::Cancel(reason) => {
                    let reason = state::clean(&reason, 200).unwrap_or_else(|| "cancelled".into());
                    out.cancelled = Some((id, reason));
                    return out;
                }
                api::ComposeResult::Replace(rw) => self.rewrite(id, rw, &mut draft, &mut out),
            }
        }
        if let Some((by, seen, parts)) = split {
            if seen == draft.text {
                out.parts = parts;
            } else {
                self.host_log(
                    &by,
                    model::LogLevel::Warn,
                    "split dropped: a later plugin changed the message",
                );
            }
        }
        out.text = draft.text;
        out
    }

    /// Apply plugin `id`'s rewrite to `draft`, refusing text that isn't a
    /// sendable message, and record what changed in `out`.
    fn rewrite(
        &mut self,
        id: String,
        rw: api::Rewrite,
        draft: &mut api::Draft,
        out: &mut model::Composed,
    ) {
        let mut changed = false;
        if rw.text != draft.text {
            match check_compose(&rw.text) {
                None => {
                    draft.text = rw.text;
                    changed = true;
                }
                Some(why) => {
                    self.host_log(
                        &id,
                        model::LogLevel::Warn,
                        &format!("compose rewrite refused: {why}"),
                    );
                    out.rejected.push((id.clone(), why.to_string()));
                }
            }
        }
        if let Some(m) = rw.mention
            && let Some(r) = draft.reply.as_mut()
            && r.mention != m
        {
            r.mention = m;
            out.mention = Some(m);
            changed = true;
        }
        if let Some(s) = rw.silent
            && draft.silent != s
        {
            draft.silent = s;
            out.silent = Some(s);
            changed = true;
        }
        if changed && !out.rewritten_by.contains(&id) {
            out.rewritten_by.push(id);
        }
    }

    /// [`Self::compose`] for a plain send of `text` to `channel`.
    pub fn compose_text(&mut self, channel: u64, text: &str) -> model::Composed {
        self.compose(&api::Draft {
            channel,
            guild: None,
            kind: api::ComposeKind::Send,
            editing: None,
            text: text.to_string(),
            reply: None,
            attachments: vec![],
            sticker_count: 0,
            silent: false,
        })
    }

    /// A file the user attached: offer it to plugins that opted in with
    /// `compose-uploads` (in load order). A file larger than
    /// `actions.send-message` takes is kept as it is.
    pub fn upload(&mut self, file: &api::Upload) -> model::UploadOutcome {
        if file.data.len() > MAX_SEND_BYTES {
            return model::UploadOutcome::Keep;
        }
        let mut cur = file.clone();
        let mut by = None;
        for idx in 0..self.plugins.len() {
            let p = &self.plugins[idx];
            if p.status != Status::Active
                || p.hooks.compose.is_none()
                || !p.manifest.compose_uploads
            {
                continue;
            }
            let id = p.manifest.id.clone();
            match self.call(idx, |h, store| match &h.compose {
                Some(c) => c.call_on_upload(store, &cur),
                None => Ok(api::UploadResult::Keep),
            }) {
                Some(api::UploadResult::Drop(reason)) => {
                    return model::UploadOutcome::Drop {
                        plugin: id,
                        reason: state::clean(&reason, 200).unwrap_or_default(),
                    };
                }
                // The checks `actions.send-message` makes of a file.
                Some(api::UploadResult::Replace(u))
                    if !u.data.is_empty()
                        && u.data.len() <= MAX_SEND_BYTES
                        && state::clean(&u.filename, 200).is_some_and(|n| n == u.filename) =>
                {
                    cur = u;
                    by = Some(id);
                }
                _ => {}
            }
        }
        match by {
            Some(by) => model::UploadOutcome::Replace {
                filename: cur.filename,
                content_type: cur.content_type,
                data: cur.data,
                by,
            },
            None => model::UploadOutcome::Keep,
        }
    }
}

/// Max characters in a composed message. Fixed at Nitro's 4000 for now;
/// without Nitro Discord takes 2000, so with a real client the host should
/// read the user's limit from the client.
const MAX_COMPOSE_CHARS: usize = 4000;

/// The most parts `compose-control.split` sends a message as.
pub(super) const MAX_SPLIT_PARTS: usize = 10;

/// Why a `compose` rewrite is refused, if it is: it must still be a sendable
/// message (non-blank, within the length cap, no control characters besides
/// newlines and tabs).
pub(super) fn check_compose(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        Some("rewrote the message to nothing")
    } else if text.chars().count() > MAX_COMPOSE_CHARS {
        Some("rewrote the message past the length limit")
    } else if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        Some("rewrote the message with control characters")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::check_compose;

    #[test]
    fn compose_checks() {
        assert!(check_compose("hello").is_none());
        assert!(check_compose("  ").is_some());
        assert!(check_compose("a\u{0007}b").is_some());
        assert!(check_compose(&"x".repeat(4001)).is_some());
        assert!(check_compose("line\nbreak").is_none());
    }
}
