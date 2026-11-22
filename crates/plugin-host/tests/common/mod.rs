//! Helpers the end-to-end tests of installs, updates, themes and the
//! settings panel share.

use std::ops::Deref;
use std::path::{Path, PathBuf};

/// An empty directory under the system's temp directory, removed when
/// dropped.
pub(crate) struct Scratch(PathBuf);

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A file something still holds leaves the directory behind, in the
        // temp directory, where it fails nothing.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh [`Scratch`] directory for the test called `name`.
pub(crate) fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "rekindle-{}-{}-{name}",
        env!("CARGO_CRATE_NAME"),
        std::process::id()
    ));
    // Absent, unless an earlier process had the same id.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// The `midnight-theme` example plugin, built once per test process.
pub(crate) fn midnight() -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/rust/midnight-theme");
    rekindle_plugin_host::testing::build(dir)
        .expect("building midnight-theme")
        .bytes
        .clone()
}
