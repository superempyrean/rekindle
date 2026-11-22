//! What a rekindle release is made of, for both halves of rekindle that
//! update: the plugin runtime (`plugin-host`: plugins and the blocklist)
//! and the launcher (`rekindle-launcher`: rekindle's own files).
//!
//! - [`signing`]: Ed25519 keys, detached signatures, the project keys.
//! - [`location`]: a path or https URL to fetch from.
//! - [`framework`]: the release channel's `framework` and `launcher`
//!   sections.
//! - [`update`]: updating an install from them.

pub mod framework;
pub mod location;
pub mod signing;
pub mod update;

/// Environment variable overriding the release channel (path or https URL).
pub const CHANNEL_ENV: &str = "REKINDLE_CHANNEL";

/// The official release channel, used when neither `$REKINDLE_CHANNEL` nor
/// the user's config names one.
pub const OFFICIAL_CHANNEL: Option<&str> = Some("https://dl.rekindle.dev/channel/channel.json");

/// The channel to use: `$REKINDLE_CHANNEL`, else `configured`, else the
/// official one.
pub fn channel_location(configured: Option<&str>) -> anyhow::Result<Option<location::Location>> {
    let env = std::env::var(CHANNEL_ENV).ok().filter(|c| !c.is_empty());
    env.as_deref()
        .or(configured)
        .or(OFFICIAL_CHANNEL)
        .map(location::Location::parse)
        .transpose()
}

/// Environment variable overriding the plugin home.
pub const PLUGIN_HOME_ENV: &str = "REKINDLE_HOME";

/// The user's plugin home (installed plugins, their settings and saved
/// data): `$REKINDLE_HOME`, else `%USERPROFILE%\.rekindle` / `$HOME/.rekindle`.
pub fn plugin_home() -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context as _;
    if let Some(h) = std::env::var_os(PLUGIN_HOME_ENV).filter(|h| !h.is_empty()) {
        return Ok(h.into());
    }
    let base = std::env::var_os("USERPROFILE")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|h| !h.is_empty()))
        .context("cannot find your home directory; set REKINDLE_HOME")?;
    Ok(std::path::PathBuf::from(base).join(".rekindle"))
}
