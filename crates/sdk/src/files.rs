//! Files through the system's own dialogs (requires `files`, and only
//! during a user gesture). The plugin never sees a path: the user picks the
//! file, and the host reads or writes it.

use crate::raw::rekindle::plugin::files as raw;
pub use raw::{FileError, Picked};

/// Shows a Save dialog suggesting `name`, and writes `data` where the user
/// chose. At most 64 MiB.
pub fn save(name: &str, content_type: &str, data: &[u8]) -> Result<(), FileError> {
    raw::save(name, content_type, data)
}

/// Shows an Open dialog for files with these extensions (`["json"]`), and
/// returns the file the user picked. At most 16 MiB.
pub fn open(accept: &[&str]) -> Result<Picked, FileError> {
    let accept: Vec<String> = accept.iter().map(|e| (*e).to_string()).collect();
    raw::open(&accept)
}
