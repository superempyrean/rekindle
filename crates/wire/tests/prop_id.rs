//! The recovered 855-entry `PropId` table.

use rekindle_wire::{PropId, ids};

#[test]
fn count_is_855() {
    assert_eq!(PropId::COUNT, 855);
    // The last valid id is 854; 855 is out of range (the host's bound 0x357).
    assert!(PropId(854).is_valid());
    assert!(!PropId(855).is_valid());
    assert_eq!(PropId(855).name(), None);
}

#[test]
fn known_ids_resolve_to_names() {
    assert_eq!(PropId(90).name(), Some("background"));
    assert_eq!(PropId(104).name(), Some("borderColor"));
    assert_eq!(PropId(129).name(), Some("color"));
    assert_eq!(PropId(522).name(), Some("tint"));
    // Endpoints of the id->name table.
    assert_eq!(PropId(0).name(), Some("absolute"));
    assert_eq!(PropId(854).name(), Some("lineSpacingExtra"));
}

#[test]
fn from_name_binary_searches_back() {
    assert_eq!(PropId::from_name("background"), Some(PropId(90)));
    assert_eq!(PropId::from_name("color"), Some(PropId(129)));
    assert_eq!(PropId::from_name("tint"), Some(PropId(522)));
    assert_eq!(PropId::from_name("borderColor"), Some(PropId(104)));
    assert_eq!(PropId::from_name("not_a_prop"), None);
}

/// id -> name -> id is identity for every id (the two tables agree, as the
/// binary's cross-check found).
#[test]
fn id_name_id_is_identity() {
    for raw in 0..PropId::COUNT {
        let id = PropId(raw);
        let name = id.name().expect("every id in range has a name");
        assert_eq!(
            PropId::from_name(name),
            Some(id),
            "{name} did not map back to {raw}"
        );
    }
}

#[test]
fn named_recolour_constants_match() {
    assert_eq!(ids::BACKGROUND, PropId(90));
    assert_eq!(ids::COLOR, PropId(129));
    assert_eq!(ids::TINT, PropId(522));
    assert_eq!(ids::BORDER_COLOR, PropId(104));
    assert_eq!(ids::SELECTION_COLOR.name(), Some("selectionColor"));
    assert_eq!(ids::ACTIVE_BACKGROUND.name(), Some("activeBackground"));
}
