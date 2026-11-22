//! Client tweaks the host maintains (requires `tweaks`): behaviour switches
//! rekindle implements for each client build, such as silent typing,
//! always animate and no mosaic. A plugin can turn them on as a preset, and
//! a value the user set by hand wins.

use crate::raw::rekindle::plugin::tweaks as raw;
pub use raw::{TweakError, TweakInfo, TweakKind};

/// Every tweak, with its kind, its current value and whether this build has
/// it.
pub fn all() -> Vec<TweakInfo> {
    raw::all()
}

/// Sets a tweak while the plugin is enabled: `"true"`, one of its choices,
/// or a number. Fails with [`TweakError::UserLocked`] if the user set it.
pub fn set(id: &str, value: &str) -> Result<(), TweakError> {
    raw::set(id, value)
}

/// Removes the plugin's value for a tweak.
pub fn reset(id: &str) {
    raw::reset(id);
}
