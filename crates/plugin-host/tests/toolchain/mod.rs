//! Whether the toolchain for TypeScript and JavaScript plugins, jco from
//! `npm install` in `plugins/`, is installed.

use std::path::{Path, PathBuf};

/// The plugin directory `dir`, or `None` after saying why if jco isn't
/// installed. With `REKINDLE_REQUIRE_GUEST_TOOLCHAINS` set, a missing
/// toolchain fails the test instead (CI).
pub(crate) fn installed(dir: &Path) -> Option<PathBuf> {
    if dir
        .ancestors()
        .any(|d| d.join("node_modules/@bytecodealliance/jco").exists())
    {
        return Some(dir.to_path_buf());
    }
    let how = "`npm install` in plugins/";
    assert!(
        std::env::var_os("REKINDLE_REQUIRE_GUEST_TOOLCHAINS").is_none(),
        "{}: toolchain not installed; run {how}",
        dir.display()
    );
    eprintln!(
        "skipping {}: toolchain not installed (run {how})",
        dir.display()
    );
    None
}

/// The plugin directory, or return from the test if its toolchain isn't
/// installed.
macro_rules! needs {
    ($dir:expr) => {
        match $crate::toolchain::installed(::std::path::Path::new(&$dir)) {
            Some(dir) => dir,
            None => return,
        }
    };
}
pub(crate) use needs;
