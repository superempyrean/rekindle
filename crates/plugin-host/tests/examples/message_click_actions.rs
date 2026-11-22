use rekindle_plugin_host::backend::fixtures::{GENERAL, ME};
use rekindle_plugin_host::model::{Action, ClickArea, Mods};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/message-click-actions"
);

fn first_by(h: &Harness, author: u64) -> u64 {
    h.with_world(|w| {
        w.messages[&GENERAL]
            .iter()
            .find(|m| m.author.id == author)
            .unwrap()
            .id
    })
}

fn others(h: &Harness) -> u64 {
    h.with_world(|w| {
        w.messages[&GENERAL]
            .iter()
            .find(|m| m.author.id != ME)
            .unwrap()
            .id
    })
}

const SHIFT: Mods = Mods {
    shift: true,
    ctrl: false,
    alt: false,
    meta: false,
};

#[test]
fn double_click_edits_your_own_and_replies_to_others() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let mine = h.message_ref(GENERAL, first_by(&h, ME));
    let theirs = h.message_ref(GENERAL, others(&h));
    assert_eq!(
        h.message_click(ClickArea::Content, 2, Mods::default(), mine)
            .as_deref(),
        Some(h.id())
    );
    assert_eq!(
        h.message_click(ClickArea::Content, 2, Mods::default(), theirs)
            .as_deref(),
        Some(h.id())
    );
    assert_eq!(
        h.performed(),
        vec![
            Action::StartEdit { message: mine },
            Action::StartReply {
                message: theirs,
                mention: true
            }
        ]
    );
}

#[test]
fn shift_double_click_deletes_without_the_confirm() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let theirs = h.message_ref(GENERAL, others(&h));
    h.message_click(ClickArea::Content, 2, SHIFT, theirs);
    // The client's own rule: Shift skips the confirmation.
    assert_eq!(
        h.performed(),
        vec![Action::DeleteMessage {
            message: theirs,
            confirm: false
        }]
    );
    assert!(h.with_world(|w| w.find_message(GENERAL, theirs.id).is_none()));
}

#[test]
fn other_clicks_are_left_alone() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let theirs = h.message_ref(GENERAL, others(&h));
    assert_eq!(
        h.message_click(ClickArea::Avatar, 2, Mods::default(), theirs),
        None
    );
    assert_eq!(h.message_click(ClickArea::Content, 1, SHIFT, theirs), None);
    assert!(h.performed().is_empty());
}
