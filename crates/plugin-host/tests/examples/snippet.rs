use rekindle_plugin_host::api::{CommandReply, OptionValue};
use rekindle_plugin_host::backend::fixtures::GENERAL;
use rekindle_plugin_host::model::Action;
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/ts/snippet");

#[test]
fn sends_the_code_as_a_file_named_for_its_language() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let code = "fn main() {\n    println!(\"hi\");\n}\n";
    let reply = h.slash(
        "snippet",
        vec![
            ("code", OptionValue::Text(code.into())),
            ("language", OptionValue::Text("Rust".into())),
        ],
    );
    assert_eq!(reply, CommandReply::None, "{}", h.log_text());
    let sent = h.performed();
    let [
        Action::SendMessage {
            channel,
            content,
            files,
        },
    ] = &sent[..]
    else {
        panic!("{sent:?}")
    };
    assert_eq!((*channel, content.as_str()), (GENERAL, ""));
    assert_eq!(files.len(), 1);
    assert_eq!(
        (files[0].filename.as_str(), files[0].data.as_slice()),
        ("snippet.rs", code.as_bytes())
    );
    // The mock posted it, with the attachment.
    let last = h.with_world(|w| w.messages[&GENERAL].last().cloned().unwrap());
    assert_eq!(
        last.attachments
            .iter()
            .map(|a| a.filename.as_str())
            .collect::<Vec<_>>(),
        ["snippet.rs"]
    );
}

#[test]
fn an_unknown_language_is_a_text_file_and_nothing_is_not_sent() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    h.slash(
        "snippet",
        vec![
            ("code", OptionValue::Text("x".into())),
            ("language", OptionValue::Text("brainfreeze".into())),
        ],
    );
    assert!(
        matches!(&h.performed()[..], [Action::SendMessage { files, .. }] if files[0].filename == "snippet.txt")
    );
    let reply = h.slash("snippet", vec![("code", OptionValue::Text("  ".into()))]);
    assert!(matches!(reply, CommandReply::Local(_)), "{reply:?}");
    assert_eq!(h.performed().len(), 1);
}
