use rekindle_plugin_host::api::Transform;
use rekindle_plugin_host::backend::fixtures::{CAROL, GENERAL, LAB, ME};
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/ts/staff-tag");

/// The text of each author badge in `t`.
fn badges(t: &[Transform]) -> Vec<String> {
    t.iter()
        .filter_map(|t| match t {
            Transform::AuthorBadge(b) => b.text.clone(),
            _ => None,
        })
        .collect()
}

#[test]
fn the_servers_owner_gets_an_owner_badge() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    // The Lab's owner is the user.
    let m = h.receive_message(GENERAL, ME, "hi");
    assert_eq!(
        badges(&h.render_message(GENERAL, m.id)),
        ["Owner"],
        "{}",
        h.log_text()
    );
    let m = h.receive_message(GENERAL, CAROL, "hello");
    assert!(badges(&h.render_message(GENERAL, m.id)).is_empty());
}

#[test]
fn a_role_with_administrator_gets_an_admin_badge() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    // Give Carol a role with Administrator, then tell the plugin the roles changed.
    let admin = h.with_world(|w| {
        let mut role = w
            .roles
            .iter()
            .find(|r| r.guild == LAB && r.name == "Moderator")
            .unwrap()
            .clone();
        role.id = 1199;
        role.name = "Admin".into();
        role.permissions |= 1 << 3;
        w.roles.push(role.clone());
        w.members
            .iter_mut()
            .find(|m| m.guild == LAB && m.user == CAROL)
            .unwrap()
            .roles
            .push(role.id);
        role
    });
    h.event(rekindle_plugin_host::api::Event::RoleUpdated(admin));
    let m = h.receive_message(GENERAL, CAROL, "hello");
    assert_eq!(
        badges(&h.render_message(GENERAL, m.id)),
        ["Admin"],
        "{}",
        h.log_text()
    );
}
