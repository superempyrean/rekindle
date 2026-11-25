//! The example plugins in `plugins/`, each driven through the harness.
//! A module per plugin, in one test binary. The TypeScript plugins' tests
//! need jco: `npm install` in `plugins/`.

#[path = "../toolchain/mod.rs"]
mod toolchain;

mod accent_panel;
mod bookmarks;
mod call_timer;
mod channel_notes;
mod compact_mode;
mod copy_user_urls;
mod declutter;
mod focus_mode;
mod friend_notifications;
mod kaomoji;
mod keyword_highlight;
mod keyword_notify;
mod message_click_actions;
mod message_link_embeds;
mod message_logger;
mod midnight_theme;
mod mute_words;
mod no_reply_mention;
mod notification_router;
mod oneko;
mod online_friend_count;
mod permissions_viewer;
mod pin_dms;
mod platform_indicators;
mod quick_mention;
mod quick_react;
mod read_all;
mod server_info;
mod silent_typing;
mod snippet;
mod split_long_messages;
mod staff_tag;
mod text_replace;
mod theme_gallery;
mod typing_indicator;
mod typography;
mod user_volume;
mod view_raw;
