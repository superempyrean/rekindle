//! No Reply Mention, after Vencord's `NoReplyMention`: replies don't ping
//! their author by default, except people you list in the settings.
//!
//! Compose v2 (design doc §7.1): the draft carries the reply and whether it
//! mentions; the plugin answers with a rewrite that flips `mention`, and the
//! host tells the user which plugin changed their message.

use rekindle_sdk::prelude::*;

struct NoReplyMention;

impl Plugin for NoReplyMention {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        Ok(NoReplyMention)
    }

    fn on_compose(&mut self, d: &Draft) -> ComposeResult {
        let Some(reply) = &d.reply else {
            return ComposeResult::Keep;
        };
        if !reply.mention || settings::ids("always-ping").contains(&reply.author.id) {
            return ComposeResult::Keep;
        }
        compose::set_mention(d, false)
    }
}

rekindle_sdk::plugin!(NoReplyMention);
