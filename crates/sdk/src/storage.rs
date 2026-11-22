//! Per-plugin key/value storage (requires `storage`).
//!
//! Values are strings; [`get_or`] and [`set`] convert. Only this plugin sees
//! its keys, and they belong to the account the user is logged in with. The
//! limits are 256 bytes per key, 64 KiB per value, and 1024 keys and 1 MiB
//! in all.

use std::str::FromStr;

use crate::raw::rekindle::plugin::storage;
pub use storage::StorageError;

/// The stored value of `key`, if any.
pub fn get(key: &str) -> Option<String> {
    storage::get(key)
}

/// The stored value of `key` parsed as `T`, or `default` if it's missing or
/// doesn't parse.
pub fn get_or<T: FromStr>(key: &str, default: T) -> T {
    get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Stores `value` as text (`true`, `42`, `#ff00aa`, ...). Fails with the
/// limit it would break: [`StorageError::KeyTooLong`],
/// [`StorageError::ValueTooLarge`] or [`StorageError::QuotaExceeded`].
#[expect(
    clippy::needless_pass_by_value,
    reason = "callers pass values as they are: `set(\"count\", 3)`"
)]
pub fn set(key: &str, value: impl ToString) -> Result<(), StorageError> {
    storage::set(key, &value.to_string())
}

/// Removes `key`; nothing happens if it isn't there.
pub fn delete(key: &str) {
    storage::delete(key);
}

/// Every key the plugin has stored, sorted.
pub fn keys() -> Vec<String> {
    storage::keys(None)
}

/// The keys starting with `prefix`, sorted.
pub fn keys_with_prefix(prefix: &str) -> Vec<String> {
    storage::keys(Some(prefix))
}

/// The values of several keys at once, in the same order.
pub fn get_many(keys: &[&str]) -> Vec<Option<String>> {
    let keys: Vec<String> = keys.iter().map(|k| (*k).to_string()).collect();
    storage::get_many(&keys)
}

/// The bytes used and the bytes allowed.
pub fn usage() -> (u64, u64) {
    storage::usage()
}
