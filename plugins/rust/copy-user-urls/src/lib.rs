//! Copy User URLs: a `ui` + `clipboard` plugin, after Vencord's `CopyUserURLs`.
//!
//! Adds "Copy User URL" to every user's right-click menu; clicking it puts
//! `https://discord.com/users/<id>` on the clipboard. The click is the user's
//! gesture, which is the only time the host lets a plugin write the clipboard.

use rekindle_sdk::prelude::*;
use rekindle_sdk::slots::UiError;

const ITEM: &str = "copy-user-url";

/// The canonical profile link for a user id.
fn user_url(id: u64) -> String {
    format!("https://discord.com/users/{id}")
}

struct CopyUserUrls;

impl Plugin for CopyUserUrls {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        match slots::add(
            slots::USER_MENU,
            MenuItem::new(ITEM, "Copy User URL")
                .icon(Icon::Link)
                .group("copy"),
        ) {
            Ok(()) => {}
            Err(UiError::Unavailable) => {
                warn!("user context menus aren't available on this client build");
            }
            Err(e) => error!("couldn't add the menu item: {e:?}"),
        }
        Ok(Self)
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        let SlotEvent::Clicked(SlotClick {
            item,
            target: Target::User((user, _)),
            ..
        }) = ev
        else {
            return;
        };
        if item != ITEM {
            return;
        }
        match clipboard::write_text(&user_url(user.id)) {
            // Some clients only hand over the id; don't print an empty name.
            Ok(()) if user.name.is_empty() => ui::toast_kind("Copied user URL", ToastKind::Success),
            Ok(()) => ui::toast_kind(&format!("Copied {}'s URL", user.name), ToastKind::Success),
            Err(e) => ui::toast_kind(&format!("Couldn't copy: {e:?}"), ToastKind::Error),
        }
    }
}

rekindle_sdk::plugin!(CopyUserUrls);
