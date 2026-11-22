//! Server Info, after Vencord's `ServerInfo`: "Server info" in every guild's
//! right-click menu opens a modal with what the client already knows about
//! the server. A `read-client` + `read-users` plugin: nothing here fetches,
//! and dates are formatted by the host (the plugin never learns the user's
//! timezone).

use rekindle_sdk::consts::channel_kind;
use rekindle_sdk::prelude::*;
use rekindle_sdk::time::TimeStyle;

const MODAL: &str = "server-info";

struct ServerInfo;

fn when(ms: u64) -> String {
    format!(
        "{} ({})",
        time::format(ms, TimeStyle::LongDate),
        time::format(ms, TimeStyle::Relative)
    )
}

fn show(guild: u64) {
    let Some(g) = state::guild(guild) else {
        ui::toast_kind("That server isn't loaded", ToastKind::Warning);
        return;
    };
    let channels = state::channels(guild, true);
    let text = channels
        .iter()
        .filter(|c| c.kind == channel_kind::TEXT || c.kind == channel_kind::ANNOUNCEMENT)
        .count();
    let voice = channels
        .iter()
        .filter(|c| c.kind == channel_kind::VOICE || c.kind == channel_kind::STAGE)
        .count();
    let hidden = channels.iter().filter(|c| !c.visible).count();
    let (_, loaded) = users::loaded_members(guild, 0);
    let mut p = Panel::new(MODAL, &g.name)
        .field(
            "Owner",
            &state::display_name(g.owner).unwrap_or_else(|| g.owner.to_string()),
        )
        .user(g.owner)
        .field("Created", &when(time::snowflake_ms(g.id)))
        .field(
            "Members",
            &g.member_count
                .map_or_else(|| format!("{loaded}+ loaded"), |n| n.to_string()),
        )
        .field(
            "Channels",
            &format!(
                "{text} text, {voice} voice{}",
                if hidden > 0 {
                    format!(", {hidden} hidden")
                } else {
                    String::new()
                }
            ),
        )
        .field("Roles", &state::roles(guild).len().to_string())
        .field(
            "Boosts",
            &format!("{} (tier {})", g.premium_subscribers, g.premium_tier),
        );
    if let Some(ms) = state::my_member(guild)
        .and_then(|m| m.joined_ms)
        .or(g.joined_ms)
    {
        p = p.field("You joined", &when(ms));
    }
    if !g.features.is_empty() {
        p = p.field("Features", &g.features.join(", "));
    }
    p.separator().button("close", "Close").show_modal();
}

impl Plugin for ServerInfo {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        if let Err(e) = slots::add(
            slots::GUILD_MENU,
            MenuItem::new("info", "Server info").icon(Icon::Info),
        ) {
            warn!("no guild menu on this build: {e:?}");
        }
        Ok(ServerInfo)
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        if let SlotEvent::Clicked(SlotClick {
            item,
            target: Target::Guild(g),
            ..
        }) = ev
            && item == "info"
        {
            show(g.id);
        }
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.clicked(MODAL, "close") {
            ui::close_modal();
        }
    }
}

rekindle_sdk::plugin!(ServerInfo);
