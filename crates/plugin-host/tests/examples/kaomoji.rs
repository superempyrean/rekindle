use rekindle_plugin_host::MockBackend;
use rekindle_plugin_host::api::{CommandReply, OptionValue};
use rekindle_plugin_host::model::{Element, SLOT_CHAT_BAR_BUTTON, SlotItem};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/kaomoji");

/// The faces on the modal's buttons, in order.
fn faces(h: &Harness) -> Vec<String> {
    let modal = h.modal().expect("modal shown");
    modal
        .elements
        .iter()
        .filter_map(|e| match e {
            Element::Button { id, label, .. } if id.starts_with("face-") => Some(label.clone()),
            _ => None,
        })
        .collect()
}

fn composer(h: &Harness) -> String {
    h.backend().world.composer.clone()
}

fn open(h: &mut Harness) {
    let here = h.current_channel();
    h.click_slot("open", here);
}

#[test]
fn registers_a_button_a_keybind_and_a_command() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    assert!(
        matches!(&h.slot_items(SLOT_CHAT_BAR_BUTTON)[..], [SlotItem::IconButton { id, tooltip, .. }] if id == "open" && tooltip == "Kaomoji")
    );
    assert_eq!(h.keybinds().len(), 1);
    assert_eq!(h.slash_commands()[0].name, "kaomoji");
}

#[test]
fn picking_a_face_inserts_it_and_closes_the_picker() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    open(&mut h);
    // Nothing used yet: the first category, not the empty recent list.
    assert!(matches!(
        &h.modal().unwrap().elements[1],
        Element::Select { selected: 1, .. }
    ));
    let first = faces(&h)[0].clone();
    h.click("kaomoji", "face-0");
    assert_eq!(composer(&h), first);
    assert!(h.modal().is_none());

    // Next time it opens on the recently used faces.
    open(&mut h);
    assert!(matches!(
        &h.modal().unwrap().elements[1],
        Element::Select { selected: 0, .. }
    ));
    assert_eq!(faces(&h), vec![first.clone()]);

    // And they survive a restart.
    h.restart().unwrap();
    open(&mut h);
    assert_eq!(faces(&h), vec![first]);
}

#[test]
fn categories_and_search() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    open(&mut h);
    let options = match &h.modal().unwrap().elements[1] {
        Element::Select { options, .. } => options.clone(),
        e => panic!("not the category list: {e:?}"),
    };
    let shrug = options.iter().position(|o| o == "Shrug").unwrap() as u32;
    h.select("kaomoji", "category", shrug);
    assert!(faces(&h).contains(&r"¯\_(ツ)_/¯".to_string()));

    // Searching hides the categories and searches all of them.
    h.type_text("kaomoji", "search", "table flip");
    assert!(
        !h.modal()
            .unwrap()
            .elements
            .iter()
            .any(|e| matches!(e, Element::Select { .. }))
    );
    let found = faces(&h);
    assert!(
        !found.is_empty() && found.iter().all(|f| f.contains('┻')),
        "{found:?}"
    );
    // A category name matches too.
    h.type_text("kaomoji", "search", "anim");
    assert!(faces(&h).contains(&"ʕ •ᴥ•ʔ".to_string()));

    h.type_text("kaomoji", "search", "zzzzzz");
    assert!(faces(&h).is_empty());
    assert!(
        h.modal()
            .unwrap()
            .elements
            .iter()
            .any(|e| matches!(e, Element::Text { text } if text.contains("No kaomoji")))
    );
}

#[test]
fn the_keybind_opens_the_picker() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.keybind("open");
    assert_eq!(h.modal().unwrap().title, "Kaomoji");
}

#[test]
fn slash_command_with_autocomplete() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let suggested = h.autocomplete("kaomoji", "search", "shrug");
    assert!(suggested.iter().any(|(_, v)| v == r"¯\_(ツ)_/¯"));
    // A picked suggestion arrives as the face itself.
    let reply = h.slash(
        "kaomoji",
        vec![("search", OptionValue::Text(suggested[1].1.clone()))],
    );
    assert!(matches!(reply, CommandReply::Insert(ref f) if *f == suggested[1].1));
    // A word inserts the first match.
    assert!(
        matches!(h.slash("kaomoji", vec![("search", OptionValue::Text("bear".into()))]), CommandReply::Insert(f) if f.contains('ᴥ'))
    );
    assert!(matches!(
        h.slash("kaomoji", vec![("search", OptionValue::Text("qqq".into()))]),
        CommandReply::Local(_)
    ));
    // Both count as used.
    open(&mut h);
    assert_eq!(faces(&h).len(), 2);
}

#[test]
fn works_without_the_chat_bar_button() {
    let mut b = MockBackend::new();
    b.set_slot_available(SLOT_CHAT_BAR_BUTTON, false);
    let mut h = Harness::builder(PLUGIN).backend(b).start().unwrap();
    h.assert_active();
    assert!(h.log_text().contains("no chat-bar button"));
    h.keybind("open");
    assert!(h.modal().is_some());
}
