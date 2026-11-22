//! Output rules for the render isolate (design doc §6.2): nothing that
//! leaves the isolate may carry content to anywhere but the user's screen.
//!
//! * No URLs: span targets that are links or media are dropped (a link a
//!   user might click, or the host might load, would be a channel out).
//! * Accessory buttons may only use element ids the main instance declared
//!   in advance (`render-control.declare-actions`): a free-form id would
//!   carry content to the main instance on click.
//! * Images are the plugin's registered assets, by id.
//! * Sizes are capped; highlight ranges are clamped to the content.

use crate::api::{self, Transform};
use crate::bindings::wit;

use super::state::{Shared, clean, clean_multiline};

const MAX_TRANSFORMS: usize = 16;
const MAX_SPANS: usize = 200;
const MAX_ACCESSORY: usize = 32;

fn spans(v: Vec<api::Span>) -> Vec<api::Span> {
    v.into_iter()
        .take(MAX_SPANS)
        .map(|mut s| {
            if matches!(s.target, Some(api::Target::Link(_) | api::Target::Media(_))) {
                s.target = None;
            }
            s.text = clean_multiline(&s.text, 4000);
            s
        })
        .collect()
}

fn badge(mut b: api::Badge, sh: &Shared) -> api::Badge {
    if b.image.as_ref().is_some_and(|i| !sh.images.contains_key(i)) {
        b.image = None;
    }
    b.text = b.text.and_then(|t| clean(&t, 24));
    b.tooltip = clean(&b.tooltip, 200).unwrap_or_default();
    b
}

fn element(e: wit::types::Element, sh: &Shared) -> Option<wit::types::Element> {
    use wit::types::Element as E;
    Some(match e {
        E::Button(b) if !sh.declared_actions.contains(&b.id) => return None,
        E::Toggle(t) if !sh.declared_actions.contains(&t.id) => return None,
        E::Select(s) if !sh.declared_actions.contains(&s.id) => return None,
        // Inputs would let the user type into something the isolate reads
        // back; no free text from accessories.
        E::TextInput(_) | E::Table(_) | E::StringList(_) | E::Slider(_) | E::ColorPicker(_) => {
            return None;
        }
        E::Image((id, _, _)) if !sh.images.contains_key(&id) => return None,
        E::RichText(v) => E::RichText(spans(v)),
        other => other,
    })
}

pub(super) fn sanitize(ts: Vec<Transform>, content: &str, sh: &Shared) -> Vec<Transform> {
    ts.into_iter()
        .take(MAX_TRANSFORMS)
        .filter_map(|t| {
            Some(match t {
                Transform::Collapse(reason) => {
                    Transform::Collapse(clean(&reason, 200).unwrap_or_else(|| "hidden".into()))
                }
                Transform::ReplaceContent(v) => Transform::ReplaceContent(spans(v)),
                Transform::Append(v) => Transform::Append(spans(v)),
                Transform::CodeBlock((n, v)) => Transform::CodeBlock((n, spans(v))),
                Transform::Highlight(hs) => Transform::Highlight(
                    hs.into_iter()
                        .take(MAX_SPANS)
                        .filter_map(|mut h| {
                            h.start = content.floor_char_boundary(h.start as usize) as u32;
                            h.end = content.floor_char_boundary(h.end as usize) as u32;
                            h.tooltip = h.tooltip.and_then(|t| clean(&t, 200));
                            (h.start < h.end).then_some(h)
                        })
                        .collect(),
                ),
                Transform::AuthorBadge(b) => Transform::AuthorBadge(badge(b, sh)),
                Transform::ReplaceEmbed((n, mut e)) => {
                    e.title = e.title.and_then(|t| clean(&t, 256));
                    e.footer = e.footer.and_then(|t| clean(&t, 256));
                    e.description = e.description.map(spans);
                    if e.image.as_ref().is_some_and(|i| !sh.images.contains_key(i)) {
                        e.image = None;
                    }
                    Transform::ReplaceEmbed((n, e))
                }
                Transform::Accessory(es) => {
                    let es: Vec<_> = es
                        .into_iter()
                        .take(MAX_ACCESSORY)
                        .filter_map(|e| element(e, sh))
                        .collect();
                    if es.is_empty() {
                        return None;
                    }
                    Transform::Accessory(es)
                }
                other => other,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, target: Option<api::Target>) -> api::Span {
        api::Span {
            text: text.into(),
            bold: false,
            italic: false,
            code: false,
            color: None,
            target,
        }
    }

    #[test]
    fn strips_urls_undeclared_buttons_and_clamps() {
        let mut sh = Shared::default();
        sh.declared_actions.insert("translate".into());
        let button = |id: &str| {
            wit::types::Element::Button(wit::types::Button {
                id: id.into(),
                label: "x".into(),
                icon: None,
                danger: false,
            })
        };
        let out = sanitize(
            vec![
                Transform::Append(vec![span(
                    "see",
                    Some(api::Target::Link("https://evil.example/?c=secret".into())),
                )]),
                Transform::Accessory(vec![button("translate"), button("exfil-hello-world")]),
                Transform::Highlight(vec![api::Highlight {
                    start: 1,
                    end: 99,
                    color: None,
                    bold: true,
                    tooltip: None,
                }]),
                Transform::Accessory(vec![button("nope")]),
            ],
            "héllo",
            &sh,
        );
        assert_eq!(
            out.len(),
            3,
            "an accessory with nothing allowed in it is dropped"
        );
        let Transform::Append(s) = &out[0] else {
            panic!()
        };
        assert!(s[0].target.is_none());
        let Transform::Accessory(es) = &out[1] else {
            panic!()
        };
        assert_eq!(es.len(), 1);
        let Transform::Highlight(h) = &out[2] else {
            panic!()
        };
        assert_eq!((h[0].start, h[0].end), (1, 6));
    }

    #[test]
    fn an_embed_patch_keeps_to_the_output_rules() {
        let mut sh = Shared::default();
        sh.images.insert("ok".into(), (16, 16));
        let patch = |image: &str| wit::render_types::EmbedPatch {
            title: Some("t\u{7}itle".into()),
            description: Some(vec![span(
                "x",
                Some(api::Target::Link("https://evil.example/".into())),
            )]),
            footer: None,
            color: None,
            image: Some(image.into()),
            hide_media: false,
        };
        let out = sanitize(
            vec![
                Transform::ReplaceEmbed((0, patch("ok"))),
                Transform::ReplaceEmbed((1, patch("nope"))),
            ],
            "",
            &sh,
        );
        let Transform::ReplaceEmbed((0, a)) = &out[0] else {
            panic!()
        };
        assert_eq!(
            a.title.as_deref(),
            Some("title"),
            "control characters are stripped"
        );
        assert!(
            a.description.as_ref().unwrap()[0].target.is_none(),
            "no links"
        );
        assert_eq!(a.image.as_deref(), Some("ok"));
        let Transform::ReplaceEmbed((1, b)) = &out[1] else {
            panic!()
        };
        assert_eq!(b.image, None, "only registered images");
    }
}
