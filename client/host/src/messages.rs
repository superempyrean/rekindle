//! Message rows on a client's wire, for the plugins' message renderers.
//!
//! The guest draws each message's content as a node whose
//! `accessibilityId` is a prefix and the message's id, and the content's
//! text as a `text` node under it whose `spans` cell holds the text in
//! runs, each with its own colour. [`shown`] finds the messages in the
//! guest's tree, and [`writes`] turns what the renderers made of one into
//! cells: the spans split where a highlight starts and ends, with the
//! highlight's colour, and the content's background for a tint.
//! Phoenix's rows have no background of their own to tint, so the tint
//! goes on the content.
//!
//! The wire map says where the messages are ([`MessageMap`]). Nothing
//! here names the client.

use rekindle_ui_hook::render::Look;
use rekindle_wire::a11y::ids as a11y;
use rekindle_wire::{ElementKind, PropId, Tree, Value};
use rekindle_wire_inject::{Cell, Selector, TEXT, Write};
use serde::Deserialize;

/// Where the messages are on the wire, in the wire map's `messages`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageMap {
    /// The `accessibilityId` of a message's content, before the message's
    /// id.
    pub(crate) content_prefix: String,
    /// A highlight's colour when its plugin names none the client has,
    /// `0xRRGGBBAA` in hex.
    #[serde(with = "hex")]
    pub(crate) highlight: u32,
}

mod hex {
    use serde::{Deserialize, Deserializer};

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        let s = String::deserialize(d)?;
        u32::from_str_radix(s.trim_start_matches('#'), 16).map_err(serde::de::Error::custom)
    }
}

/// One message the guest shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shown {
    pub(crate) id: u64,
    /// The content as the spans spell it.
    pub(crate) content: String,
    /// The content's `spans` cell, as the guest wrote it.
    pub(crate) spans: Value,
}

use rekindle_wire::ids::COLOR;
use rekindle_wire::turn::SPANS;

/// The messages in `tree`, in node order.
pub(crate) fn shown(tree: &Tree, map: &MessageMap) -> Vec<Shown> {
    let mut out = Vec::new();
    for n in tree.nodes() {
        let Some(Value::Str(aid)) = tree.props().get(n.id, a11y::ID) else {
            continue;
        };
        let Some(id) = aid
            .strip_prefix(&map.content_prefix)
            .and_then(|i| i.parse::<u64>().ok())
        else {
            continue;
        };
        let text = n.children.iter().find_map(|c| {
            let node = tree.node(*c)?;
            (node.kind == ElementKind::TEXT)
                .then(|| tree.props().get(*c, SPANS))
                .flatten()
        });
        if let Some(v @ Value::Array(runs)) = text {
            out.push(Shown {
                id,
                content: runs.iter().filter_map(run_text).collect(),
                spans: v.clone(),
            });
        }
    }
    out
}

fn field(run: &Value, prop: PropId) -> Option<&Value> {
    let Value::Object(fields) = run else {
        return None;
    };
    fields.iter().find(|(p, _)| *p == prop).map(|(_, v)| v)
}

fn run_text(run: &Value) -> Option<&str> {
    match field(run, TEXT)? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

/// The cells that give message `m` the look the renderers made: its spans
/// with the highlights, and its content's background for a tint. None when
/// the look changes neither.
pub(crate) fn writes(map: &MessageMap, m: &Shown, look: &Look) -> Vec<Write> {
    let content = Selector::cell(a11y::ID, &format!("{}{}", map.content_prefix, m.id));
    let mut out = Vec::new();
    if !look.highlights.is_empty() {
        let ranges: Vec<(usize, usize, u32)> = look
            .highlights
            .iter()
            .map(|&(s, e, c)| (s, e, if c == 0 { map.highlight } else { c }))
            .collect();
        out.push(Write {
            select: Selector::All(vec![
                Selector::Kind(ElementKind::TEXT),
                content.clone().child_of(),
            ]),
            cells: vec![Cell::new(SPANS, highlight(&m.spans, &ranges))],
        });
    }
    // A tint in a colour the client lacks is 0, and left out.
    if look.tint != 0 {
        out.push(Write {
            select: content,
            cells: vec![Cell::new(
                rekindle_wire::ids::BACKGROUND,
                rekindle_wire_inject::rgba(look.tint),
            )],
        });
    }
    out
}

/// `spans` with each byte range of the joined text in its colour
/// (`0xRRGGBBAA`): a run a range starts or ends inside is split there, and
/// the part inside gets the colour. A range that isn't on a character
/// boundary is left out.
pub(crate) fn highlight(spans: &Value, ranges: &[(usize, usize, u32)]) -> Value {
    let Value::Array(runs) = spans else {
        return spans.clone();
    };
    let mut out = Vec::new();
    let mut at = 0;
    for run in runs {
        let Some(text) = run_text(run) else {
            out.push(run.clone());
            continue;
        };
        let (start, end) = (at, at + text.len());
        at = end;
        // The points inside this run where the colour changes.
        let mut cuts = vec![start, end];
        for &(s, e, _) in ranges {
            for p in [s, e] {
                if p > start && p < end && text.is_char_boundary(p - start) {
                    cuts.push(p);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let piece = &text[a - start..b - start];
            let lit = ranges
                .iter()
                .rev()
                .find(|&&(s, e, _)| s <= a && b <= e)
                .map(|&(_, _, c)| c);
            let Value::Object(fields) = run else {
                continue;
            };
            let mut fields: Vec<(PropId, Value)> = fields
                .iter()
                .map(|(p, v)| {
                    let v = if *p == TEXT {
                        Value::Str(piece.into())
                    } else {
                        v.clone()
                    };
                    (*p, v)
                })
                .collect();
            if let Some(c) = lit {
                fields.retain(|(p, _)| *p != COLOR);
                fields.push((COLOR, rekindle_wire_inject::rgba(c)));
            }
            out.push(Value::Object(fields));
        }
    }
    Value::Array(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, color: u32) -> Value {
        Value::Object(vec![
            (TEXT, Value::Str(text.into())),
            (COLOR, rekindle_wire_inject::rgba(color)),
        ])
    }

    fn map() -> MessageMap {
        serde_json::from_str(r#"{"content_prefix": "message.content:", "highlight": "f0b232ff"}"#)
            .unwrap()
    }

    #[test]
    fn a_highlight_splits_the_runs_it_crosses() {
        let spans = Value::Array(vec![
            run("the rekin", 0xfbfbfbff),
            run("dle build", 0xaaaaaaff),
        ]);
        let out = highlight(&spans, &[(4, 12, 0xf0b232ff)]);
        let Value::Array(runs) = &out else { panic!() };
        let parts: Vec<(&str, Value)> = runs
            .iter()
            .map(|r| (run_text(r).unwrap(), field(r, COLOR).unwrap().clone()))
            .collect();
        let c = rekindle_wire_inject::rgba;
        assert_eq!(
            parts,
            [
                ("the ", c(0xfbfbfbff)),
                ("rekin", c(0xf0b232ff)),
                ("dle", c(0xf0b232ff)),
                (" build", c(0xaaaaaaff)),
            ]
        );
        // Nothing to highlight, nothing changed.
        assert_eq!(highlight(&spans, &[]), spans);
    }

    #[test]
    fn messages_are_found_by_their_contents_id() {
        use rekindle_wire::{NodeOp, Turn};
        let mut tree = Tree::new();
        let spans = Value::Array(vec![run("hello rekindle", 0xfbfbfbff)]);
        tree.apply(&Turn {
            ops: vec![
                NodeOp::Create {
                    node: 1,
                    kind: ElementKind::VIEW,
                    props: vec![(a11y::ID, Value::Str("message.content:42".into()))],
                },
                NodeOp::Create {
                    node: 2,
                    kind: ElementKind::TEXT,
                    props: vec![(SPANS, spans.clone())],
                },
                NodeOp::Children {
                    node: 1,
                    children: vec![2],
                },
            ],
        });
        let found = shown(&tree, &map());
        assert_eq!(
            found,
            [Shown {
                id: 42,
                content: "hello rekindle".into(),
                spans
            }]
        );
        let look = Look {
            highlights: vec![(6, 14, 0)],
            tint: 0,
            ..Look::default()
        };
        let w = writes(&map(), &found[0], &look);
        assert_eq!(w.len(), 1, "a highlight without a tint");
        let Value::Array(runs) = &w[0].cells[0].value else {
            panic!()
        };
        assert_eq!(
            field(&runs[1], COLOR),
            Some(&rekindle_wire_inject::rgba(0xf0b232ff)),
            "the map's colour stands in for one the client lacks"
        );
        let tinted = writes(
            &map(),
            &found[0],
            &Look {
                tint: 0x112233ff,
                ..Look::default()
            },
        );
        assert_eq!(
            tinted[0].cells[0].value,
            rekindle_wire_inject::rgba(0x112233ff)
        );
    }
}
