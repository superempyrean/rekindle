//! CLI subcommands, grouped by area.

/// `new`, `build`, `inspect`.
pub(crate) mod author;
/// `browse`, `get`, `check`, `submit`, `index`.
pub(crate) mod community;
/// `dev` (live preview + hot reload), `run`.
pub(crate) mod dev;
/// `install`, `enable`, `disable`, `developer-mode`, `list`, `uninstall`;
/// `keygen`, `sign`, `verify`, `channel`.
pub(crate) mod plugins;
/// `pick`.
pub(crate) mod wire;
