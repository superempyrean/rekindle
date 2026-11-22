//! Desktop notifications (requires `notifications`). [`post`] shows one
//! labelled with the plugin's name, rate-limited and subject to the client's
//! quiet hours and Do Not Disturb.
//! [`Plugin::on_notification`](crate::Plugin::on_notification) filters the
//! client's own, in the render isolate.

use crate::Target;
pub use crate::raw::rekindle::plugin::notification_types::{
    ClientNotification, NotificationDecision, NotificationKind,
};
use crate::raw::rekindle::plugin::notify as raw;
pub use raw::{DesktopNotification, NotifyError};

/// Posts a notification with no icon and no sound. A click goes to
/// `on_click`, if given.
pub fn post(title: &str, body: &str, on_click: Option<Target>) -> Result<(), NotifyError> {
    raw::post(&DesktopNotification {
        title: title.into(),
        body: body.into(),
        icon: None,
        on_click,
        sound: None,
    })
}

/// Posts a notification with every field set by the caller, an icon and a
/// sound included.
pub fn post_full(n: &DesktopNotification) -> Result<(), NotifyError> {
    raw::post(n)
}
