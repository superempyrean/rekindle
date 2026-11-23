//! The Developer tab's console: one action as JSON, run against the
//! client's runtime. The actions carry the same fields as the preview's
//! `/api` bodies, so an action written against `rekindle dev` runs here
//! too.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::runtime::{Runtime, Status};

/// Shown above the console's input.
pub const HELP: &str = r#"Run an action, written as JSON. {"action":"plugins"} lists the running plugins. {"action":"command","plugin":"<id>","id":"<command>"} runs a palette command, and "keybind" fires a keybind with the same fields. Both count as a user gesture."#;

/// Run one console action and describe the result for the user.
pub fn run(rt: &mut Runtime, command: &str) -> String {
    match act(rt, command) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|e| e.to_string()),
        Err(e) => format!("error: {e:#}"),
    }
}

fn act(rt: &mut Runtime, command: &str) -> Result<Value> {
    let body: Value = serde_json::from_str(command).context("the action isn't valid JSON")?;
    let action = body["action"]
        .as_str()
        .context("the action has no \"action\" field")?;
    let field = |name: &str| {
        body[name]
            .as_str()
            .with_context(|| format!("`{action}` needs a \"{name}\" field"))
    };
    match action {
        "plugins" => Ok(Value::Array(
            rt.plugins()
                .into_iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "version": p.version,
                        "status": match p.status {
                            Status::Active => "active".to_string(),
                            Status::Faulted(why) => format!("faulted: {why}"),
                        },
                        "capabilities": p.granted,
                    })
                })
                .collect(),
        )),
        "command" => {
            rt.run_command(field("plugin")?, field("id")?)?;
            Ok(json!("ok"))
        }
        "keybind" => {
            rt.keybind(field("plugin")?, field("id")?)?;
            Ok(json!("ok"))
        }
        other => bail!("unknown action `{other}`: the console runs plugins, command and keybind"),
    }
}
