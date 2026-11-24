use std::collections::{BTreeMap, BTreeSet};

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
        Selector::Kind(ElementKind::LIST),
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
            after: None,
            before: None,
            root: BOX,
            nodes: vec![
                MountNode {
                    id: BOX,
                    kind: ElementKind::VIEW,
                    cells: vec![Cell::new(ids::BACKGROUND, rgba(0x222222ff))],
                    children: vec![BOX_TEXT, BUTTON],
                    on_press: None,
                },
                MountNode {
                    id: BOX_TEXT,
                    kind: ElementKind::TEXT,
                    cells: vec![Cell::new(TEXT, str("hello from rekindle"))],
                    children: vec![],
                    on_press: None,
                },
                MountNode {
                    id: BUTTON,
                    kind: ElementKind::VIEW,
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
                    kind: ElementKind::TEXT,
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
        (PAGE, ElementKind::VIEW),
        (FEED, ElementKind::LIST),
        (HEAD, ElementKind::VIEW),
        (RULE, ElementKind::VIEW),
        (30, ElementKind::VIEW),
        (31, ElementKind::VIEW),
    ] {
        t.push(NodeOp::Create {
            node,
            kind,
            props: vec![],
        });
    }
    t.push(NodeOp::Update {
        node: PAGE,
        props: vec![(ids::BACKGROUND, rgba(0x313338ff))],
    });
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(TAG, str("head"))],
    });
    t.push(NodeOp::Update {
        node: RULE,
        props: vec![(TAG, str("rule")), (ids::COLOR, rgba(0x949ba4ff))],
    });
    t.push(NodeOp::Update {
        node: 30,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::Update {
        node: 31,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::Children {
        node: PAGE,
        children: vec![FEED],
    });
    t.push(NodeOp::Children {
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
        kind: ElementKind::VIEW,
        props: vec![],
    });
    t.push(NodeOp::Create {
        node: id + 1,
        kind: ElementKind::TEXT,
        props: vec![],
    });
    t.push(NodeOp::Create {
        node: id + 2,
        kind: ElementKind::TEXT,
        props: vec![],
    });
    t.push(NodeOp::Update {
        node: id,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::Update {
        node: id + 1,
        props: vec![(TAG, str("body"))],
    });
    t.push(NodeOp::Update {
        node: id + 2,
        props: vec![(TAG, str("span")), (TEXT, str(text))],
    });
    t.push(NodeOp::Children {
        node: id,
        children: vec![id + 1],
    });
    t.push(NodeOp::Children {
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
        kind: ElementKind::VIEW,
        props: vec![],
    });
    t.push(NodeOp::Create {
        node: FEED,
        kind: ElementKind::LIST,
        props: vec![],
    });
    t.push(NodeOp::Create {
        node: HEAD,
        kind: ElementKind::VIEW,
        props: vec![],
    });
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(TAG, str("head"))],
    });
    post(&mut t, DROPPED, "please drop me now");
    post(&mut t, REPLACED, "the old value");
    post(&mut t, MARKED, "mark me green");
    post(&mut t, PLAIN, "nothing to see");
    t.push(NodeOp::Children {
        node: PAGE,
        children: vec![FEED],
    });
    t.push(NodeOp::Children {
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
        NodeOp::Create { node, .. } | NodeOp::Update { node, .. } => *node == id,
        NodeOp::Delete { nodes } => nodes.contains(&id),
        NodeOp::Children { node, children } => *node == id || children.contains(&id),
        _ => false,
    })
}

fn creates(turn: &Turn, id: NodeId) -> bool {
    turn.ops
        .iter()
        .any(|op| matches!(op, NodeOp::Create { node, .. } if *node == id))
}

fn engine(work: Work) -> (Engine, Tree) {
    (Engine::new(work).unwrap(), Tree::new())
}

#[test]
fn a_mount_is_created_under_its_node_and_cells_are_written() {
    let (mut e, mut host) = engine(mounts());
    let (applied, _) = send(&mut e, &mut host, &first());
    assert!(creates(&applied, BOX));
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
        kind: ElementKind::VIEW,
        props: vec![],
    });
    t.push(NodeOp::Update {
        node: 32,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::Children {
        node: FEED,
        children: vec![HEAD, RULE, 30, 31, 32],
    });
    let (applied, report) = send(&mut e, &mut host, &t);

    let lists = applied
        .ops
        .iter()
        .filter(|op| matches!(op, NodeOp::Children { node: FEED, .. }))
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
    t.push(NodeOp::Update {
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
fn a_mount_goes_with_its_node_and_comes_back_with_the_next_one() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    let mut t = Turn::new();
    for node in [PAGE, FEED, HEAD, RULE, 30, 31] {
        t.push(NodeOp::Delete { nodes: vec![node] });
    }
    let (applied, _) = send(&mut e, &mut host, &t);
    // Out of its node's list, then deleted, before the guest's own ops.
    assert_eq!(
        applied.ops[0],
        NodeOp::Children {
            node: FEED,
            children: vec![HEAD, RULE, 30, 31],
        }
    );
    assert_eq!(
        applied.ops[1],
        NodeOp::Delete {
            nodes: vec![BOX, BOX_TEXT, BUTTON, BUTTON_TEXT],
        }
    );
    assert!(host.node(FEED).is_none() && host.node(BOX).is_none());
    assert!(e.mirror().node(BOX).is_none());

    let (applied, _) = send(&mut e, &mut host, &first());
    assert!(creates(&applied, BOX));
    assert_eq!(host.node(FEED).unwrap().children.last(), Some(&BOX));
    assert_eq!(host.props().get(BUTTON_TEXT, TEXT), Some(&str("0 presses")));
}

#[test]
fn a_mount_stays_when_a_turn_deletes_other_nodes() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    let mut t = Turn::new();
    t.push(NodeOp::Children {
        node: FEED,
        children: vec![HEAD, RULE, 30],
    });
    t.push(NodeOp::Delete { nodes: vec![31] });
    let (applied, _) = send(&mut e, &mut host, &t);
    assert!(
        !applied
            .ops
            .iter()
            .any(|op| matches!(op, NodeOp::Delete { nodes } if nodes.contains(&BOX)))
    );
    assert_eq!(host.node(FEED).unwrap().children, vec![HEAD, RULE, 30, BOX]);
}

#[test]
fn a_mount_goes_after_the_child_its_selector_finds() {
    let mut work = mounts();
    work.mounts[0].after = Some(tag("rule"));
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &first());
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, BOX, 30, 31]
    );
    // The guest's next list keeps it there.
    let mut t = Turn::new();
    t.push(NodeOp::Children {
        node: FEED,
        children: vec![HEAD, RULE, 31],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(host.node(FEED).unwrap().children, vec![HEAD, RULE, BOX, 31]);

    // With no such child, it goes last.
    let mut work = mounts();
    work.mounts[0].after = Some(tag("nothing"));
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &first());
    assert_eq!(host.node(FEED).unwrap().children.last(), Some(&BOX));
}

#[test]
fn a_mount_goes_before_the_child_its_selector_finds() {
    let mut work = mounts();
    work.mounts[0].before = Some(tag("rule"));
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &first());
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, BOX, RULE, 30, 31]
    );
    // The guest's next list keeps it there.
    let mut t = Turn::new();
    t.push(NodeOp::Children {
        node: FEED,
        children: vec![HEAD, RULE, 31],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(host.node(FEED).unwrap().children, vec![HEAD, BOX, RULE, 31]);

    // Before the first child, it goes first.
    let mut work = mounts();
    work.mounts[0].before = Some(tag("head"));
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &first());
    assert_eq!(host.node(FEED).unwrap().children.first(), Some(&BOX));

    // With no such child, `after` places it, then the end.
    let mut work = mounts();
    work.mounts[0].before = Some(tag("nothing"));
    work.mounts[0].after = Some(tag("head"));
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &first());
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, BOX, RULE, 30, 31]
    );
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
    t.push(NodeOp::Update {
        node: DROPPED,
        props: vec![(ids::BACKGROUND, rgba(0x00000026))],
    });
    t.push(NodeOp::Children {
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
fn a_patch_of_a_dropped_nodes_words_still_decodes() {
    let words = PropId(740);
    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &bodies());
    let mut t = Turn::new();
    t.push(NodeOp::Update {
        node: DROPPED,
        props: vec![(words, Value::U32Array(vec![1, 2, 3, 4]))],
    });
    t.push(NodeOp::Update {
        node: PLAIN,
        props: vec![(
            words,
            Value::Sparse {
                list: 0,
                pairs: vec![(0, 7)],
            },
        )],
    });
    let (applied, _) = send(&mut e, &mut host, &t);
    assert!(!mentions(&applied, DROPPED));
    assert_eq!(
        host.props().get(PLAIN, words),
        Some(&Value::U32Array(vec![7, 2, 3, 4]))
    );
}

#[test]
fn a_node_that_stops_matching_comes_back_and_one_that_starts_is_removed() {
    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &bodies());
    let mut t = Turn::new();
    t.push(NodeOp::Update {
        node: DROPPED + 2,
        props: vec![(TEXT, str("edited"))],
    });
    t.push(NodeOp::Update {
        node: PLAIN + 2,
        props: vec![(TEXT, str("now drop me"))],
    });
    let (applied, _) = send(&mut e, &mut host, &t);
    assert!(creates(&applied, DROPPED));
    assert_eq!(host.props().get(DROPPED + 2, TEXT), Some(&str("edited")));
    assert_eq!(host.node(DROPPED).unwrap().children, vec![DROPPED + 1]);
    assert!(applied.ops.contains(&NodeOp::Delete {
        nodes: vec![PLAIN, PLAIN + 1, PLAIN + 2]
    }));
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
        .filter(|op| matches!(op, NodeOp::Update { node, .. } if *node == REPLACED + 2))
        .collect();
    assert_eq!(
        sets,
        vec![&NodeOp::Update {
            node: REPLACED + 2,
            props: vec![(TAG, str("span")), (TEXT, str("the new value"))]
        }]
    );
    // The post's tag isn't a span's, so its cells are left alone.
    let mut t = Turn::new();
    t.push(NodeOp::Update {
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
            kind: ElementKind::VIEW,
            props: vec![],
        });
        let mut props = vec![(TAG, str("file")), (FILE, str("notes.txt"))];
        if let Some(l) = label {
            props.push((a11y::LABEL, str(l)));
        }
        t.push(NodeOp::Update { node: 32, props });
        t.push(NodeOp::Children {
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
        vec![NodeOp::Update {
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

/// [`first`] with three buttons the press rules of [`everything`] match:
/// 80 is dropped, 81 retargeted at 80, and 82 passes.
fn with_buttons() -> Turn {
    let mut t = first();
    for (node, label) in [(80, "Stop"), (81, "Swap"), (82, "Other")] {
        t.push(NodeOp::Create {
            node,
            kind: ElementKind::VIEW,
            props: vec![],
        });
        t.push(NodeOp::Update {
            node,
            props: vec![(a11y::LABEL, str(label))],
        });
    }
    t.push(NodeOp::Children {
        node: PAGE,
        children: vec![FEED, 80, 81, 82],
    });
    t
}

#[test]
fn presses_on_the_guests_nodes_are_dropped_retargeted_or_passed() {
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
        e.event(&[0, 0, 0, 0, 1]),
        Err(rekindle_wire::DecodeError::UnexpectedEof)
    );
    assert_eq!(e.event(&[1, 0, 0, 0]), Ok(Verdict::Pass), "a host frame");
    assert_eq!(
        e.event(&[0, 0, 0, 0, 1, 7]),
        Ok(Verdict::Pass),
        "not an event on a node"
    );

    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &with_buttons());
    for n in [80, 81, 82] {
        assert_eq!(e.event(&Event::press(n).to_bytes()), Ok(Verdict::Pass));
    }
}

/// A BATCH of `entries`, with a record the engine has to keep.
fn batch(entries: &[&[u8]]) -> Vec<u8> {
    frame::Frame {
        kind: frame::BATCH,
        module: 2,
        method: 5,
        call: 9,
        record: Value::Bool(true).to_bytes(),
        body: frame::batch_body(entries.iter().copied()),
    }
    .to_bytes()
}

/// A host module's EVENT frame.
fn module_event() -> Vec<u8> {
    frame::Frame {
        kind: frame::EVENT,
        module: 3,
        method: 1,
        call: 0,
        record: Value::I32(4).to_bytes(),
        body: vec![0xaa, 0xbb],
    }
    .to_bytes()
}

#[test]
fn presses_inside_a_batch_are_filtered_and_its_frames_pass() {
    let (mut e, mut host) = engine(everything());
    send(&mut e, &mut host, &with_buttons());
    let press = |n| Event::press(n).to_bytes();
    let event = module_event();

    // 80 is dropped, 81 becomes a press on 80, and 82 and the EVENT frame
    // stay where they were.
    let sent = batch(&[&press(80), &event, &press(81), &press(82)]);
    let Ok(Verdict::Batch(out)) = e.event(&sent) else {
        panic!("the batch wasn't rewritten");
    };
    let out = frame::Frame::from_bytes(&out).unwrap();
    let want = frame::Frame::from_bytes(&batch(&[&event, &press(80), &press(82)])).unwrap();
    assert_eq!(out, want, "the header and record are kept");
    assert_eq!(
        out.batch().unwrap(),
        vec![&event[..], &press(80)[..], &press(82)[..]]
    );

    // Nothing to filter: the batch goes through as sent.
    assert_eq!(e.event(&batch(&[&press(82), &event])), Ok(Verdict::Pass));
    assert_eq!(e.event(&batch(&[])), Ok(Verdict::Pass));
    // Every entry dropped: the guest receives nothing.
    assert_eq!(
        e.event(&batch(&[&press(80), &press(80)])),
        Ok(Verdict::Consume(None))
    );
    // A batch inside a batch goes back through the same entry.
    let inner = batch(&[&press(81)]);
    let Ok(Verdict::Batch(out)) = e.event(&batch(&[&inner, &event])) else {
        panic!("the nested batch wasn't rewritten");
    };
    let out = frame::Frame::from_bytes(&out).unwrap();
    let entries = out.batch().unwrap();
    assert_eq!(entries[1], &event[..]);
    assert_eq!(
        frame::Frame::from_bytes(entries[0])
            .unwrap()
            .batch()
            .unwrap(),
        vec![&press(80)[..]]
    );
    // An entry that runs past the end, and an event in it that doesn't
    // decode, are errors.
    let mut cut = batch(&[&press(82)]);
    cut.pop();
    assert!(e.event(&cut).is_err());
    assert!(e.event(&batch(&[&[0, 0, 0, 0, 1]])).is_err());
}

#[test]
fn a_counting_press_inside_a_batch_counts_without_a_reply() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    let event = module_event();
    let press = Event::press(BUTTON).to_bytes();
    let Ok(Verdict::Batch(out)) = e.event(&batch(&[&press, &event])) else {
        panic!("the batch wasn't rewritten");
    };
    assert_eq!(
        frame::Frame::from_bytes(&out).unwrap().batch().unwrap(),
        vec![&event[..]]
    );
    assert_eq!(e.count(BUTTON), 1);
    let (applied, _) = send(&mut e, &mut host, &Turn::new());
    assert_eq!(
        applied.ops,
        vec![NodeOp::Update {
            node: BUTTON_TEXT,
            props: vec![(TEXT, str("1 press"))]
        }]
    );
}

#[test]
fn a_frame_request_carries_what_the_engine_changed() {
    let (mut e, mut host) = engine(mounts());
    send(&mut e, &mut host, &first());
    let request = Turn {
        ops: vec![NodeOp::RequestFrame],
    };
    assert_eq!(e.filter(&request.to_bytes()), Ok(None));
    e.event(&Event::press(BUTTON).to_bytes()).unwrap();
    let (applied, _) = send(&mut e, &mut host, &request);
    assert_eq!(
        applied.ops,
        vec![
            NodeOp::RequestFrame,
            NodeOp::Update {
                node: BUTTON_TEXT,
                props: vec![(TEXT, str("1 press"))]
            }
        ]
    );
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
            Recolor::new(0x31_3338, 0x0b0d_12ff),
            Recolor::new(0x94_9ba4, 0xff00_0080),
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
    assert_eq!(
        host.props().get(RULE, ids::COLOR).and_then(Value::as_color),
        Some(0x80ff_0000)
    );
    // A colour no recolour names is left alone.
    let mut t = Turn::new();
    t.push(NodeOp::Update {
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
fn a_recolour_with_a_role_reaches_only_its_cells() {
    // The light theme's surface is white, and so is the label on its brand
    // button: the surface token recolours the fill, the text token the text.
    let work = Work {
        recolors: vec![
            Recolor {
                role: ColorRole::Fill,
                ..Recolor::new(0xff_ffff, 0x0b0d_12ff)
            },
            Recolor {
                role: ColorRole::Content,
                ..Recolor::new(0x31_3338, 0xd6dc_e8ff)
            },
        ],
        ..Work::default()
    };
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &themed(0xffff_ffff, 0xffff_ffff));
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0x0b0d_12ff))
    );
    assert_eq!(host.props().get(RULE, ids::COLOR), Some(&rgba(0xffff_ffff)));
    let mut t = Turn::new();
    t.push(NodeOp::Update {
        node: RULE,
        props: vec![(ids::COLOR, rgba(0x3133_38ff))],
    });
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(ids::BACKGROUND, rgba(0x3133_38ff))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(host.props().get(RULE, ids::COLOR), Some(&rgba(0xd6dc_e8ff)));
    assert_eq!(
        host.props().get(HEAD, ids::BACKGROUND),
        Some(&rgba(0x3133_38ff))
    );
}

#[test]
fn a_recolour_for_the_cells_role_applies_over_one_for_every_cell() {
    let work = Work {
        recolors: vec![
            Recolor {
                role: ColorRole::Fill,
                ..Recolor::new(0x31_3338, 0x0b0d_12ff)
            },
            Recolor::new(0x31_3338, 0xff00_00ff),
        ],
        ..Work::default()
    };
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &themed(0x3133_38ff, 0x3133_38ff));
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0x0b0d_12ff))
    );
    assert_eq!(host.props().get(RULE, ids::COLOR), Some(&rgba(0xff00_00ff)));
}

#[test]
fn a_recolour_reaches_gradient_stops_and_text_spans_by_role() {
    let work = Work {
        recolors: vec![
            Recolor {
                role: ColorRole::Fill,
                ..Recolor::new(0xfb_fbfb, 0x0b0d_12ff)
            },
            Recolor {
                role: ColorRole::Content,
                ..Recolor::new(0x28_282d, 0xd6dc_e8ff)
            },
        ],
        ..Work::default()
    };
    let (mut e, mut host) = engine(work);
    let span = |c: u32| {
        Value::Object(vec![
            (PropId(499), Value::Str("type".into())),
            (ids::COLOR, Value::color(c)),
        ])
    };
    let mut t = first();
    t.push(NodeOp::Update {
        node: PAGE,
        props: vec![(
            ids::BACKGROUND_GRADIENT,
            Value::Array(vec![Value::color(0x00fb_fbfb), Value::color(0xfffb_fbfb)]),
        )],
    });
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(
            ids::BACKGROUND_GRADIENT,
            Value::U32Array(vec![0x00fb_fbfb, 0x8028_282d]),
        )],
    });
    t.push(NodeOp::Update {
        node: RULE,
        props: vec![(
            rekindle_wire::turn::SPANS,
            Value::Array(vec![span(0xff28_282d), span(0xfffb_fbfb)]),
        )],
    });
    send(&mut e, &mut host, &t);
    // The fade keeps its transparent end, in the theme's colour.
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND_GRADIENT),
        Some(&Value::Array(vec![
            Value::color(0x000b_0d12),
            Value::color(0xff0b_0d12)
        ]))
    );
    // A text colour in a fill's gradient is left alone.
    assert_eq!(
        host.props().get(HEAD, ids::BACKGROUND_GRADIENT),
        Some(&Value::U32Array(vec![0x000b_0d12, 0x8028_282d]))
    );
    // A span's colour is text: the surface colour in a span is kept.
    assert_eq!(
        host.props().get(RULE, rekindle_wire::turn::SPANS),
        Some(&Value::Array(vec![span(0xffd6_dce8), span(0xfffb_fbfb)]))
    );
}

#[test]
fn every_colour_cell_has_a_role() {
    for p in COLOR_PROPS {
        assert!(ColorRole::of(*p).is_some(), "{p:?}");
    }
    assert_eq!(ColorRole::of(PropId(322)), None);
    let json = r#"{"recolors": [{"from": "ffffff", "to": "0b0d12ff", "role": "fill"}, {"from": "313338", "to": "ff0000ff"}]}"#;
    let work = Work::from_json(json).unwrap();
    assert_eq!(work.recolors[0].role, ColorRole::Fill);
    assert_eq!(work.recolors[1].role, ColorRole::Any);
    assert!(!work.to_json().contains("\"any\""));
}

/// Every grey `background` magenta.
fn magenta_surfaces() -> Work {
    Work {
        prop_recolors: vec![PropRecolor {
            props: vec![ids::BACKGROUND],
            select: None,
            max_chroma: Some(24),
            opaque: false,
            to: 0xff00_ffff,
        }],
        ..Work::default()
    }
}

/// [`first`] in a theme: the page's fill and the rule's text colour.
fn themed(page: u32, text: u32) -> Turn {
    let mut t = first();
    t.push(NodeOp::Update {
        node: PAGE,
        props: vec![(ids::BACKGROUND, rgba(page))],
    });
    t.push(NodeOp::Update {
        node: RULE,
        props: vec![(ids::COLOR, rgba(text))],
    });
    t
}

#[test]
fn a_prop_recolour_paints_the_cell_in_a_light_and_a_dark_theme() {
    // The dark theme's surface is the light theme's text.
    for (page, text) in [(0xffff_ffff, 0x3133_38ff), (0x3133_38ff, 0xffff_ffff)] {
        let (mut e, mut host) = engine(magenta_surfaces());
        send(&mut e, &mut host, &themed(page, text));
        assert_eq!(
            host.props().get(PAGE, ids::BACKGROUND),
            Some(&rgba(0xff00_ffff))
        );
        assert_eq!(host.props().get(RULE, ids::COLOR), Some(&rgba(text)));
    }
}

#[test]
fn a_prop_recolour_keeps_coloured_and_transparent_fills() {
    let (mut e, mut host) = engine(magenta_surfaces());
    let mut t = first();
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(ids::BACKGROUND, rgba(0x5865_f2ff))],
    });
    t.push(NodeOp::Update {
        node: 30,
        props: vec![(ids::BACKGROUND, Value::color(0))],
    });
    t.push(NodeOp::Update {
        node: 31,
        props: vec![(ids::BACKGROUND, Value::color(0x8020_2024))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(HEAD, ids::BACKGROUND),
        Some(&rgba(0x5865_f2ff))
    );
    assert_eq!(
        host.props().get(30, ids::BACKGROUND),
        Some(&Value::color(0))
    );
    // A half-transparent grey stays half transparent.
    assert_eq!(
        host.props().get(31, ids::BACKGROUND),
        Some(&Value::color(0x80ff_00ff))
    );

    let mut work = magenta_surfaces();
    work.prop_recolors[0].opaque = true;
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(31, ids::BACKGROUND),
        Some(&Value::color(0x8020_2024))
    );
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0xff00_ffff))
    );
}

#[test]
fn a_prop_recolour_follows_the_guests_writes_and_its_selector() {
    let mut work = magenta_surfaces();
    work.recolors = recolours().recolors;
    work.prop_recolors[0].select = Some(feed().parent_of());
    let (mut e, mut host) = engine(work);
    send(&mut e, &mut host, &first());
    // Over the recolour of the same colour.
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0xff00_ffff))
    );
    let mut t = Turn::new();
    t.push(NodeOp::Update {
        node: PAGE,
        props: vec![(ids::BACKGROUND, rgba(0xf2f3_f5ff))],
    });
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(ids::BACKGROUND, rgba(0xf2f3_f5ff))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0xff00_ffff))
    );
    assert_eq!(
        host.props().get(HEAD, ids::BACKGROUND),
        Some(&rgba(0xf2f3_f5ff))
    );
}

#[test]
fn a_prop_recolour_paints_a_gradients_stops_and_keeps_a_fade() {
    let mut work = magenta_surfaces();
    work.prop_recolors[0].props.push(ids::BACKGROUND_GRADIENT);
    work.prop_recolors[0].opaque = true;
    let (mut e, mut host) = engine(work);
    let fade = Value::Array(vec![Value::color(0xff12_1214), Value::color(0x0012_1214)]);
    let object = |stops: Vec<Value>| {
        Value::Object(vec![
            (PropId(70), Value::F64(180.0)),
            (PropId(577), Value::Array(stops)),
        ])
    };
    let mut t = first();
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(ids::BACKGROUND_GRADIENT, fade)],
    });
    t.push(NodeOp::Update {
        node: 30,
        props: vec![(
            ids::BACKGROUND_GRADIENT,
            object(vec![Value::color(0x001a_1a1e), Value::color(0xff1a_1a1e)]),
        )],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(HEAD, ids::BACKGROUND_GRADIENT),
        Some(&Value::Array(vec![
            Value::color(0xffff_00ff),
            Value::color(0x0012_1214)
        ]))
    );
    assert_eq!(
        host.props().get(30, ids::BACKGROUND_GRADIENT),
        Some(&object(vec![
            Value::color(0x001a_1a1e),
            Value::color(0xffff_00ff)
        ]))
    );
}

#[test]
fn a_prop_recolour_reads_back_from_its_json_and_names_colour_cells() {
    let json = r#"{"prop_recolors": [{"props": ["background", 21], "max_chroma": 24, "opaque": true, "to": "ff00ffff"}]}"#;
    let work = Work::from_json(json).unwrap();
    assert_eq!(
        work.prop_recolors[0].props,
        vec![ids::BACKGROUND, ids::ACTIVE_BACKGROUND]
    );
    assert!(work.prop_recolors[0].opaque);
    assert_eq!(Work::from_json(&work.to_json()).unwrap(), work);
    assert!(
        Work::from_json(r#"{"prop_recolors": [{"props": ["text"], "to": "ff00ffff"}]}"#).is_err()
    );
    assert!(
        Work::from_json(
            r#"{"prop_recolors": [{"props": ["backgroundGradient"], "to": "ff00ffff"}]}"#
        )
        .is_ok()
    );
}

#[test]
fn a_recolour_keeps_the_guests_alpha() {
    let (mut e, mut host) = engine(recolours());
    let mut t = first();
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![(ids::BORDER_COLOR, Value::color(0x4031_3338))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(HEAD, ids::BORDER_COLOR),
        Some(&Value::I32(0x400b_0d12))
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
    t.push(NodeOp::Update {
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
    t.push(NodeOp::Update {
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
        after: None,
        before: None,
        root,
        nodes: vec![
            MountNode {
                id: root,
                kind: ElementKind::VIEW,
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
                kind: ElementKind::TEXT,
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
    assert!(applied.ops.contains(&NodeOp::Delete {
        nodes: vec![PLUGIN + 0x10, PLUGIN + 0x11]
    }));
    for n in [PLUGIN + 0x10, PLUGIN + 0x11] {
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
        kind: ElementKind::VIEW,
        props: vec![],
    });
    t.push(NodeOp::Update {
        node: 32,
        props: vec![(TAG, str("post"))],
    });
    t.push(NodeOp::Children {
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
            .any(|op| matches!(op, NodeOp::Create { .. } | NodeOp::Delete { .. })),
        "{:?}",
        applied.ops
    );
    assert_eq!(host.props().get(PLUGIN + 1, TEXT), Some(&str("opened")));
    // A cell is cleared by writing it null, as the guest clears its own.
    assert_eq!(host.props().get(PLUGIN, a11y::LABEL), None);
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
    let at = |want: &dyn Fn(&NodeOp) -> bool| applied.ops.iter().position(want).unwrap();
    assert!(
        at(&|op| matches!(op, NodeOp::Delete { nodes } if nodes.contains(&PLUGIN)))
            < at(&|op| matches!(op, NodeOp::Create { node, .. } if *node == PLUGIN))
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

#[test]
fn a_guests_child_edits_are_read_as_its_list() {
    let (mut e, mut host) = engine(mounts());
    let (applied, _) = send(&mut e, &mut host, &first());
    let button = applied.ops.iter().find_map(|op| match op {
        NodeOp::Create { node, props, .. } if *node == BUTTON => Some(props),
        _ => None,
    });
    assert_eq!(
        button.and_then(|p| p.last()),
        Some(&(rekindle_wire::turn::ON, Value::Array(vec![str("press")]))),
        "a node with a handler listens for presses"
    );
    let old = [HEAD, RULE, 30, 31];
    let new = [HEAD, RULE, 30, 31, 32];
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: 32,
        kind: ElementKind::VIEW,
        props: vec![],
    });
    t.push(NodeOp::ChildEdits {
        node: FEED,
        edits: rekindle_wire::turn::child_edits(&old, &new).unwrap(),
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.node(FEED).unwrap().children,
        vec![HEAD, RULE, 30, 31, 32, BOX]
    );
    assert_eq!(e.guest().node(FEED).unwrap().children, new);
}

/// The `worklet` (733) cell of the Register button on the real guest's
/// welcome screen, led by its `PropId`: a `press_look` whose fill goes from
/// white to `0xffc2c2c7` when pressed.
const REGISTER_WORKLET: &str = concat!(
    "dd02073500000003cca19f3b0309000000030a000000030b000000030f0000700300000041030000803f030100000003",
    "0000803f0300009643030000d8410301000000030ad7233c030000004003000000000300000000030000000003000000",
    "00030000000003000000000301000000030800000004ffffffff0000000004c7c2c2ff000000000300000000030000c0",
    "7f030000c07f0302000000030000803f030000c84203000020410300000000030ad7233c030000004003000000000300",
    "00000003000000000300000000030000000003000000000300000000030000803f030000c84203000020410300000000",
    "030ad7233c0300000040030000000003000000000300000000030000000003000000000300000000",
);

#[test]
fn a_recolour_reaches_a_buttons_press_look() {
    use rekindle_wire::motion::ids::WORKLET;
    use rekindle_wire::motion::worklet::{PressLook, Worklet};
    let bytes: Vec<u8> = (0..REGISTER_WORKLET.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&REGISTER_WORKLET[i..i + 2], 16).unwrap())
        .collect();
    let cell = rekindle_wire::decode(&bytes[2..]).unwrap();
    let work = Work {
        recolors: vec![Recolor::new(0xff_ffff, 0x1e1f_22ff)],
        ..Work::default()
    };
    let (mut e, mut host) = engine(work);
    let mut t = first();
    t.push(NodeOp::Update {
        node: HEAD,
        props: vec![
            (ids::BACKGROUND, rgba(0xffff_ffff)),
            (WORKLET, cell.clone()),
        ],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(
        host.props().get(HEAD, ids::BACKGROUND),
        Some(&rgba(0x1e1f_22ff))
    );
    let look = |v: &Value| PressLook::from_worklet(&Worklet::from_value(v).unwrap()).unwrap();
    let written = host.props().get(HEAD, WORKLET).unwrap();
    // The rest colour follows the recolour, the pressed one isn't named.
    assert_eq!(look(written).looks[0].rest, 0xff1e_1f22);
    assert_eq!(look(written).looks[0].pressed, 0xffc2_c2c7);
    // Only that word of the guest's cell changed.
    let (a, b) = (
        Worklet::from_value(&cell).unwrap().to_words(),
        Worklet::from_value(written).unwrap().to_words(),
    );
    assert_eq!(a.len(), b.len());
    assert_eq!(
        (0..a.len()).filter(|&i| a[i] != b[i]).collect::<Vec<_>>(),
        [22]
    );
}

#[test]
fn a_flush_carries_new_work_with_no_turn_from_the_guest() {
    let (mut e, mut host) = engine(with_mounts(vec![plugin_button(PLUGIN, "open")]));
    send(&mut e, &mut host, &first());
    assert_eq!(e.flush(), None);
    let mut changed = plugin_button(PLUGIN, "open");
    changed.nodes[1].cells = vec![Cell::new(TEXT, str("opened"))];
    e.set_work(with_mounts(vec![changed])).unwrap();
    let flushed = Turn::from_bytes(&e.flush().unwrap()).unwrap();
    assert_eq!(
        flushed.ops,
        vec![NodeOp::Update {
            node: PLUGIN + 1,
            props: vec![(TEXT, str("opened"))]
        }]
    );
    host.apply(&flushed);
    assert_eq!(host.props().get(PLUGIN + 1, TEXT), Some(&str("opened")));
    // The guest's next turn doesn't write it again.
    assert_eq!(e.flush(), None);
    let (applied, _) = send(&mut e, &mut host, &quiet());
    assert!(!mentions(&applied, PLUGIN + 1));
}

#[test]
fn a_recolour_reaches_a_mounts_cells() {
    let mut work = with_mounts(vec![plugin_button(PLUGIN, "open")]);
    work.mounts[0].nodes[0]
        .cells
        .push(Cell::new(ids::BACKGROUND, rgba(0x3133_3880)));
    let themed = Work {
        recolors: recolours().recolors,
        ..work.clone()
    };
    let (mut e, mut host) = engine(themed);
    send(&mut e, &mut host, &first());
    // The mount's alpha, times `to`'s.
    assert_eq!(
        host.props().get(PLUGIN, ids::BACKGROUND),
        Some(&rgba(0x0b0d_1280))
    );
    // Without the theme the mount's own colour comes back.
    e.set_work(work).unwrap();
    host.apply(&Turn::from_bytes(&e.flush().unwrap()).unwrap());
    assert_eq!(
        host.props().get(PLUGIN, ids::BACKGROUND),
        Some(&rgba(0x3133_3880))
    );
}

/// A switch on [`plugin_button`]'s "open": its text says the state, a label
/// is written only while off, and a recolour applies only while on.
fn open_switch() -> Switches {
    Switches::from_json(
        r#"{"switches": [{
            "element": "open",
            "on": true,
            "when_on": {
                "cells": [{"node": 4026532097, "cells": [{"prop": "text", "value": {"str": "on"}}]}],
                "recolors": [{"from": "313338", "to": "0b0d12ff"}]
            },
            "when_off": {
                "cells": [{"node": 4026532097, "cells": [
                    {"prop": "text", "value": {"str": "off"}},
                    {"prop": "accessibilityLabel", "value": {"str": "switched off"}}
                ]}]
            }
        }]}"#,
    )
    .unwrap()
}

#[test]
fn a_switch_writes_its_states_cells_and_recolours() {
    assert_eq!(PLUGIN + 1, 4_026_532_097);
    let base = with_mounts(vec![plugin_button(PLUGIN, "open")]);
    let switches = open_switch();
    let on = switches.apply(&base, &BTreeMap::new());
    assert_eq!(
        on.mounts[0].nodes[1].cells,
        vec![Cell::new(TEXT, str("on"))]
    );
    assert_eq!(on.recolors.len(), 1);
    let off = switches.apply(&base, &BTreeMap::from([("open".to_string(), false)]));
    assert_eq!(
        off.mounts[0].nodes[1].cells,
        vec![
            Cell::new(TEXT, str("off")),
            Cell::new(a11y::LABEL, str("switched off"))
        ]
    );
    assert!(off.recolors.is_empty());

    // Flipped while running, the mount keeps its nodes and only its cells
    // change, the label cleared.
    let (mut e, mut host) = engine(off);
    send(&mut e, &mut host, &first());
    e.set_work(on).unwrap();
    let flushed = Turn::from_bytes(&e.flush().unwrap()).unwrap();
    assert!(
        !flushed
            .ops
            .iter()
            .any(|op| matches!(op, NodeOp::Create { .. } | NodeOp::Delete { .. }))
    );
    host.apply(&flushed);
    assert_eq!(host.props().get(PLUGIN + 1, TEXT), Some(&str("on")));
    assert_eq!(host.props().get(PLUGIN + 1, a11y::LABEL), None);
    assert_eq!(
        host.props().get(PAGE, ids::BACKGROUND),
        Some(&rgba(0x0b0d_12ff))
    );
}

/// The system back, as the host raises it: UI event op 2.
fn back_event() -> Vec<u8> {
    let mut b = rekindle_wire::event::UI.to_le_bytes().to_vec();
    b.extend([
        rekindle_wire::event::VERSION,
        rekindle_wire::event::ops::BACK,
    ]);
    b
}

#[test]
fn the_system_back_reaches_a_back_handler_instead_of_the_guest() {
    let (mut e, mut host) = engine(with_mounts(vec![plugin_button(PLUGIN, "open")]));
    send(&mut e, &mut host, &first());
    // Without a handler, the guest receives it.
    assert_eq!(e.event(&back_event()), Ok(Verdict::Pass));
    let back = Handler::Plugin {
        plugin: "rekindle".into(),
        surface: "page".into(),
        element: "back".into(),
    };
    e.set_work(Work {
        back: Some(back),
        ..with_mounts(vec![plugin_button(PLUGIN, "open")])
    })
    .unwrap();
    assert_eq!(e.event(&back_event()), Ok(Verdict::Consume(None)));
    // In a batch, only the back is taken out.
    let pressed = Event::press(80).to_bytes();
    let sent = batch(&[&back_event(), &pressed]);
    let Ok(Verdict::Batch(out)) = e.event(&sent) else {
        panic!("the batch wasn't rewritten");
    };
    let out = frame::Frame::from_bytes(&out).unwrap();
    assert_eq!(out.batch().unwrap(), vec![pressed.as_slice()]);
    let want = PluginPress {
        plugin: "rekindle".into(),
        surface: "page".into(),
        element: "back".into(),
    };
    assert_eq!(e.take_presses(), vec![want.clone(), want]);
    // A count handler can't be the back's.
    let counting = Work {
        back: Some(Handler::Count {
            node: PLUGIN,
            prop: TEXT,
            one: "1".into(),
            many: "{n}".into(),
        }),
        ..with_mounts(vec![plugin_button(PLUGIN, "open")])
    };
    assert!(counting.check().is_err());
}

/// A page with a search field and a composer, and the window's root on it.
fn fields() -> Turn {
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: PAGE,
        kind: ElementKind::VIEW,
        props: vec![],
    });
    for (node, id) in [(40, "search"), (41, "composer")] {
        t.push(NodeOp::Create {
            node,
            kind: ElementKind::INPUT,
            props: vec![(a11y::ID, str(id)), (VALUE, str(""))],
        });
    }
    t.push(NodeOp::Children {
        node: PAGE,
        children: vec![40, 41],
    });
    t.push(NodeOp::Root { node: PAGE });
    t
}

fn change_text(node: NodeId, text: &str, count: i64) -> Vec<u8> {
    let id = |n: &str| PropId::from_name(n).unwrap();
    Event {
        node,
        name: CHANGE_TEXT,
        value: Value::Object(vec![
            (id("text"), str(text)),
            (id("eventCount"), Value::int(count)),
        ]),
    }
    .to_bytes()
}

fn force_value(turn: &Turn, id: NodeId) -> Option<rekindle_wire::text::ForceValue> {
    turn.ops.iter().find_map(|op| match op {
        NodeOp::Update { node, props } if *node == id => props
            .iter()
            .find(|(p, _)| *p == FORCE_VALUE)
            .and_then(|(_, v)| rekindle_wire::text::ForceValue::from_value(v)),
        _ => None,
    })
}

#[test]
fn inserted_text_goes_after_what_the_user_typed_in_the_field_they_used() {
    let (mut e, mut host) = engine(Work::default());
    send(&mut e, &mut host, &fields());
    assert_eq!(e.insert_text("x"), None, "two fields and none used");
    assert_eq!(
        e.event(&change_text(41, "hi ", 3)).unwrap(),
        Verdict::Pass,
        "the platform's edit reaches the guest"
    );
    let ins = e.insert_text("(^_^)").unwrap();
    assert_eq!(
        (ins.node, ins.found, ins.force.text.as_str()),
        (41, Found::Used, "hi (^_^)")
    );
    let flushed = Turn::from_bytes(&e.flush().unwrap()).unwrap();
    let force = force_value(&flushed, 41).unwrap();
    assert_eq!(force.text, "hi (^_^)");
    assert_eq!((force.start, force.end), (8, 8));
    assert_eq!(
        force.event_count,
        Some(3),
        "the platform's count: no rebase"
    );
    assert_eq!(e.flush(), None, "written once");

    // The guest writes the field back, then deletes it.
    let mut t = Turn::new();
    t.push(NodeOp::Update {
        node: 41,
        props: vec![(VALUE, str(""))],
    });
    send(&mut e, &mut host, &t);
    assert_eq!(e.fields()[&41].text, "");
    let mut t = Turn::new();
    t.push(NodeOp::Children {
        node: PAGE,
        children: vec![40],
    });
    t.push(NodeOp::Delete { nodes: vec![41] });
    send(&mut e, &mut host, &t);
    let ins = e.insert_text("!").unwrap();
    assert_eq!((ins.node, ins.found), (40, Found::Only));
}

#[test]
fn a_composer_selector_finds_the_field_when_none_was_used() {
    let (mut e, mut host) = engine(Work::default());
    send(&mut e, &mut host, &fields());
    e.set_composer(Composer {
        field: Some(Selector::cell(a11y::ID, "composer")),
        ..Composer::default()
    });
    let ins = e.insert_text("hey").unwrap();
    assert_eq!(
        (ins.node, ins.found, ins.force.text.as_str()),
        (41, Found::Selector, "hey")
    );
    // The guest's own next turn carries it.
    let (applied, _) = send(&mut e, &mut host, &Turn::new());
    assert_eq!(force_value(&applied, 41).unwrap().text, "hey");
}

fn selection(node: NodeId, start: i64, end: i64) -> Vec<u8> {
    let id = |n: &str| PropId::from_name(n).unwrap();
    Event {
        node,
        name: SELECTION_CHANGE,
        value: Value::Object(vec![
            (id("start"), Value::int(start)),
            (id("end"), Value::int(end)),
        ]),
    }
    .to_bytes()
}

#[test]
fn inserted_text_replaces_the_selection_in_utf16_units() {
    let (mut e, mut host) = engine(Work::default());
    send(&mut e, &mut host, &fields());
    e.event(&change_text(41, "a😀b", 2)).unwrap();
    // The caret after the emoji: 3 UTF-16 units in.
    e.event(&selection(41, 3, 3)).unwrap();
    let ins = e.insert_text("(^_^)").unwrap();
    assert_eq!(ins.force.text, "a😀(^_^)b");
    assert_eq!((ins.force.start, ins.force.end), (8, 8));
    // "😀(" selected, backwards.
    e.event(&selection(41, 4, 1)).unwrap();
    let ins = e.insert_text("x").unwrap();
    assert_eq!(ins.force.text, "ax^_^)b");
    assert_eq!((ins.force.start, ins.force.end), (2, 2));
}

#[test]
fn a_slash_command_the_host_runs_is_kept_and_its_reply_replaces_the_text() {
    let (mut e, mut host) = engine(Work::default());
    let mut t = fields();
    t.push(NodeOp::Create {
        node: 42,
        kind: ElementKind::VIEW,
        props: vec![(a11y::ID, str("send"))],
    });
    t.push(NodeOp::Children {
        node: PAGE,
        children: vec![40, 41, 42],
    });
    send(&mut e, &mut host, &t);
    e.set_composer(Composer {
        field: Some(Selector::cell(a11y::ID, "composer")),
        send: Some(Selector::cell(a11y::ID, "send")),
        commands: ["kaomoji".to_string()].into(),
    });
    let press = Event::press(42).to_bytes();
    // Plain text and the guest's own commands go to the guest.
    e.event(&change_text(41, "hello", 1)).unwrap();
    assert_eq!(e.event(&press).unwrap(), Verdict::Pass);
    e.event(&change_text(41, "/giphy cat", 2)).unwrap();
    assert_eq!(e.event(&press).unwrap(), Verdict::Pass);
    e.event(&change_text(41, " /kaomoji happy ", 3)).unwrap();
    assert_eq!(e.event(&press).unwrap(), Verdict::Consume(None));
    let submit = Event {
        node: 41,
        name: rekindle_wire::event::names::SUBMIT,
        value: Value::Null,
    };
    assert_eq!(e.event(&submit.to_bytes()).unwrap(), Verdict::Consume(None));
    let typed = e.take_typed();
    assert_eq!(typed.len(), 2);
    assert_eq!(
        e.last_event(rekindle_wire::event::names::SUBMIT),
        Some(&submit),
        "a consumed event is still the last of its name"
    );
    assert_eq!(
        typed[0],
        Typed {
            field: 41,
            text: "/kaomoji happy".into()
        }
    );
    let force = e.set_text(41, "(^_^)");
    assert_eq!(
        (force.text.as_str(), force.start, force.event_count),
        ("(^_^)", 5, Some(3))
    );
    let flushed = Turn::from_bytes(&e.flush().unwrap()).unwrap();
    assert_eq!(force_value(&flushed, 41).unwrap().text, "(^_^)");
}

#[test]
fn a_prefix_selector_finds_every_node_whose_cell_starts_with_it() {
    let width = PropId::from_name("width").unwrap();
    let (mut e, mut host) = engine(Work {
        rows: vec![RowRule {
            select: Selector::Prefix {
                prop: a11y::ID,
                prefix: "avatar:".into(),
            },
            action: RowAction::Cells(vec![Cell::new(width, Value::F64(24.0))]),
        }],
        ..Work::default()
    });
    let mut t = Turn::new();
    for (node, id) in [(50, "avatar:1"), (51, "avatar:2"), (52, "author:1")] {
        t.push(NodeOp::Create {
            node,
            kind: ElementKind::VIEW,
            props: vec![(a11y::ID, str(id))],
        });
    }
    let (applied, _) = send(&mut e, &mut host, &t);
    let widths: Vec<NodeId> = [50, 51, 52]
        .into_iter()
        .filter(|n| host.props().get(*n, width) == Some(&Value::F64(24.0)))
        .collect();
    assert_eq!(widths, [50, 51], "{applied:?}");
}

/// A settings sheet and a list of DM rows, as an iOS guest names them.
fn picker_tree() -> Turn {
    let mut t = Turn::new();
    let node = |t: &mut Turn, node: NodeId, kind: ElementKind, cells: Vec<(PropId, Value)>| {
        t.push(NodeOp::Create {
            node,
            kind,
            props: vec![],
        });
        if !cells.is_empty() {
            t.push(NodeOp::Update { node, props: cells });
        }
    };
    let id = |s: &str| (a11y::ID, str(s));
    let text = |s: &str| (TEXT, str(s));
    node(&mut t, 1, ElementKind::VIEW, vec![id("settings-phone")]);
    node(&mut t, 2, ElementKind::SCROLL, vec![]);
    node(
        &mut t,
        3,
        ElementKind::VIEW,
        vec![id("settings.row.account"), (a11y::ROLE, str("button"))],
    );
    node(&mut t, 4, ElementKind::TEXT, vec![text("Account")]);
    node(
        &mut t,
        7,
        ElementKind::VIEW,
        vec![(a11y::LABEL, str("Mute"))],
    );
    node(&mut t, 8, ElementKind::TEXT, vec![text("Mute")]);
    node(&mut t, 9, ElementKind::VIEW, vec![]);
    node(&mut t, 10, ElementKind::TEXT, vec![text("Help")]);
    node(&mut t, 20, ElementKind::LIST, vec![id("dms")]);
    node(
        &mut t,
        21,
        ElementKind::VIEW,
        vec![id("dm_row_1531461354488201407")],
    );
    node(&mut t, 22, ElementKind::TEXT, vec![text("alice")]);
    node(
        &mut t,
        23,
        ElementKind::VIEW,
        vec![id("dm_row_1531461354488201999")],
    );
    node(&mut t, 24, ElementKind::TEXT, vec![text("bob")]);
    node(&mut t, 25, ElementKind::VIEW, vec![]);
    node(&mut t, 26, ElementKind::IMAGE, vec![id("dm-avatar")]);
    node(&mut t, 27, ElementKind::VIEW, vec![]);
    node(&mut t, 28, ElementKind::TEXT, vec![text("Help")]);
    node(&mut t, 30, ElementKind::VIEW, vec![]);
    for (n, children) in [
        (30, vec![1, 20]),
        (1, vec![2]),
        (2, vec![3, 7, 9]),
        (3, vec![4]),
        (7, vec![8]),
        (9, vec![10]),
        (20, vec![21, 23, 25, 27]),
        (21, vec![22]),
        (23, vec![24]),
        (25, vec![26]),
        (27, vec![28]),
    ] {
        t.push(NodeOp::Children { node: n, children });
    }
    t.push(NodeOp::Root { node: 30 });
    t
}

fn picker() -> Tree {
    let mut tree = Tree::new();
    tree.apply(&picker_tree());
    tree
}

#[test]
fn a_pick_prefers_the_nodes_own_id() {
    let p = pick::pick(&picker(), 3).unwrap();
    assert_eq!(
        p.best.selector,
        Selector::cell(a11y::ID, "settings.row.account")
    );
    assert_eq!(p.best.basis, Basis::Id);
    assert_eq!((p.best.score, p.best.matches), (100, 1));
    assert_eq!(p.name, "settings-row-account");
    assert!(p.fallbacks.iter().all(|c| c.score < 100));
    assert!(
        p.fallbacks.iter().any(|c| c.basis == Basis::TextUnder),
        "{:#?}",
        p.fallbacks
    );
}

#[test]
fn an_entity_id_is_offered_whole_and_as_a_prefix() {
    let p = pick::pick(&picker(), 21).unwrap();
    assert_eq!(p.best.basis, Basis::IdEntity);
    assert!(p.best.unique());
    assert_eq!(p.name, "dm-row");
    let prefix = p
        .fallbacks
        .iter()
        .find(|c| c.basis == Basis::IdPrefix)
        .unwrap();
    assert_eq!(
        prefix.selector,
        Selector::Prefix {
            prop: a11y::ID,
            prefix: "dm_row_".into()
        }
    );
    assert_eq!(prefix.matches, 2, "both rows");
}

#[test]
fn a_node_without_an_id_is_found_under_the_nearest_one() {
    let tree = picker();
    let p = pick::pick(&tree, 7).unwrap();
    assert_eq!(p.best.basis, Basis::LabelUnder);
    assert!(p.best.unique());
    assert_eq!(p.name, "settings-phone-mute-view");
    let sheet = serde_json::json!({ "cell": { "prop": "accessibilityId", "value": { "str": "settings-phone" } } });
    assert_eq!(
        serde_json::to_value(&p.best.selector).unwrap(),
        serde_json::json!({ "all": [{ "kind": "view" }, { "label": "Mute" }, { "within": sheet }] })
    );
    // The chain through the scroll matches the same one node, and costs
    // more than `within` at depth 2, so it isn't offered.
    assert!(p.fallbacks.iter().all(|c| c.basis != Basis::LabelUnder));

    // "Help" is under the list too, and every view above either row holds
    // it, so the text alone matches four nodes.
    let p = pick::pick(&tree, 9).unwrap();
    assert_eq!(p.best.basis, Basis::TextUnder);
    assert!(p.best.unique());
    let text = p.fallbacks.iter().find(|c| c.basis == Basis::Text).unwrap();
    assert_eq!(text.matches, 4);
}

#[test]
fn a_childs_id_finds_its_parent() {
    let p = pick::pick(&picker(), 25).unwrap();
    assert_eq!(p.best.basis, Basis::ChildId);
    assert!(p.best.unique());
    assert_eq!(p.name, "dm-avatar-view");
}

#[test]
fn every_candidate_matches_the_picked_node() {
    let tree = picker();
    let view = select::View::new(&tree);
    for n in tree.nodes().map(|n| n.id) {
        let Some(p) = pick::pick(&tree, n) else {
            continue;
        };
        for c in std::iter::once(&p.best).chain(&p.fallbacks) {
            assert!(view.matches(n, &c.selector), "#{n}: {c:?}");
            assert_eq!(view.select(&c.selector).len(), c.matches);
        }
    }
    assert!(pick::pick(&tree, 999).is_none());
}

#[test]
fn a_pick_consumes_the_next_press_only() {
    let (mut e, mut host) = engine(Work::default());
    send(&mut e, &mut host, &picker_tree());
    assert!(e.take_pick().is_none());
    e.pick();
    assert!(e.picking());
    assert_eq!(
        e.event(&Event::press(3).to_bytes()),
        Ok(Verdict::Consume(None))
    );
    assert!(!e.picking());
    let p = e.take_pick().unwrap();
    assert_eq!(p.node, 3);
    assert_eq!(
        p.best.selector,
        Selector::cell(a11y::ID, "settings.row.account")
    );
    assert!(e.take_pick().is_none());
    assert_eq!(e.event(&Event::press(3).to_bytes()), Ok(Verdict::Pass));
    assert_eq!(e.pick_node(21).unwrap().name, "dm-row");

    e.pick();
    e.cancel_pick();
    assert_eq!(e.event(&Event::press(3).to_bytes()), Ok(Verdict::Pass));
}

#[test]
fn within_matches_an_ancestor_at_any_depth() {
    let tree = picker();
    let view = select::View::new(&tree);
    let sheet = Selector::cell(a11y::ID, "settings-phone").within();
    let help = Selector::All(vec![Selector::Kind(ElementKind::TEXT), sheet]);
    assert_eq!(view.select(&help), vec![4, 8, 10]);
    assert!(!view.matches(1, &Selector::cell(a11y::ID, "settings-phone").within()));
}
