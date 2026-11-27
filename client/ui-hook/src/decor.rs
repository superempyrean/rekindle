//! Plugins' keyed decorations and message accessories, as the runtime sets
//! them, for a host that draws them.
//!
//! A decoration is a small badge a plugin keys by a user, channel, guild or
//! message id, in one of the contract's kinds (`user.name-badge`,
//! `member-list.trailing`, ...). An accessory is a plugin's lines under one
//! message. The host says which kinds it draws ([`set_decoration_kinds`])
//! and whether it draws accessories ([`set_accessories_drawn`]); the runtime
//! refuses the others as unavailable. [`generation`] moves on every change,
//! so a host can poll it and read the lists only when it moved.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A plugin's decoration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecorationItem {
    pub plugin: String,
    pub kind: String,
    pub key: u64,
    /// What it shows: its text, or its tooltip when it has none.
    pub text: String,
    pub tooltip: String,
    /// `0xRRGGBBAA`; 0 is the client's own colour.
    pub color: u32,
}

/// A plugin's lines under a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessoryItem {
    pub plugin: String,
    pub message: u64,
    pub channel: u64,
    pub lines: Vec<String>,
}

static KINDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ACCESSORIES_DRAWN: AtomicBool = AtomicBool::new(false);
static DECORATIONS: Mutex<Vec<DecorationItem>> = Mutex::new(Vec::new());
static ACCESSORIES: Mutex<Vec<AccessoryItem>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn changed() {
    GENERATION.fetch_add(1, Ordering::Release);
}

/// The decoration kinds the host draws.
pub fn set_decoration_kinds(kinds: Vec<String>) {
    *lock(&KINDS) = kinds;
}

pub fn decoration_kinds() -> Vec<String> {
    lock(&KINDS).clone()
}

/// Whether the host draws message accessories.
pub fn set_accessories_drawn(drawn: bool) {
    ACCESSORIES_DRAWN.store(drawn, Ordering::Release);
}

pub fn accessories_drawn() -> bool {
    ACCESSORIES_DRAWN.load(Ordering::Acquire)
}

/// Moves on every change to the decorations or the accessories.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub fn decorations() -> Vec<DecorationItem> {
    lock(&DECORATIONS).clone()
}

pub fn accessories() -> Vec<AccessoryItem> {
    lock(&ACCESSORIES).clone()
}

/// Set `plugin`'s decoration of `kind` on `key`, or take it off.
pub fn put_decoration(plugin: &str, kind: &str, key: u64, item: Option<DecorationItem>) {
    let mut all = lock(&DECORATIONS);
    all.retain(|d| !(d.plugin == plugin && d.kind == kind && d.key == key));
    all.extend(item);
    changed();
}

/// Take off every decoration of `kind` that `plugin` set.
pub fn clear_decorations(plugin: &str, kind: &str) {
    lock(&DECORATIONS).retain(|d| !(d.plugin == plugin && d.kind == kind));
    changed();
}

/// Set `plugin`'s accessory on `message`, or take it off.
pub fn put_accessory(plugin: &str, message: u64, item: Option<AccessoryItem>) {
    let mut all = lock(&ACCESSORIES);
    all.retain(|a| !(a.plugin == plugin && a.message == message));
    all.extend(item);
    changed();
}

/// Drop everything `plugin` set.
pub fn plugin_unloaded(plugin: &str) {
    lock(&DECORATIONS).retain(|d| d.plugin != plugin);
    lock(&ACCESSORIES).retain(|a| a.plugin != plugin);
    changed();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(plugin: &str, key: u64, text: &str) -> DecorationItem {
        DecorationItem {
            plugin: plugin.into(),
            kind: "test.kind".into(),
            key,
            text: text.into(),
            tooltip: String::new(),
            color: 0,
        }
    }

    /// A decoration replaces the plugin's own on the same key, and leaves
    /// other plugins' alone.
    #[test]
    fn a_decoration_replaces_the_plugins_own() {
        put_decoration("a", "test.kind", 1, Some(badge("a", 1, "x")));
        put_decoration("b", "test.kind", 1, Some(badge("b", 1, "y")));
        let before = generation();
        put_decoration("a", "test.kind", 1, Some(badge("a", 1, "z")));
        assert!(generation() > before);
        let mine: Vec<String> = decorations()
            .into_iter()
            .filter(|d| d.kind == "test.kind" && d.key == 1)
            .map(|d| d.text)
            .collect();
        assert_eq!(mine, ["y", "z"]);
        clear_decorations("a", "test.kind");
        plugin_unloaded("b");
        assert!(decorations().iter().all(|d| d.kind != "test.kind"));
    }
}
