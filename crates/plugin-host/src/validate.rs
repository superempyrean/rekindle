//! Checks that an interaction is one a real client could deliver: shared by
//! `rekindle dev`'s API and the test [`Harness`](crate::testing::Harness), so
//! a test can't click a button that isn't on the panel.

use crate::api::Target;
use crate::model::{self, Element, Panel, SlotItem, UiValue};

/// What a slot's items are clicked on, by target kind name.
pub fn slot_target_kind(slot: &str) -> Option<&'static str> {
    Some(match slot {
        model::SLOT_MESSAGE_MENU | model::SLOT_MESSAGE_HOVER_BAR => "message",
        model::SLOT_USER_MENU => "user",
        model::SLOT_CHANNEL_MENU
        | model::SLOT_THREAD_MENU
        | model::SLOT_ATTACH_MENU
        | model::SLOT_CHAT_BAR_BUTTON
        | model::SLOT_CHANNEL_TOOLBAR
        | model::SLOT_VOICE_PANEL_BUTTON
        | model::SLOT_VOICE_PANEL_STATUS => "channel",
        model::SLOT_GUILD_MENU => "guild",
        model::SLOT_ROLE_MENU => "role",
        model::SLOT_MEDIA_MENU | model::SLOT_MEDIA_VIEWER_MENU => "media",
        model::SLOT_LINK_MENU => "link",
        model::SLOT_SELECTION_MENU => "selection",
        model::SLOT_SETTINGS_COG_MENU
        | model::SLOT_USER_AREA_BUTTON
        | model::SLOT_GUILD_LIST_BUTTON
        | model::SLOT_DM_LIST_BUTTON => "none",
        _ => return None,
    })
}

/// A target's kind name, as [`slot_target_kind`] names them.
pub fn target_kind(t: &Target) -> &'static str {
    match t {
        Target::None => "none",
        Target::Message(_) => "message",
        Target::User(_) => "user",
        Target::Channel(_) => "channel",
        Target::Guild(_) => "guild",
        Target::Role(_) => "role",
        Target::Media(_) => "media",
        Target::Link(_) => "link",
        Target::Selection(_) => "selection",
    }
}

/// A real client only offers an item where its slot lives: a message-menu
/// item is clicked on a message, a guild-list button with no target, and a
/// profile section isn't clickable at all.
pub fn check_slot_target(slot: &str, item: &SlotItem, target: &Target) -> Result<(), String> {
    if slot == model::SLOT_PROFILE_SECTION {
        return Err(format!(
            "`{}` is a profile section; it can't be clicked",
            item.id()
        ));
    }
    let want = slot_target_kind(slot).ok_or_else(|| format!("unknown slot `{slot}`"))?;
    let got = target_kind(target);
    if want == got {
        Ok(())
    } else {
        Err(format!(
            "`{}` lives in `{slot}`, which is clicked on a {want}, not a {got}",
            item.id()
        ))
    }
}

/// A real client only sends events for controls that exist: check the event
/// against the panel the plugin last showed.
pub fn check_ui_event(panel: &Panel, element: &str, value: &UiValue) -> Result<(), String> {
    check_elements(&panel.id, &panel.elements, element, value)
}

pub fn check_elements(
    surface: &str,
    elements: &[Element],
    element: &str,
    value: &UiValue,
) -> Result<(), String> {
    let el = elements
        .iter()
        .find(|e| e.id() == Some(element))
        .ok_or_else(|| format!("`{surface}` has no element `{element}`"))?;
    let ok = match (el, value) {
        (Element::Toggle { .. }, UiValue::Toggled(_))
        | (Element::ColorPicker { .. }, UiValue::Color(_))
        | (Element::Button { .. }, UiValue::Clicked)
        | (Element::StringList { .. }, UiValue::Items(_)) => true,
        (Element::Slider { min, max, .. }, UiValue::Number(n)) => {
            if n < min || n > max {
                return Err(format!("{n} is outside the slider's range {min}..={max}"));
            }
            true
        }
        (Element::TextInput { multiline, .. }, UiValue::Text(t)) => {
            if !multiline && t.contains('\n') {
                return Err(format!("`{element}` is a single-line input"));
            }
            true
        }
        (Element::Select { options, .. }, UiValue::Selected(i)) => {
            if *i as usize >= options.len() {
                return Err(format!(
                    "option {i} out of range (select has {})",
                    options.len()
                ));
            }
            true
        }
        (Element::Table { columns, .. }, UiValue::Rows(rows)) => {
            if let Some(r) = rows.iter().find(|r| r.len() != columns.len()) {
                return Err(format!(
                    "a row has {} cells; the table has {} columns",
                    r.len(),
                    columns.len()
                ));
            }
            true
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "element `{element}` doesn't accept a `{}` value",
            value_kind(value)
        ))
    }
}

fn value_kind(v: &UiValue) -> &'static str {
    match v {
        UiValue::Toggled(_) => "toggled",
        UiValue::Number(_) => "number",
        UiValue::Color(_) => "color",
        UiValue::Text(_) => "text",
        UiValue::Selected(_) => "selected",
        UiValue::Clicked => "clicked",
        UiValue::Rows(_) => "rows",
        UiValue::Items(_) => "items",
    }
}
