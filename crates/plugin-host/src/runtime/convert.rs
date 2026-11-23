//! Conversions between generated WIT types and the host's model types, and
//! the event-kind table.

use crate::api;
use crate::bindings::wit;
use crate::capability::Capability;
use crate::model::{self, Mods};
use crate::settings::SettingValue;

use super::state::{clean, clean_multiline};

/// Map identically named enum variants across two enums.
macro_rules! map_enum {
    ($name:ident: $from:ty => $to:ty { $($v:ident),* $(,)? }) => {
        pub(super) fn $name(x: $from) -> $to {
            match x { $(<$from>::$v => <$to>::$v),* }
        }
    };
}

map_enum!(icon_from_wit: wit::types::Icon => model::Icon {
    Star, Bookmark, Note, Palette, Bolt, Heart, Bell, Gear, Link, Search, Sparkle, Eye, EyeOff, Clock, Pin, Puzzle,
    Translate, Microphone, Headphones, Camera, Shield, Lock, User, Users, Chat, Reply, Copy, Trash, Edit, Check, Cross,
    Info, Warning, Download, Upload, Globe, Code, Hash, At, Smile, Image, Music, Play, Pause,
});

map_enum!(predicate_from_wit: wit::ui::MenuPredicate => model::MenuPredicate {
    OwnMessage, OthersMessage, HasAttachments, HasEmbeds, InGuild, InDm, TargetIsBot, TargetIsSelf, TargetIsFriend,
    HasContent, CanManageMessages, CanPin, CanEdit, CanReply, CanReact, CanDelete,
});

map_enum!(click_area_from_wit: wit::ui::ClickArea => model::ClickArea { Content, Avatar, AuthorName, Timestamp });

map_enum!(toast_from_wit: wit::ui::ToastKind => model::ToastKind { Info, Success, Warning, Error });

map_enum!(ui_error_to_wit: model::UiError => wit::ui::UiError { Unavailable, Invalid, Limit, NoGesture, NotFound, NotGranted });

map_enum!(action_error_to_wit: model::ActionError => wit::action_types::ActionError {
    NoGesture, Forbidden, NotFound, RateLimited, Unavailable, Declined, Invalid, Failed,
});

map_enum!(tweak_kind_to_wit: model::TweakKind => wit::tweaks::TweakKind { Toggle, Choice, Number });

map_enum!(tweak_error_to_wit: model::TweakError => wit::tweaks::TweakError { Unknown, Unavailable, InvalidValue, UserLocked });

map_enum!(file_error_to_wit: model::FileError => wit::files::FileError { NoGesture, Cancelled, TooLarge, Failed });

pub(super) fn cap_to_wit(c: Capability) -> wit::types::Capability {
    use wit::types::Capability as W;
    match c {
        Capability::ReadClient => W::ReadClient,
        Capability::ReadUsers => W::ReadUsers,
        Capability::ReadMessages => W::ReadMessages,
        Capability::RenderMessages => W::RenderMessages,
        Capability::Pointer => W::Pointer,
        Capability::Ui => W::Ui,
        Capability::Theming => W::Theming,
        Capability::Overlay => W::Overlay,
        Capability::Audio => W::Audio,
        Capability::Notifications => W::Notifications,
        Capability::Storage => W::Storage,
        Capability::Network => W::Network,
        Capability::Clipboard => W::Clipboard,
        Capability::Compose => W::Compose,
        Capability::ActLocal => W::ActLocal,
        Capability::React => W::React,
        Capability::Act => W::Act,
        Capability::Presence => W::Presence,
        Capability::Tweaks => W::Tweaks,
        Capability::Files => W::Files,
        Capability::Intercept => W::Intercept,
        Capability::Moderate => W::Moderate,
        Capability::History => W::History,
        Capability::View => W::View,
        Capability::ReadAccount => W::ReadAccount,
        Capability::InterceptHttp => W::InterceptHttp,
    }
}

pub(super) fn color_to_wit(c: model::Color) -> wit::types::Color {
    wit::types::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

pub(super) fn color_from_wit(c: wit::types::Color) -> model::Color {
    model::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

pub(super) fn mods_from_wit(m: wit::types::Modifiers) -> Mods {
    Mods {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.meta,
    }
}

pub(super) fn mods_to_wit(m: Mods) -> wit::types::Modifiers {
    wit::types::Modifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.meta,
    }
}

/// A span from the plugin. In the render isolate, `isolate` drops link and
/// media targets (no URLs out of the isolate).
pub(super) fn span_from_wit(s: wit::types::Span, isolate: bool) -> model::Span {
    let target = s
        .target
        .filter(|t| !(isolate && matches!(t, api::Target::Link(_) | api::Target::Media(_))));
    model::Span {
        text: clean_multiline(&s.text, 2000),
        bold: s.bold,
        italic: s.italic,
        code: s.code,
        color: s.color.map(|c| match c {
            wit::types::SpanColor::Token(t) => {
                model::SpanColor::Token(t.chars().take(64).collect())
            }
            wit::types::SpanColor::Rgba(c) => model::SpanColor::Rgba(color_from_wit(c)),
        }),
        target,
    }
}

pub(super) fn element_from_wit(e: wit::types::Element) -> model::Element {
    use model::Element as E;
    use wit::types::Element as W;
    let label = |s: String| clean(&s, 200).unwrap_or_default();
    match e {
        W::Heading(text) => E::Heading { text: label(text) },
        W::Text(text) => E::Text {
            text: clean_multiline(&text, 4000),
        },
        W::RichText(spans) => E::RichText {
            spans: spans
                .into_iter()
                .take(200)
                .map(|s| span_from_wit(s, false))
                .collect(),
        },
        W::Separator => E::Separator,
        W::Toggle(t) => E::Toggle {
            id: t.id,
            label: label(t.label),
            value: t.value,
        },
        W::Slider(s) => E::Slider {
            id: s.id,
            label: label(s.label),
            min: s.min,
            max: s.max,
            step: s.step,
            value: s.value,
        },
        W::ColorPicker(c) => E::ColorPicker {
            id: c.id,
            label: label(c.label),
            value: color_from_wit(c.value),
        },
        W::TextInput(t) => E::TextInput {
            id: t.id,
            label: label(t.label),
            value: clean_multiline(&t.value, 4000),
            placeholder: label(t.placeholder),
            multiline: t.multiline,
        },
        W::Button(b) => E::Button {
            id: b.id,
            label: label(b.label),
            icon: b.icon.map(icon_from_wit),
            danger: b.danger,
        },
        W::Select(s) => E::Select {
            id: s.id,
            label: label(s.label),
            options: s.options.into_iter().take(100).map(label).collect(),
            selected: s.selected,
        },
        W::Table(t) => E::Table {
            id: t.id,
            label: label(t.label),
            columns: t.columns.into_iter().take(8).map(label).collect(),
            rows: t.rows.into_iter().take(500).collect(),
        },
        W::StringList(l) => E::StringList {
            id: l.id,
            label: label(l.label),
            items: l.items.into_iter().take(500).collect(),
            placeholder: label(l.placeholder),
        },
        W::Image((image, width, height)) => E::Image {
            image,
            width: width.min(1024),
            height: height.min(1024),
        },
        W::User(id) => E::User { id },
        W::Field((k, v)) => E::Field {
            key: label(k),
            value: clean_multiline(&v, 1000),
        },
    }
}

/// Elements for a host surface (capped at 256).
pub(super) fn elements_from_wit(es: Vec<wit::types::Element>) -> Vec<model::Element> {
    es.into_iter().take(256).map(element_from_wit).collect()
}

pub(super) fn panel_from_wit(p: wit::ui::Panel) -> model::Panel {
    model::Panel {
        id: p.id,
        title: clean(&p.title, 200).unwrap_or_default(),
        elements: elements_from_wit(p.elements),
    }
}

pub(super) fn slot_item_from_wit(i: wit::ui::SlotItem) -> model::SlotItem {
    use wit::ui::SlotItem as W;
    match i {
        W::MenuItem(m) => model::SlotItem::MenuItem {
            id: m.id,
            label: m.label,
            icon: m.icon.map(icon_from_wit),
            danger: m.danger,
            group: m.group.and_then(|g| clean(&g, 32)),
            checked: m.checked,
            when: m.when.into_iter().map(predicate_from_wit).collect(),
        },
        W::IconButton(b) => model::SlotItem::IconButton {
            id: b.id,
            icon: icon_from_wit(b.icon),
            image: b.image,
            tooltip: b.tooltip,
            active: b.active,
        },
        W::ListItem(l) => model::SlotItem::ListItem {
            id: l.id,
            title: l.title,
        },
    }
}

pub(super) fn decoration_from_wit(d: wit::ui::Decoration) -> model::Decoration {
    model::Decoration {
        icon: d.icon.map(icon_from_wit),
        image: d.image,
        text: d.text.and_then(|t| clean(&t, 32)),
        tooltip: d.tooltip.and_then(|t| clean(&t, 200)),
        color: d.color.map(color_from_wit),
        clickable: d.clickable,
    }
}

pub(super) fn ui_event_to_wit(e: &model::UiEvent) -> wit::types::UiEvent {
    use model::UiValue as V;
    use wit::types::UiValue as W;
    wit::types::UiEvent {
        surface: e.surface.clone(),
        element: e.element.clone(),
        value: match &e.value {
            V::Toggled(b) => W::Toggled(*b),
            V::Number(n) => W::Number(*n),
            V::Color(c) => W::Color(color_to_wit(*c)),
            V::Text(t) => W::Text(t.clone()),
            V::Selected(i) => W::Selected(*i),
            V::Clicked => W::Clicked,
            V::Rows(r) => W::Rows(r.clone()),
            V::Items(i) => W::Items(i.clone()),
        },
    }
}

pub(super) fn theme_value_to_wit(v: model::ThemeValue) -> wit::theming::ThemeValue {
    use wit::theming::ThemeValue as W;
    match v {
        model::ThemeValue::Color(c) => W::Color(color_to_wit(c)),
        model::ThemeValue::Length(x) => W::Length(x),
        model::ThemeValue::Font(f) => W::Font(f),
        model::ThemeValue::Choice(k) => W::Choice(k),
    }
}

pub(super) fn theme_value_from_wit(v: wit::theming::ThemeValue) -> model::ThemeValue {
    use wit::theming::ThemeValue as W;
    match v {
        W::Color(c) => model::ThemeValue::Color(color_from_wit(c)),
        W::Length(x) => model::ThemeValue::Length(x),
        W::Font(f) => model::ThemeValue::Font(f),
        W::Choice(k) => model::ThemeValue::Choice(k),
    }
}

pub(super) fn setting_to_wit(v: SettingValue) -> wit::settings::SettingValue {
    use wit::settings::SettingValue as W;
    match v {
        SettingValue::Boolean(b) => W::Boolean(b),
        SettingValue::Number(n) => W::Number(n),
        SettingValue::Text(t) => W::Text(t),
        SettingValue::Selected(i) => W::Selected(i),
        SettingValue::Color(c) => W::Color(color_to_wit(c)),
        SettingValue::Items(i) => W::Items(i),
        SettingValue::Rows(r) => W::Rows(r),
        SettingValue::Keybind(k) => W::Keybind(k),
        SettingValue::Ids(i) => W::Ids(i),
    }
}

pub(super) fn setting_from_wit(v: wit::settings::SettingValue) -> SettingValue {
    use wit::settings::SettingValue as W;
    match v {
        W::Boolean(b) => SettingValue::Boolean(b),
        W::Number(n) => SettingValue::Number(n),
        W::Text(t) => SettingValue::Text(t),
        W::Selected(i) => SettingValue::Selected(i),
        W::Color(c) => SettingValue::Color(color_from_wit(c)),
        W::Items(i) => SettingValue::Items(i),
        W::Rows(r) => SettingValue::Rows(r),
        W::Keybind(k) => SettingValue::Keybind(k),
        W::Ids(i) => SettingValue::Ids(i),
    }
}

/// The kind an event is subscribed under (`None` for `dropped`, which is
/// always delivered).
pub(super) fn event_kind(e: &api::Event) -> Option<api::EventKind> {
    use api::Event as E;
    use api::EventKind as K;
    Some(match e {
        E::SelectionChanged(_) => K::SelectionChanged,
        E::Connection(_) => K::Connection,
        E::Window(_) => K::Window,
        E::ReadState(_) => K::ReadState,
        E::GuildJoined(_) | E::GuildUpdated(_) | E::GuildLeft(_) => K::Guild,
        E::ChannelCreated(_) | E::ChannelUpdated(_) | E::ChannelDeleted(_) => K::Channel,
        E::RoleUpdated(_) | E::RoleDeleted(_) => K::Role,
        E::EmojiUpdated(_) => K::Emoji,
        E::MyVoice(_) | E::MyVoiceLeft => K::MyVoice,
        E::MyPresence(_) => K::MyPresence,
        E::MySettings(_) => K::MySettings,
        E::ReplyStarted(_) | E::EditStarted(_) => K::Composer,
        E::Hook(_) => K::Hook,
        E::AccountSwitched(_) => K::Account,
        E::Presence(_) => K::Presence,
        E::Typing(_) => K::Typing,
        E::VoiceState(_) | E::VoiceLeft(_) => K::VoiceState,
        E::Speaking(_) => K::Speaking,
        E::MemberJoined(_) | E::MemberUpdated(_) | E::MemberLeft(_) => K::Member,
        E::Relationship(_) => K::Relationship,
        E::MessageCreated(_)
        | E::MessageEdited(_)
        | E::MessageDeleted(_)
        | E::MessagesBulkDeleted(_) => K::Message,
        E::Reaction(_) | E::ReactionsCleared(_) => K::Reaction,
        E::PollVote(_) => K::PollVote,
        E::Pins(_) => K::Pins,
        E::Raw(_) => K::Raw,
        E::PointerMoved(_) => K::Pointer,
        E::Dropped(_) => return None,
    })
}

/// The capability needed to receive events of a kind.
pub(super) fn kind_capability(kind: api::EventKind) -> Capability {
    use api::EventKind as K;
    match kind {
        K::SelectionChanged
        | K::Connection
        | K::Window
        | K::ReadState
        | K::Guild
        | K::Channel
        | K::Role
        | K::Emoji
        | K::MyVoice
        | K::MyPresence
        | K::MySettings
        | K::Composer
        | K::Hook
        | K::Account => Capability::ReadClient,
        K::Presence | K::Typing | K::VoiceState | K::Speaking | K::Member | K::Relationship => {
            Capability::ReadUsers
        }
        K::Message | K::Reaction | K::PollVote | K::Pins | K::Raw => Capability::ReadMessages,
        K::Pointer => Capability::Pointer,
    }
}
