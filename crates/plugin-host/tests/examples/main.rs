//! The example plugins in `plugins/`, each driven through the harness.
//! A module per plugin, in one test binary. The TypeScript plugins' tests
//! need jco: `npm install` in `plugins/`.

#[path = "../toolchain/mod.rs"]
mod toolchain;

mod accent_panel;
mod compact_mode;
mod copy_user_urls;
mod declutter;
mod kaomoji;
mod keyword_highlight;
mod message_logger;
mod midnight_theme;
mod mute_words;
mod no_reply_mention;
mod oneko;
mod permissions_viewer;
mod pin_dms;
mod platform_indicators;
mod quick_react;
mod read_all;
mod silent_typing;
mod split_long_messages;
mod staff_tag;
mod text_replace;
mod typing_indicator;
mod typography;
mod user_volume;
mod view_raw;
