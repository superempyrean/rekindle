use super::*;
use crate::backend::fixtures::{ALICE, BOT, DM_ALICE, GAMERS, GENERAL, LAB, ME, STAFF_ONLY};
use crate::model::{Icon, SLOT_CHAT_BAR_BUTTON, SLOT_USER_MENU};

fn col(h: u32) -> ThemeValue {
    ThemeValue::Color(Color::hex(h))
}

#[test]
fn overrides_layer_and_unwind() {
    let mut b = MockBackend::new();
    let t = "background-brand";
    let base = b.get_color(t).unwrap();
    b.set_value("a", t, col(0x111111)).unwrap();
    b.set_value("b", t, col(0x222222)).unwrap();
    assert_eq!(b.get_color(t), Some(Color::hex(0x222222)));
    assert_eq!(b.override_owner(t), Some("b"));
    b.plugin_unloaded("b");
    assert_eq!(b.get_color(t), Some(Color::hex(0x111111)));
    b.reset_value("a", t);
    assert_eq!(b.get_color(t), Some(base));
    assert_eq!(
        b.set_value("a", "no-such-token", col(0)),
        Err(ThemeError::UnknownToken)
    );
    assert_eq!(
        b.set_value("a", "radius-md", col(0)),
        Err(ThemeError::WrongKind)
    );
    b.set_value("a", "radius-md", ThemeValue::Length(0.0))
        .unwrap();
    assert_eq!(b.get_value("radius-md"), Some(ThemeValue::Length(0.0)));
}

#[test]
fn tint_applies_to_base_surfaces_only() {
    let mut b = MockBackend::new();
    let surface = b.get_color("background-base-low").unwrap();
    let link = b.get_color("text-link").unwrap();
    b.set_tint(
        "a",
        Some(Tint {
            color: Color::hex(0xff0080),
            amount: 0.2,
            text_amount: 0.0,
        }),
    );
    assert_ne!(b.get_color("background-base-low"), Some(surface));
    assert_eq!(
        b.get_color("text-link"),
        Some(link),
        "tint role none is untouched"
    );
    // An explicit override beats the tint.
    b.set_value("b", "background-base-low", col(0x000000))
        .unwrap();
    assert_eq!(
        b.get_color("background-base-low"),
        Some(Color::hex(0x000000))
    );
    b.plugin_unloaded("a");
    b.plugin_unloaded("b");
    assert_eq!(b.get_color("background-base-low"), Some(surface));
}

#[test]
fn fonts_must_be_registered_by_the_plugin() {
    let mut b = MockBackend::new();
    let f = ThemeValue::Font("Fancy".into());
    assert_eq!(
        b.set_value("a", "font-primary", f.clone()),
        Err(ThemeError::OutOfRange)
    );
    b.register_font("a", "Fancy", vec![0; 10]).unwrap();
    assert!(
        b.set_value("b", "font-primary", f.clone()).is_err(),
        "another plugin's font"
    );
    b.set_value("a", "font-primary", f).unwrap();
    // The client's own fonts are always allowed.
    b.set_value("b", "font-code", ThemeValue::Font("gg sans".into()))
        .unwrap();
}

#[test]
fn slots_respect_availability_kind_and_unwind() {
    let mut b = MockBackend::new();
    let btn = SlotItem::IconButton {
        id: "b".into(),
        icon: Icon::Star,
        image: None,
        tooltip: "Star".into(),
        active: false,
    };
    b.add_slot_item("a", SLOT_CHAT_BAR_BUTTON, btn.clone())
        .unwrap();
    b.add_slot_item("a", SLOT_CHAT_BAR_BUTTON, btn.clone())
        .unwrap(); // replace, not duplicate
    assert_eq!(b.slot_items(SLOT_CHAT_BAR_BUTTON).len(), 1);
    assert_eq!(
        b.add_slot_item("a", SLOT_USER_MENU, btn.clone()),
        Err(SlotError::Invalid),
        "a button in a menu"
    );
    b.set_slot_available(SLOT_CHAT_BAR_BUTTON, false);
    assert!(b.slot_items(SLOT_CHAT_BAR_BUTTON).is_empty());
    assert_eq!(
        b.add_slot_item("a", SLOT_CHAT_BAR_BUTTON, btn),
        Err(SlotError::Unavailable)
    );
    let menu = SlotItem::MenuItem {
        id: "m".into(),
        label: "Hi".into(),
        icon: None,
        danger: false,
        group: None,
        checked: None,
        when: vec![],
    };
    b.add_slot_item("a", SLOT_USER_MENU, menu).unwrap();
    b.set_profile_section("a", "m", 7, vec![]);
    b.show_modal(
        "a",
        Panel {
            id: "x".into(),
            title: "X".into(),
            elements: vec![],
        },
    );
    b.plugin_unloaded("a");
    assert!(b.slot_items(SLOT_USER_MENU).is_empty() && b.modal("a").is_none());
    assert!(b.profile_section("a", "m", 7).is_none());
}

#[test]
fn panels_replace_by_id_and_clear_on_unload() {
    let mut b = MockBackend::new();
    let p = |t: &str| Panel {
        id: "s".into(),
        title: t.into(),
        elements: vec![],
    };
    b.show_panel("a", p("one"));
    b.show_panel("a", p("two"));
    b.show_panel("b", p("other"));
    assert_eq!(b.panels().count(), 2);
    assert_eq!(b.panel("a", "s").unwrap().title, "two");
    b.register_command("a", "go", "Go");
    b.plugin_unloaded("a");
    assert_eq!(b.panels().count(), 1);
    assert!(b.commands().is_empty());
}

#[test]
fn sample_world_answers_queries() {
    let b = MockBackend::new();
    assert_eq!(b.me().unwrap().id, ME);
    assert_eq!(b.guild_ids(), vec![LAB, GAMERS]);
    let visible: Vec<_> = b.channels(LAB, false).into_iter().map(|c| c.name).collect();
    assert!(!visible.contains(&"staff-only".to_string()));
    assert!(
        b.channels(LAB, true)
            .iter()
            .any(|c| c.id == STAFF_ONLY && !c.visible)
    );
    assert_eq!(b.dm_channel(ALICE), Some(DM_ALICE));
    assert_eq!(b.loaded_messages(GENERAL, None, 2).len(), 2);
    assert_eq!(
        b.presence(ALICE, None).unwrap().client_status.mobile,
        "idle"
    );
    assert_eq!(b.mutual_guilds(ALICE), vec![LAB, GAMERS]);
    assert_eq!(b.member_color(LAB, BOT), Some(Color::hex(0x5865f2)));
    assert!(b.guild_read_state(LAB).unread);
    assert!(
        b.raw_json(&api::Target::Guild(api::GuildRef {
            id: LAB,
            name: String::new()
        }))
        .unwrap()
        .contains("Rekindle Lab")
    );
}

#[test]
fn member_permissions_apply_the_channels_overwrites() {
    let b = MockBackend::new();
    let can_view = |channel| b.member_permissions(ALICE, channel).unwrap() & VIEW_CHANNEL != 0;
    assert!(can_view(LAB) && can_view(GENERAL));
    assert!(!can_view(STAFF_ONLY), "@everyone is denied staff-only");
    assert_eq!(b.member_permissions(ME, STAFF_ONLY), Some(ALL_PERMISSIONS));
    assert_eq!(b.member_permissions(4242, LAB), None, "not a member");
}

#[test]
fn actions_change_the_world() {
    let mut b = MockBackend::new();
    let first = b.world.messages[&GENERAL][0].clone();
    let r = message_ref(&first);
    b.perform(
        "p",
        Action::React {
            message: r,
            emoji: "👋".into(),
            burst: false,
        },
    )
    .unwrap();
    assert_eq!(b.world.messages[&GENERAL][0].reactions[0].count, 3);
    b.perform(
        "p",
        Action::Unreact {
            message: r,
            emoji: "👋".into(),
            burst: false,
        },
    )
    .unwrap();
    assert_eq!(b.world.messages[&GENERAL][0].reactions[0].count, 2);
    b.perform("p", Action::MarkAllRead).unwrap();
    assert!(b.unread_channels(10).is_empty());
    assert_eq!(
        b.perform(
            "p",
            Action::EditMessage {
                message: r,
                content: "x".into()
            }
        ),
        Err(ActionError::Forbidden)
    );
    assert_eq!(b.performed.len(), 3);
}

#[test]
fn tweaks_layer_and_user_wins() {
    let mut b = MockBackend::new();
    let value = |b: &MockBackend| {
        b.tweaks()
            .into_iter()
            .find(|t| t.id == "silent-typing")
            .unwrap()
            .value
    };
    b.set_tweak("a", "silent-typing", "true").unwrap();
    assert_eq!(value(&b), "true");
    assert_eq!(
        b.set_tweak("a", "silent-typing", "maybe"),
        Err(TweakError::InvalidValue)
    );
    b.set_tweak_by_user("silent-typing", Some("false"));
    assert_eq!(
        b.set_tweak("a", "silent-typing", "true"),
        Err(TweakError::UserLocked)
    );
    assert_eq!(value(&b), "false");
}
