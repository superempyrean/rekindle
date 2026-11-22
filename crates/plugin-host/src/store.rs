//! The store: the community index of the cached release channel, as the
//! user browses it, and downloading a plugin from it.
//!
//! Listing reads only the cached channel, so it works offline and shows
//! what the last update check accepted. [`download`] contacts the release
//! channel's host and nothing else. Installing goes through the review
//! screen (`review`) like any plugin that isn't first-party.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use semver::Version;

use crate::channel::{self, ChannelPlugin, CommunityPlugin};
use crate::home::Home;
use crate::manifest;
use crate::signing::Signature;

/// One plugin in the store.
#[derive(Debug, Clone)]
pub struct Item {
    pub entry: CommunityPlugin,
    /// The installed version, if any.
    pub installed: Option<String>,
}

impl Item {
    /// Whether the store has a newer version than the one installed.
    pub fn is_update(&self) -> bool {
        let (Some(installed), Ok(listed)) =
            (&self.installed, Version::parse(&self.entry.plugin.version))
        else {
            return false;
        };
        Version::parse(installed).is_ok_and(|installed| listed > installed)
    }
}

/// What the store lists: the newest version of each community plugin this
/// host can load, in id order. Empty until an update check has accepted a
/// channel.
pub fn list(home: &Home) -> Vec<Item> {
    let Some(c) = home.cached_channel() else {
        return Vec::new();
    };
    let mut installed = installed_versions(home);
    c.store(manifest::rekindle_wit_version())
        .into_iter()
        .map(|entry| Item {
            installed: installed.remove(&entry.plugin.id),
            entry: entry.clone(),
        })
        .collect()
}

/// What the first-party tab lists: the newest version of each of rekindle's
/// own plugins this host can load, in id order, with the installed version.
pub fn first_party(home: &Home) -> Vec<(ChannelPlugin, Option<String>)> {
    let Some(c) = home.cached_channel() else {
        return Vec::new();
    };
    let mut installed = installed_versions(home);
    c.first_party(manifest::rekindle_wit_version())
        .into_iter()
        .map(|p| (p.clone(), installed.remove(&p.id)))
        .collect()
}

/// The version of each installed plugin whose file could be read.
fn installed_versions(home: &Home) -> BTreeMap<String, String> {
    home.installed()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| Some((i.id, i.manifest?.version)))
        .collect()
}

/// A downloaded plugin file: its bytes, and its signature if the channel
/// carries one (first-party plugins).
pub type Download = (Vec<u8>, Option<Signature>);

/// Download the newest version of plugin `id` from the release channel,
/// checked against the cached channel's listing: one of rekindle's own
/// plugins with its signature, or a community plugin. Blocking: run it off
/// the client's UI thread.
pub fn download(home: &Home, id: &str) -> Result<Download> {
    let c = home
        .cached_channel()
        .context("rekindle hasn't checked for updates yet, so there is nothing to install")?;
    let location = home
        .channel_location()?
        .context("no release channel is set")?;
    let wit = manifest::rekindle_wit_version();
    if let Some(entry) = c.latest(id, wit) {
        let (bytes, sig) = channel::fetch_plugin(&location, entry)?;
        return Ok((bytes, Some(sig)));
    }
    let entry = c
        .latest_community(id, wit)
        .with_context(|| format!("{id} isn't listed"))?;
    Ok((channel::fetch_community(&location, entry)?, None))
}
