//! A slash command typed as text, read as the plugin declared it.
//!
//! A client that hands rekindle the composer's text, rather than its own
//! command picker's options, gets the options from the words after the
//! command's name, in the order the command lists them. The last option
//! takes the rest of the text when it is a text option, so a search can
//! have spaces in it. A user, channel or role is its id, with or without
//! Discord's `<@…>`, `<#…>` or `<@&…>` around it.

use rekindle_plugin_host::api::{
    ChannelRef, CommandOption, Invocation, OptionKind, OptionValue, SlashCommand,
};

/// The command `text` runs among `commands`, as (plugin, command): the
/// word after its leading `/`.
pub fn find<'c>(
    commands: &'c [(String, SlashCommand)],
    text: &str,
) -> Option<&'c (String, SlashCommand)> {
    let name = text
        .trim_start()
        .strip_prefix('/')?
        .split_whitespace()
        .next()?;
    commands.iter().find(|(_, c)| c.name == name)
}

/// The invocation `text` (`/name` and its options) makes of `cmd` in
/// `channel`, or what is wrong with it, for the user.
pub fn invocation(
    cmd: &SlashCommand,
    text: &str,
    channel: ChannelRef,
) -> Result<Invocation, String> {
    let mut rest = text
        .trim()
        .strip_prefix('/')
        .and_then(|t| t.strip_prefix(cmd.name.as_str()))
        .ok_or_else(|| format!("that isn't /{}", cmd.name))?
        .trim_start();
    let mut options = Vec::new();
    for (i, o) in cmd.options.iter().enumerate() {
        let last = i + 1 == cmd.options.len();
        let raw = if last && o.kind == OptionKind::Text {
            std::mem::take(&mut rest).trim()
        } else {
            let (word, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            rest = after.trim_start();
            word
        };
        if raw.is_empty() {
            if o.required {
                return Err(format!("/{} needs {}", cmd.name, o.name));
            }
            continue;
        }
        let value = value(o, raw).map_err(|e| format!("/{} {}: {e}", cmd.name, o.name))?;
        options.push((o.name.clone(), value));
    }
    Ok(Invocation {
        command: cmd.name.clone(),
        options,
        channel,
    })
}

fn value(o: &CommandOption, raw: &str) -> Result<OptionValue, String> {
    let in_range = |n: f64| {
        if o.min.is_some_and(|m| n < m) || o.max.is_some_and(|m| n > m) {
            Err(format!("{raw} is out of range"))
        } else {
            Ok(())
        }
    };
    let id = |prefixes: &[&str]| {
        let inner = raw
            .strip_prefix('<')
            .and_then(|r| r.strip_suffix('>'))
            .map_or(raw, |r| {
                prefixes.iter().find_map(|p| r.strip_prefix(p)).unwrap_or(r)
            });
        inner
            .parse::<u64>()
            .map_err(|_| format!("{raw} isn't an id"))
    };
    Ok(match o.kind {
        OptionKind::Text => OptionValue::Text(raw.to_string()),
        OptionKind::Integer => {
            let n: i64 = raw
                .parse()
                .map_err(|_| format!("{raw} isn't a whole number"))?;
            in_range(n as f64)?;
            OptionValue::Integer(n)
        }
        OptionKind::Number => {
            let n: f64 = raw.parse().map_err(|_| format!("{raw} isn't a number"))?;
            in_range(n)?;
            OptionValue::Number(n)
        }
        OptionKind::Boolean => match raw.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => OptionValue::Boolean(true),
            "false" | "no" | "off" => OptionValue::Boolean(false),
            _ => return Err(format!("{raw} isn't true or false")),
        },
        OptionKind::User => OptionValue::User(id(&["@!", "@"])?),
        OptionKind::Channel => OptionValue::Channel(id(&["#"])?),
        OptionKind::Role => OptionValue::Role(id(&["@&"])?),
        OptionKind::Mentionable => OptionValue::User(id(&["@&", "@!", "@"])?),
        OptionKind::Choice => o
            .choices
            .iter()
            .find(|(label, value)| label.eq_ignore_ascii_case(raw) || value == raw)
            .map(|(_, value)| OptionValue::Choice(value.clone()))
            .ok_or_else(|| format!("{raw} isn't one of the choices"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(name: &str, kind: OptionKind, required: bool) -> CommandOption {
        CommandOption {
            name: name.into(),
            description: String::new(),
            kind,
            required,
            choices: vec![("Big".into(), "big".into())],
            min: Some(1.0),
            max: None,
            autocomplete: false,
        }
    }

    fn command(options: Vec<CommandOption>) -> SlashCommand {
        SlashCommand {
            name: "kaomoji".into(),
            description: String::new(),
            options,
            in_guilds: true,
            in_dms: true,
        }
    }

    fn channel() -> ChannelRef {
        ChannelRef {
            id: 0,
            name: String::new(),
            guild: None,
            kind: 0,
        }
    }

    #[test]
    fn the_last_text_option_takes_the_rest() {
        let cmd = command(vec![
            option("size", OptionKind::Choice, false),
            option("count", OptionKind::Integer, true),
            option("search", OptionKind::Text, true),
        ]);
        let list = vec![("p".to_string(), cmd.clone())];
        assert!(find(&list, "  /kaomoji big 2 x").is_some());
        assert!(find(&list, "/kaomojis").is_none());
        let inv = invocation(&cmd, "/kaomoji big 2  happy cat ", channel()).unwrap();
        assert_eq!(
            inv.options,
            [
                ("size".to_string(), OptionValue::Choice("big".into())),
                ("count".to_string(), OptionValue::Integer(2)),
                ("search".to_string(), OptionValue::Text("happy cat".into())),
            ]
        );
    }

    #[test]
    fn a_missing_or_bad_option_is_the_users_to_fix() {
        let cmd = command(vec![
            option("count", OptionKind::Integer, true),
            option("who", OptionKind::User, false),
        ]);
        assert_eq!(
            invocation(&cmd, "/kaomoji", channel()).unwrap_err(),
            "/kaomoji needs count"
        );
        assert_eq!(
            invocation(&cmd, "/kaomoji 0", channel()).unwrap_err(),
            "/kaomoji count: 0 is out of range"
        );
        let inv = invocation(&cmd, "/kaomoji 3 <@!42>", channel()).unwrap();
        assert_eq!(inv.options[1], ("who".to_string(), OptionValue::User(42)));
    }
}
