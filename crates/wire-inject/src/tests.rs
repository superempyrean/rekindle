use std::collections::BTreeSet;

use rekindle_wire::a11y::ids as a11y;
use rekindle_wire::event::Event;
use rekindle_wire::{
    ChildOp, ElementKind, NodeId, NodeOp, PropId, Tree, Turn, TurnReport, Value, ids,
};

use super::*;

/// The cell this test's guest tags its nodes with.
const TAG: PropId = PropId(539);
/// An item's file name.
const FILE: PropId = PropId(249);

const PAGE: NodeId = 5;
const FEED: NodeId = 6;
const HEAD: NodeId = 20;
const RULE: NodeId = 21;

const BOX: NodeId = RESERVED;
const BOX_TEXT: NodeId = RESERVED + 1;
const BUTTON: NodeId = RESERVED + 2;
const BUTTON_TEXT: NodeId = RESERVED + 3;

const PAGE_BG: u32 = 0x102030ff;
const RULE_COLOR: u32 = 0xaa5500ff;
const MARK_BG: u32 = 0x003300ff;

fn str(s: &str) -> Value {
    Value::Str(s.into())
}

fn tag(t: &str) -> Selector {
    Selector::cell(TAG, t)
}

/// The feed: the `list` holding a `head` or a `post`.
fn feed() -> Selector {
    Selector::All(vec![
        Selector::Kind(ElementKind::List),
        Selector::Any(vec![tag("head"), tag("post")]).parent_of(),
    ])
}

fn posts() -> Selector {
    Selector::All(vec![tag("post"), feed().child_of()])
}

/// A post whose body (the `text` of its spans) contains `s`.
fn post_saying(s: &str) -> Selector {
    Selector::All(vec![
        posts(),
        Selector::Text {
            contains: s.into(),
            from: Some(Box::new(tag("body").child_of())),
        },
    ])
}

/// A box with a text and a counting button, under the feed, and the page
/// and rules recoloured.
fn mounts() -> Work {
    Work {
        mounts: vec![Mount {
            under: feed(),
            root: BOX,
            nodes: vec![
                MountNode {
                    id: BOX,
                    kind: ElementKind::Div,
                    cells: vec![Cell::new(ids::BACKGROUND, rgba(0x222222ff))],
                    children: vec![BOX_TEXT, BUTTON],
                    on_press: None,
                },
                MountNode {
                    id: BOX_TEXT,
                    kind: ElementKind::Text,
                    cells: vec![Cell::new(TEXT, str("hello from rekindle"))],
                    children: vec![],
                    on_press: None,
                },
                MountNode {
                    id: BUTTON,
                    kind: ElementKind::Div,
                    cells: vec![Cell::new(a11y::LABEL, str("count"))],
                    children: vec![BUTTON_TEXT],
                    on_press: Some(Handler::Count {
                        node: BUTTON_TEXT,
                        prop: TEXT,
                        one: "1 press".into(),
                        many: "{n} presses".into(),
                    }),
                },
                MountNode {
                    id: BUTTON_TEXT,
                    kind: ElementKind::Text,
                    cells: vec![],
                    children: vec![],
                    on_press: None,
                },
            ],
        }],
        cells: vec![
            Write {
                select: feed().parent_of(),
                cells: vec![Cell::new(ids::BACKGROUND, rgba(PAGE_BG))],
            },
            Write {
                select: Selector::All(vec![tag("rule"), feed().child_of()]),
                cells: vec![Cell::new(ids::COLOR, rgba(RULE_COLOR))],
            },
        ],
        ..Work::default()
    }
}

/// [`mounts`], plus a rule of each kind for posts and presses.
fn everything() -> Work {
    let mut w = mounts();
    w.rows = vec![
        RowRule {
            select: post_saying("drop me"),
            action: RowAction::Drop,
        },
        RowRule {
            select: Selector::All(vec![tag("span"), tag("body").child_of()]),
            action: RowAction::Replace {
                prop: TEXT,
                from: "old".into(),
                to: "new".into(),
            },
        },
        RowRule {
            select: post_saying("mark me"),
            action: RowAction::Cells(vec![Cell::new(ids::BACKGROUND, rgba(MARK_BG))]),
        },
    ];
    w.presses = vec![
        PressRule {
            select: Selector::Label("Stop".into()),
            action: PressAction::Drop,
        },
        PressRule {
            select: Selector::Label("Swap".into()),
            action: PressAction::Retarget(Selector::Label("Stop".into())),
        },
    ];
    w.labels = vec![LabelRule {
        select: tag("file"),
        prefix: "File: ".into(),
        from: FILE,
    }];
    w
}

/// The page, its feed, a head, a rule and two empty posts.
fn first() -> Turn {
    let mut t = Turn::new();
    for (node, kind) in [
        (PAGE, ElementKind::Div),
        (FEED, ElementKind::List),
        (HEAD, ElementKind::Div),
        (RULE, ElementKind::Div),
        (30, ElementKind::Div),
        (31, ElementKind::Div),
    ] {
        t.push(NodeOp::Create { node, kind });
    }
    t.push(NodeOp::SetProps {
        node: PAGE,
        props: vec![(ids::BACKGROUND, rgba(0x313338ff))],
    });
    t.push(NodeOp::SetProps {
        node: HEAD,
        props: vec![(TAG, str("head"))],
    });
    t.push(NodeOp::SetProps {
        node: RULE,
        props: vec![(TAG, str("rule")), (ids::COLOR, rgba(0x949ba4ff))],
    });
    t.push(NodeOp::SetProps {
        node: 30,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::SetProps {
        node: 31,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::SetChildren {
        node: PAGE,
        children: vec![FEED],
    });
    t.push(NodeOp::SetChildren {
        node: FEED,
        children: vec![HEAD, RULE, 30, 31],
    });
    t
}

/// A post with a one-span body: the post is `id`, the body `id + 1` and
/// the span `id + 2`.
fn post(t: &mut Turn, id: NodeId, text: &str) {
    t.push(NodeOp::Create {
        node: id,
        kind: ElementKind::Div,
    });
    t.push(NodeOp::Create {
        node: id + 1,
        kind: ElementKind::Text,
    });
    t.push(NodeOp::Create {
        node: id + 2,
        kind: ElementKind::Text,
    });
    t.push(NodeOp::SetProps {
        node: id,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::SetProps {
        node: id + 1,
        props: vec![(TAG, str("body"))],
    });
    t.push(NodeOp::SetProps {
        node: id + 2,
        props: vec![(TAG, str("span")), (TEXT, str(text))],
    });
    t.push(NodeOp::SetChildren {
        node: id,
        children: vec![id + 1],
    });
    t.push(NodeOp::SetChildren {
        node: id + 1,
        children: vec![id + 2],
    });
}

const DROPPED: NodeId = 40;
const REPLACED: NodeId = 50;
const MARKED: NodeId = 60;
const PLAIN: NodeId = 70;

fn bodies() -> Turn {
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: PAGE,
        kind: ElementKind::Div,
    });
    t.push(NodeOp::Create {
        node: FEED,
        kind: ElementKind::List,
    });
    t.push(NodeOp::Create {
        node: HEAD,
        kind: ElementKind::Div,
    });
    t.push(NodeOp::SetProps {
        node: HEAD,
        props: vec![(TAG, str("head"))],
    });
    post(&mut t, DROPPED, "please drop me now");
    post(&mut t, REPLACED, "the old value");
    post(&mut t, MARKED, "mark me green");
    post(&mut t, PLAIN, "nothing to see");
    t.push(NodeOp::SetChildren {
        node: PAGE,
        children: vec![FEED],
    });
    t.push(NodeOp::SetChildren {
        node: FEED,
        children: vec![HEAD, DROPPED, REPLACED, MARKED, PLAIN],
    });
    t
}

/// Run a guest turn through the engine and apply what comes out, as a host
/// would.
fn send(e: &mut Engine, host: &mut Tree, turn: &Turn) -> (Turn, TurnReport) {
    let bytes = turn.to_bytes();
    let out = e.filter(&bytes).unwrap().unwrap_or(bytes);
    let applied = Turn::from_bytes(&out).unwrap();
    let report = host.apply(&applied);
    (applied, report)
}

fn mentions(turn: &Turn, id: NodeId) -> bool {
    turn.ops.iter().any(|op| match op {
        NodeOp::Create { node, .. } | NodeOp::SetProps { node, .. } | NodeOp::Remove { node } => {
            *node == id
        }
        NodeOp::SetChildren { node, children } => *node == id || children.contains(&id),
    })
}

fn engine(work: Work) -> (Engine, Tree) {
    (Engine::new(work).unwrap(), Tree::new())
}

#[test]
fn a_mount_is_created_under_its_node_and_cells_are_written() {
    let (mut e, mut host) = engine(mounts());
    let (applied, _) = send(&mut e, &mut host, &first());
    assert!(applied.ops.contains(&NodeOp::Create {
        node: BOX,
        kind: ElementKind::Div
    }));
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, 30, 31, BOX]
    );
    assert_eq!(host.node(BOX).unwrap().children, vec![BOX_TEXT, BUTTON]);
    assert_eq!(
        host.props().get(BOX_TEXT, TEXT),
        Some(&str("hello from rekindle"))
    );
    assert_eq!(host.props().get(BUTTON_TEXT, TEXT), Some(&str("0 presses")));
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(PAGE_BG))
    );
    assert_eq!(host.props().get(RULE, ids::COLOR), Some(&rgba(RULE_COLOR)));
}

#[test]
fn a_mount_survives_the_guests_next_list() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: 32,
        kind: ElementKind::Div,
    });
    t.push(NodeOp::SetProps {
        node: 32,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::SetChildren {
        node: FEED,
        children: vec![HEAD, RULE, 30, 31, 32],
    });
    let (applied, report) = send(&mut e, &mut host, &t);

    let lists = applied
        .ops
        .iter()
        .filter(|op| matches!(op, NodeOp::SetChildren { node: FEED, .. }))
        .count();
    assert_eq!(lists, 1);
    let diff = &report.child_ops.iter().find(|(n, _)| *n == FEED).unwrap().1;
    assert!(!diff.contains(&ChildOp::Remove { id: BOX }), "{diff:?}");
    assert!(!mentions(&applied, BUTTON));
}

#[test]
fn a_guest_write_is_written_over_and_a_quiet_turn_passes() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    assert_eq!(e.filter(&Turn::new().to_bytes()).unwrap(), None);
    let mut t = Turn::new();
    t.push(NodeOp::SetProps {
        node: PAGE,
        props: vec![(ids::BACKGROUND, rgba(0xffffffff))],
    });
    let (applied, _) = send(&mut e, &mut host, &t);
    assert_eq!(applied.ops.len(), 2);
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(PAGE_BG))
    );
}

#[test]
fn the_node_coming_back_gets_the_mount_again() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    let mut t = Turn::new();
    for node in [PAGE, FEED, HEAD, RULE, 30, 31] {
        t.push(NodeOp::Remove { node });
    }
    send(&mut e, &mut host, &t);
    assert!(host.node(FEED).is_none());
    let (applied, _) = send(&mut e, &mut host, &first());
    assert!(
        !applied
            .ops
            .iter()
            .any(|op| matches!(op, NodeOp::Create { node: BOX, .. }))
    );
    assert_eq!(host.node(FEED).unwrap().children.last(), Some(&BOX));
}

#[test]
fn a_dropped_node_never_reaches_the_host() {
    let (mut e, mut host) = engine(everything());
    let (applied, _) = send(&mut e, &mut host, &bodies());
    for n in DROPPED..DROPPED + 3 {
        assert!(!mentions(&applied, n), "op on dropped node {n}");
    }
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, REPLACED, MARKED, PLAIN, BOX]
    );
    assert_eq!(
        e.dropped(),
        &BTreeSet::from([DROPPED, DROPPED + 1, DROPPED + 2])
    );

    let mut t = Turn::new();
    t.push(NodeOp::SetProps {
        node: DROPPED,
        props: vec![(ids::BACKGROUND, rgba(0x00000026))],
    });
    t.push(NodeOp::SetChildren {
        node: FEED,
        children: vec![HEAD, DROPPED, REPLACED, PLAIN, MARKED],
    });
    let (applied, report) = send(&mut e, &mut host, &t);
    assert!(!mentions(&applied, DROPPED));
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, REPLACED, PLAIN, MARKED, BOX]
    );
    let diff = &report.child_ops.iter().find(|(n, _)| *n == FEED).unwrap().1;
    assert!(
        !diff.iter().any(|op| matches!(op, ChildOp::Insert { .. })),
        "{diff:?}"
    );
}

#[test]
fn a_node_that_stops_matching_comes_back_and_one_that_starts_is_removed() {
    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &bodies());
    let mut t = Turn::new();
    t.push(NodeOp::SetProps {
        node: DROPPED + 2,
        props: vec![(TEXT, str("edited"))],
    });
    t.push(NodeOp::SetProps {
        node: PLAIN + 2,
        props: vec![(TEXT, str("now drop me"))],
    });
    let (applied, _) = send(&mut e, &mut host, &t);
    assert!(applied.ops.contains(&NodeOp::Create {
        node: DROPPED,
        kind: ElementKind::Div
    }));
    assert_eq!(host.props().get(DROPPED + 2, TEXT), Some(&str("edited")));
    assert_eq!(host.node(DROPPED).unwrap().children, vec![DROPPED + 1]);
    assert!(applied.ops.contains(&NodeOp::Remove { node: PLAIN }));
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, DROPPED, REPLACED, MARKED, BOX]
    );
}

#[test]
fn replaced_text_is_rewritten_inside_the_guests_own_op() {
    let (mut e, mut host) = engine(everything());
    let (applied, _) = send(&mut e, &mut host, &bodies());
    let sets: Vec<_> = applied
        .ops
        .iter()
        .filter(|op| matches!(op, NodeOp::SetProps { node, .. } if *node == REPLACED + 2))
        .collect();
    assert_eq!(
        sets,
        vec![&NodeOp::SetProps {
            node: REPLACED + 2,
            props: vec![(TAG, str("span")), (TEXT, str("the new value"))]
        }]
    );
    // The post's tag isn't a span's, so its cells are left alone.
    let mut t = Turn::new();
    t.push(NodeOp::SetProps {
        node: REPLACED,
        props: vec![(TEXT, str("old"))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(host.props().get(REPLACED, TEXT), Some(&str("old")));
}

#[test]
fn a_marked_node_keeps_its_node_and_carries_rekindles_cells() {
    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &bodies());
    assert_eq!(
        host.props().get(MARKED, ids::BACKGROUND),
        Some(&rgba(MARK_BG))
    );
    assert!(host.props().get(PLAIN, ids::BACKGROUND).is_none());
}

#[test]
fn an_unlabelled_node_gets_a_label_and_a_labelled_one_keeps_its_own() {
    let with_file = |label: Option<&str>| {
        let mut t = first();
        t.push(NodeOp::Create {
            node: 32,
            kind: ElementKind::Div,
        });
        let mut props = vec![(TAG, str("file")), (FILE, str("notes.txt"))];
        if let Some(l) = label {
            props.push((a11y::LABEL, str(l)));
        }
        t.push(NodeOp::SetProps { node: 32, props });
        t.push(NodeOp::SetChildren {
            node: 30,
            children: vec![32],
        });
        t
    };
    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &with_file(None));
    assert_eq!(
        host.props().get(32, a11y::LABEL),
        Some(&str("File: notes.txt"))
    );
    assert_eq!(e.filter(&Turn::new().to_bytes()).unwrap(), None);

    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &with_file(Some("my notes")));
    assert_eq!(host.props().get(32, a11y::LABEL), Some(&str("my notes")));
}

#[test]
fn a_press_on_a_counting_button_is_consumed_and_the_next_turn_shows_it() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    let press = Event::press(BUTTON).to_bytes();
    assert_eq!(e.event(&press), Ok(Verdict::Consume(Some(Value::I32(1)))));
    let (applied, _) = send(&mut e, &mut host, &Turn::new());
    assert_eq!(
        applied.ops,
        vec![NodeOp::SetProps {
            node: BUTTON_TEXT,
            props: vec![(TEXT, str("1 press"))]
        }]
    );
    e.event(&press).unwrap();
    send(&mut e, &mut host, &Turn::new());
    assert_eq!(host.props().get(BUTTON_TEXT, TEXT), Some(&str("2 presses")));
    assert_eq!(e.count(BUTTON), 2);
    // A press on any other of rekindle's nodes is consumed with no reply.
    assert_eq!(
        e.event(&Event::press(BOX_TEXT).to_bytes()),
        Ok(Verdict::Consume(None))
    );
}

#[test]
fn presses_on_the_guests_nodes_are_dropped_retargeted_or_passed() {
    let with_buttons = || {
        let mut t = first();
        for (node, label) in [(80, "Stop"), (81, "Swap"), (82, "Other")] {
            t.push(NodeOp::Create {
                node,
                kind: ElementKind::Div,
            });
            t.push(NodeOp::SetProps {
                node,
                props: vec![(a11y::LABEL, str(label))],
            });
        }
        t.push(NodeOp::SetChildren {
            node: PAGE,
            children: vec![FEED, 80, 81, 82],
        });
        t
    };
    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &with_buttons());
    assert_eq!(
        e.event(&Event::press(80).to_bytes()),
        Ok(Verdict::Consume(None))
    );
    assert_eq!(
        e.event(&Event::press(81).to_bytes()),
        Ok(Verdict::Replace(Event::press(80)))
    );
    assert_eq!(e.event(&Event::press(82).to_bytes()), Ok(Verdict::Pass));
    assert_eq!(
        e.event(&[1, 2]),
        Err(rekindle_wire::DecodeError::UnexpectedEof)
    );

    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &with_buttons());
    for n in [80, 81, 82] {
        assert_eq!(e.event(&Event::press(n).to_bytes()), Ok(Verdict::Pass));
    }
}

#[test]
fn empty_work_leaves_every_turn_alone() {
    let (mut e, _) = engine(Work::default());
    assert!(e.work().is_empty());
    assert_eq!(e.filter(&first().to_bytes()).unwrap(), None);
}

#[test]
fn work_reads_back_from_its_json() {
    let work = everything();
    let json = work.to_json();
    assert!(json.contains("\"background\""), "{json}");
    assert_eq!(Work::from_json(&json).unwrap(), work);
    let colour = Work::from_json(r#"{"cells": [{"select": {"kind": "list"}, "cells": [{"prop": 90, "value": {"rgba": "ff7a18ff"}}]}]}"#).unwrap();
    assert_eq!(
        colour.cells[0].cells[0],
        Cell::new(ids::BACKGROUND, rgba(0xff7a18ff))
    );
}

#[test]
fn work_with_ids_outside_the_reserved_range_is_refused() {
    let mut work = mounts();
    work.mounts[0].nodes[1].id = 7;
    assert!(Engine::new(work).is_err());
    let mut work = mounts();
    work.mounts[0].root = 12;
    assert!(Work::from_json(&work.to_json()).is_err());
    assert!(Work::from_json(r#"{"mount": []}"#).is_err());
}

/// The page's background and the rule's colour as a theme maps them.
fn recolours() -> Work {
    Work {
        recolors: vec![
            Recolor {
                from: 0x31_3338,
                to: 0x0b0d_12ff,
            },
            Recolor {
                from: 0x94_9ba4,
                to: 0xff00_0080,
            },
        ],
        ..Work::default()
    }
}

#[test]
fn a_recolour_replaces_the_guests_colour_wherever_it_is_written() {
    let (mut e, mut host) = engine(recolours());
    send(&mut e, &mut host, &first());
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0x0b0d_12ff))
    );
    // Half the alpha, from `to`'s.
    let Some(Value::Vec4([r, _, _, a])) = host.props().get(RULE, ids::COLOR) else {
        panic!("the rule has no colour");
    };
    assert!((r - 1.0).abs() < f32::EPSILON && (a - 128.0 / 255.0).abs() < 1e-6);
    // A colour no recolour names is left alone.
    let mut t = Turn::new();
    t.push(NodeOp::SetProps {
        node: HEAD,
        props: vec![(ids::BACKGROUND, rgba(0x1234_56ff))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(HEAD, ids::BACKGROUND),
        Some(&rgba(0x1234_56ff))
    );
}

#[test]
fn a_recolour_matches_a_colour_off_by_less_than_a_byte() {
    let (mut e, mut host) = engine(recolours());
    let mut t = first();
    let Value::Vec4([r, g, b, a]) = rgba(0x3133_38ff) else {
        unreachable!()
    };
    t.push(NodeOp::SetProps {
        node: HEAD,
        props: vec![(ids::BORDER_COLOR, Value::Vec4([r + 0.001, g, b - 0.001, a]))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(HEAD, ids::BORDER_COLOR),
        Some(&rgba(0x0b0d_12ff))
    );
}

#[test]
fn new_work_sets_back_what_the_old_work_wrote() {
    let (mut e, mut host) = engine(recolours());
    send(&mut e, &mut host, &first());
    let mut only_rule = recolours();
    only_rule.recolors.remove(0);
    e.set_work(only_rule).unwrap();
    // The next turn, whatever it holds, carries the page's own colour.
    let mut t = Turn::new();
    t.push(NodeOp::SetProps {
        node: HEAD,
        props: vec![(TAG, str("head"))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0x3133_38ff))
    );
    assert_ne!(host.props().get(RULE, ids::COLOR), Some(&rgba(0x949b_a4ff)));
}

/// A quiet turn: the guest writes the head's tag again.
fn quiet() -> Turn {
    let mut t = Turn::new();
    t.push(NodeOp::SetProps {
        node: HEAD,
        props: vec![(TAG, str("head"))],
    });
    t
}

#[test]
fn a_rules_cell_keeps_its_value_over_a_recolour_of_the_same_cell() {
    let mut work = recolours();
    work.rows = vec![RowRule {
        select: feed().parent_of(),
        action: RowAction::Cells(vec![Cell::new(ids::BACKGROUND, rgba(MARK_BG))]),
    }];
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &first());
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(MARK_BG))
    );
    // The rule's value stays, and isn't written again.
    let (applied, _) = send(&mut e, &mut host, &quiet());
    assert!(!mentions(&applied, PAGE));
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(MARK_BG))
    );
    // Without the rule the recolour applies again.
    e.set_work(recolours()).unwrap();
    send(&mut e, &mut host, &quiet());
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0x0b0d_12ff))
    );
}

#[test]
fn a_drop_added_to_running_work_takes_the_subtree_out_and_removing_it_brings_it_back() {
    let (mut e, mut host) = engine(Work::default());
    send(&mut e, &mut host, &first());
    e.set_work(Work {
        rows: vec![RowRule {
            select: feed(),
            action: RowAction::Drop,
        }],
        ..Work::default()
    })
    .unwrap();
    send(&mut e, &mut host, &quiet());
    for n in [FEED, HEAD, RULE, 30, 31] {
        assert!(host.node(n).is_none(), "{n} is still in the host's tree");
    }
    assert!(host.node(PAGE).unwrap().children.is_empty());
    e.set_work(Work::default()).unwrap();
    send(&mut e, &mut host, &quiet());
    assert_eq!(host.node(PAGE).unwrap().children, vec![FEED]);
    assert_eq!(host.node(FEED).unwrap().children, vec![HEAD, RULE, 30, 31]);
    assert_eq!(host.props().get(RULE, ids::COLOR), Some(&rgba(0x949b_a4ff)));
}

/// A plugin's button: a `div` with a `text`, under the feed.
fn plugin_button(root: NodeId, label: &str) -> Mount {
    Mount {
        under: feed(),
        root,
        nodes: vec![
            MountNode {
                id: root,
                kind: ElementKind::Div,
                cells: vec![Cell::new(a11y::LABEL, str(label))],
                children: vec![root + 1],
                on_press: Some(Handler::Plugin {
                    plugin: "dev.example.p".into(),
                    surface: "chat-bar.button".into(),
                    element: label.into(),
                }),
            },
            MountNode {
                id: root + 1,
                kind: ElementKind::Text,
                cells: vec![Cell::new(TEXT, str(label))],
                children: vec![],
                on_press: None,
            },
        ],
    }
}

const PLUGIN: NodeId = RESERVED + 0x100;

fn with_mounts(mounts: Vec<Mount>) -> Work {
    Work {
        mounts,
        ..Work::default()
    }
}

#[test]
fn a_press_for_a_plugin_is_consumed_and_kept_once() {
    let (mut e, mut host) = engine(with_mounts(vec![plugin_button(PLUGIN, "open")]));
    send(&mut e, &mut host, &first());
    assert_eq!(
        e.event(&Event::press(PLUGIN).to_bytes()),
        Ok(Verdict::Consume(None))
    );
    // A press on the button's text has no handler of its own.
    assert_eq!(
        e.event(&Event::press(PLUGIN + 1).to_bytes()),
        Ok(Verdict::Consume(None))
    );
    assert_eq!(
        e.take_presses(),
        vec![PluginPress {
            plugin: "dev.example.p".into(),
            surface: "chat-bar.button".into(),
            element: "open".into(),
        }]
    );
    assert!(e.take_presses().is_empty());
    // Nothing reaches the guest's tree or changes the host's.
    let (applied, _) = send(&mut e, &mut host, &Turn::new());
    assert!(applied.ops.is_empty(), "{:?}", applied.ops);
}

#[test]
fn a_mount_added_to_running_work_is_created_in_the_next_turn() {
    let (mut e, mut host) = engine(Work::default());
    send(&mut e, &mut host, &first());
    e.set_work(with_mounts(vec![plugin_button(PLUGIN, "open")]))
        .unwrap();
    send(&mut e, &mut host, &Turn::new());
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, 30, 31, PLUGIN]
    );
    assert_eq!(host.props().get(PLUGIN + 1, TEXT), Some(&str("open")));
}

#[test]
fn a_mount_left_out_of_new_work_is_taken_out_of_the_host() {
    let a = plugin_button(PLUGIN, "a");
    let b = plugin_button(PLUGIN + 0x10, "b");
    let (mut e, mut host) = engine(with_mounts(vec![a.clone(), b]));
    send(&mut e, &mut host, &first());
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, 30, 31, PLUGIN, PLUGIN + 0x10]
    );
    e.set_work(with_mounts(vec![a])).unwrap();
    let (applied, _) = send(&mut e, &mut host, &Turn::new());
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, 30, 31, PLUGIN]
    );
    for n in [PLUGIN + 0x10, PLUGIN + 0x11] {
        assert!(applied.ops.contains(&NodeOp::Remove { node: n }));
        assert!(host.node(n).is_none());
    }
    // The kept one stays as it was, and a quiet turn after passes.
    assert!(
        !applied
            .ops
            .iter()
            .any(|op| matches!(op, NodeOp::Create { .. }))
    );
    assert_eq!(e.filter(&Turn::new().to_bytes()).unwrap(), None);
    // Empty work takes the last one out too.
    e.set_work(Work::default()).unwrap();
    send(&mut e, &mut host, &Turn::new());
    assert_eq!(host.node(FEED).unwrap().children, vec![HEAD, RULE, 30, 31]);
    assert!(host.node(PLUGIN).is_none());
}

#[test]
fn a_mount_taken_out_while_the_guest_relists_its_node_stays_out() {
    let (mut e, mut host) = engine(with_mounts(vec![plugin_button(PLUGIN, "open")]));
    send(&mut e, &mut host, &first());
    e.set_work(Work::default()).unwrap();
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: 32,
        kind: ElementKind::Div,
    });
    t.push(NodeOp::SetProps {
        node: 32,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::SetChildren {
        node: FEED,
        children: vec![HEAD, RULE, 30, 31, 32],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, 30, 31, 32]
    );
    assert!(host.node(PLUGIN).is_none() && host.node(PLUGIN + 1).is_none());
}

#[test]
fn a_kept_mount_changes_only_its_cells() {
    let (mut e, mut host) = engine(with_mounts(vec![plugin_button(PLUGIN, "open")]));
    send(&mut e, &mut host, &first());
    let mut changed = plugin_button(PLUGIN, "open");
    changed.nodes[1].cells = vec![Cell::new(TEXT, str("opened"))];
    changed.nodes[0].cells.clear();
    e.set_work(with_mounts(vec![changed])).unwrap();
    let (applied, _) = send(&mut e, &mut host, &Turn::new());
    assert!(
        !applied
            .ops
            .iter()
            .any(|op| matches!(op, NodeOp::Create { .. } | NodeOp::Remove { .. })),
        "{:?}",
        applied.ops
    );
    assert_eq!(host.props().get(PLUGIN + 1, TEXT), Some(&str("opened")));
    // A cell is cleared by writing it null, as the guest clears its own.
    assert_eq!(host.props().get(PLUGIN, a11y::LABEL), Some(&Value::Null));
}

#[test]
fn a_mount_whose_shape_changes_is_made_again() {
    let (mut e, mut host) = engine(with_mounts(vec![plugin_button(PLUGIN, "open")]));
    send(&mut e, &mut host, &first());
    let mut bare = plugin_button(PLUGIN, "open");
    bare.nodes[0].children.clear();
    bare.nodes.truncate(1);
    e.set_work(with_mounts(vec![bare])).unwrap();
    let (applied, _) = send(&mut e, &mut host, &Turn::new());
    let at = |want: &NodeOp| applied.ops.iter().position(|op| op == want).unwrap();
    assert!(
        at(&NodeOp::Remove { node: PLUGIN })
            < at(&NodeOp::Create {
                node: PLUGIN,
                kind: ElementKind::Div
            })
    );
    assert!(host.node(PLUGIN + 1).is_none());
    assert!(host.node(PLUGIN).unwrap().children.is_empty());
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, 30, 31, PLUGIN]
    );
}

#[test]
fn recolours_read_back_from_their_json() {
    let work = recolours();
    let json = work.to_json();
    assert!(
        json.contains("\"313338\"") && json.contains("\"0b0d12ff\""),
        "{json}"
    );
    assert_eq!(Work::from_json(&json).unwrap(), work);
    assert!(Work::from_json(r#"{"recolors": [{"from": "31333", "to": "0b0d12ff"}]}"#).is_err());
}
