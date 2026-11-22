//! Notification Router: suppress the client's notifications from channels
//! you list, and show them without sound when they contain words you list.
//!
//! [`Plugin::on_notification`] runs in the render isolate: it reads the
//! notification's text but its only output is the decision, a few bits, so
//! it can't leak what it saw.

use rekindle_sdk::notify::{ClientNotification, NotificationDecision};
use rekindle_sdk::prelude::*;

struct NotificationRouter;

impl Plugin for NotificationRouter {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        Ok(NotificationRouter)
    }

    fn on_notification(n: &ClientNotification) -> NotificationDecision {
        if n.channel
            .is_some_and(|c| settings::ids("quiet-channels").contains(&c))
        {
            return NotificationDecision::Suppress;
        }
        let body = n.body.to_lowercase();
        if settings::items("silent-words")
            .iter()
            .any(|w| !w.is_empty() && body.contains(&w.to_lowercase()))
        {
            return NotificationDecision::Silent;
        }
        NotificationDecision::Show
    }
}

rekindle_sdk::plugin!(NotificationRouter);
