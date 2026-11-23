//! Local overrides of how this client shows users and members, the current
//! user included, and of the user's own settings (`view`). The queries in
//! [`crate::state`] and [`crate::users`] return the overridden records.
//! Nothing is sent to Discord, and the overrides go when the plugin
//! unloads. A later override of the same field for the same users replaces
//! an earlier one; between plugins, the one set last wins.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! use rekindle_sdk::view::{UserField, Users};
//! // Everyone else's profile cosmetics, gone.
//! let _ = view::set_users(Users::Others, &[
//!     UserField::AvatarDecoration(None),
//!     UserField::Nameplate(None),
//!     UserField::DisplayNameStyle(None),
//!     UserField::ThemeColors(vec![]),
//!     UserField::ProfileEffect(None),
//! ]);
//! ```

pub use crate::raw::rekindle::plugin::action_types::ActionError;
use crate::raw::rekindle::plugin::view as raw;
pub use raw::{MemberField, UserField, Users};

/// What every call returns.
pub type Result = std::result::Result<(), ActionError>;

/// Overrides these fields of the users `who` names.
pub fn set_users(who: Users, fields: &[UserField]) -> Result {
    raw::set_users(who, fields)
}

/// Overrides these fields of members, in `guild` or, with `None`, in every
/// guild.
pub fn set_members(guild: Option<u64>, who: Users, fields: &[MemberField]) -> Result {
    raw::set_members(guild, who, fields)
}

/// Overrides one of the keys [`crate::state::my_setting_keys`] lists, as its
/// string form, or drops the override with `None`.
pub fn set_my_setting(key: &str, value: Option<&str>) -> Result {
    raw::set_my_setting(key, value)
}

/// Drops every override this plugin made.
pub fn reset() {
    raw::reset();
}
