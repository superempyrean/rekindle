//! Local slash commands (requires `ui`), listed next to the client's own and
//! marked with the plugin's name. Options arrive typed. A command replies
//! with [`CommandReply::Local`] (an "only you can see this" message),
//! [`CommandReply::Insert`] (text in the composer) or [`CommandReply::Send`]
//! (a message from the user, which needs `compose` and still passes every
//! compose hook).
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # struct Shrug;
//! impl Plugin for Shrug {
//!     fn activate(_: &Granted) -> Result<Self, String> {
//!         let cmd = SlashCommand::new("shrug", "Append a shrug").text("message", "What to say", false);
//!         commands::register(cmd).map_err(|e| format!("{e:?}"))?;
//!         Ok(Shrug)
//!     }
//!
//!     fn on_slash_command(&mut self, inv: Invocation) -> CommandReply {
//!         let text = commands::text(&inv, "message").unwrap_or_default();
//!         CommandReply::Insert(format!("{text} ¯\\_(ツ)_/¯"))
//!     }
//! }
//! ```

use crate::raw::rekindle::plugin::command_types::SlashCommand as RawCommand;
pub use crate::raw::rekindle::plugin::command_types::{
    CommandOption, CommandReply, Invocation, OptionKind, OptionValue,
};
use crate::raw::rekindle::plugin::ui as raw;
pub use raw::UiError;

/// A slash command, built up option by option and passed to [`register`].
#[derive(Debug, Clone)]
pub struct SlashCommand(pub RawCommand);

impl SlashCommand {
    /// A command with no options, offered in guilds and DMs. `name` is
    /// lower-case, 1 to 32 characters, and shown as `/name`.
    pub fn new(name: &str, description: &str) -> Self {
        Self(RawCommand {
            name: name.into(),
            description: description.into(),
            options: Vec::new(),
            in_guilds: true,
            in_dms: true,
        })
    }

    /// Adds an option of any kind.
    pub fn option(
        mut self,
        name: &str,
        description: &str,
        kind: OptionKind,
        required: bool,
    ) -> Self {
        self.0.options.push(CommandOption {
            name: name.into(),
            description: description.into(),
            kind,
            required,
            choices: Vec::new(),
            min: None,
            max: None,
            autocomplete: false,
        });
        self
    }

    /// Adds a text option.
    pub fn text(self, name: &str, description: &str, required: bool) -> Self {
        self.option(name, description, OptionKind::Text, required)
    }

    /// Adds a true/false option.
    pub fn boolean(self, name: &str, description: &str, required: bool) -> Self {
        self.option(name, description, OptionKind::Boolean, required)
    }

    /// Adds a whole-number option.
    pub fn integer(self, name: &str, description: &str, required: bool) -> Self {
        self.option(name, description, OptionKind::Integer, required)
    }

    /// Adds a user option; the invocation carries the user's id.
    pub fn user(self, name: &str, description: &str, required: bool) -> Self {
        self.option(name, description, OptionKind::User, required)
    }

    /// Adds an option that takes one of `choices`, given as (label, value)
    /// pairs. The invocation carries the value.
    pub fn choice(
        mut self,
        name: &str,
        description: &str,
        required: bool,
        choices: &[(&str, &str)],
    ) -> Self {
        self = self.option(name, description, OptionKind::Choice, required);
        if let Some(o) = self.0.options.last_mut() {
            o.choices = choices
                .iter()
                .map(|(l, v)| ((*l).to_string(), (*v).to_string()))
                .collect();
        }
        self
    }

    /// Asks [`Plugin::on_autocomplete`](crate::Plugin::on_autocomplete) for
    /// suggestions on the last option added.
    pub fn autocomplete(mut self) -> Self {
        if let Some(o) = self.0.options.last_mut() {
            o.autocomplete = true;
        }
        self
    }

    /// Offers the command in guilds only.
    pub fn guilds_only(mut self) -> Self {
        self.0.in_dms = false;
        self
    }

    /// Offers the command in DMs only.
    pub fn dms_only(mut self) -> Self {
        self.0.in_guilds = false;
        self
    }
}

/// Registers a command; registering a name again replaces the command.
/// Runs arrive in [`Plugin::on_slash_command`](crate::Plugin::on_slash_command).
#[expect(
    clippy::needless_pass_by_value,
    reason = "the builder chain ends here: `register(SlashCommand::new(..).text(..))`"
)]
pub fn register(command: SlashCommand) -> Result<(), UiError> {
    raw::register_slash_command(&command.0)
}

/// Removes a command by name.
pub fn unregister(name: &str) {
    raw::unregister_slash_command(name);
}

/// The value of a text or choice option, if the user gave one.
pub fn text(inv: &Invocation, name: &str) -> Option<String> {
    match option(inv, name)? {
        OptionValue::Text(t) | OptionValue::Choice(t) => Some(t.clone()),
        _ => None,
    }
}

/// The value of a true/false option, if the user gave one.
pub fn boolean(inv: &Invocation, name: &str) -> Option<bool> {
    match option(inv, name)? {
        OptionValue::Boolean(b) => Some(*b),
        _ => None,
    }
}

/// The value of a whole-number option, if the user gave one.
pub fn integer(inv: &Invocation, name: &str) -> Option<i64> {
    match option(inv, name)? {
        OptionValue::Integer(i) => Some(*i),
        _ => None,
    }
}

/// The user id of a user option, if the user gave one.
pub fn user(inv: &Invocation, name: &str) -> Option<u64> {
    match option(inv, name)? {
        OptionValue::User(u) => Some(*u),
        _ => None,
    }
}

/// A plain-text "only you can see this" reply.
pub fn local(text: &str) -> CommandReply {
    CommandReply::Local(vec![crate::ui::span(text)])
}

fn option<'a>(inv: &'a Invocation, name: &str) -> Option<&'a OptionValue> {
    inv.options.iter().find(|(n, _)| n == name).map(|(_, v)| v)
}
