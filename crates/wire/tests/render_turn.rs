//! A render turn crosses as bytes, is decoded and applied against a retained
//! tree, and a later turn changes only the cells and children it names while
//! retained nodes keep their identity.

use rekindle_wire::turn::{ON, TEXT, child_edits, create_props};
use rekindle_wire::{ChildOp, ElementKind, NodeOp, PropId, Tree, Turn, Value, ids};

const ROOT: u32 = 1;
const LIST: u32 = 2;

fn create(node: u32, kind: ElementKind, props: Vec<(PropId, Value)>) -> NodeOp {
    NodeOp::Create {
        node,
        kind,
        props: create_props(props),
    }
}

fn first_render() -> Turn {
    let mut t = Turn::new();
    t.push(create(
        ROOT,
        ElementKind::VIEW,
        vec![(ids::BACKGROUND, Value::color(0xFF1A_1A1F))],
    ));
    t.push(create(LIST, ElementKind::LIST, vec![]));
    for id in [10, 11, 12, 14, 15, 17, 18, 19] {
        t.push(create(id, ElementKind::VIEW, vec![]));
    }
    t.push(create(
        13,
        ElementKind::TEXT,
        vec![
            (ids::COLOR, Value::color(0xFFFF_FFFF)),
            (ON, Value::Array(vec![Value::Str("press".into())])),
            (TEXT, Value::Str("hello".into())),
        ],
    ));
    t.push(NodeOp::Children {
        node: 10,
        children: vec![13],
    });
    t.push(NodeOp::Children {
        node: LIST,
        children: vec![10, 11, 12, 14, 15, 17, 18, 19],
    });
    t.push(NodeOp::Children {
        node: ROOT,
        children: vec![LIST],
    });
    t.push(NodeOp::Root { node: ROOT });
    t
}

#[test]
fn render_turn_survives_the_wire_and_applies() {
    let bytes = first_render().to_bytes();
    assert_eq!(bytes[0], 1, "version byte");
    assert_eq!(Turn::from_bytes(&bytes).unwrap(), first_render());

    let mut tree = Tree::new();
    let report = tree.apply_bytes(&bytes).unwrap();
    assert_eq!(tree.len(), 11);
    assert_eq!(tree.root(), Some(ROOT));
    assert_eq!(
        tree.node(LIST).unwrap().children,
        vec![10, 11, 12, 14, 15, 17, 18, 19]
    );
    assert_eq!(tree.node(13).unwrap().kind, ElementKind::TEXT);
    assert_eq!(
        tree.props().get(ROOT, ids::BACKGROUND),
        Some(&Value::I64(0xFF1A_1A1F))
    );
    // Create writes text first and on last.
    let NodeOp::Create { props, .. } = &first_render().ops[10] else {
        panic!("a create");
    };
    assert_eq!(props.first().map(|p| p.0), Some(TEXT));
    assert_eq!(props.last().map(|p| p.0), Some(ON));
    // First mount: every child is an insert.
    let list_ops = &report.child_ops.iter().find(|(n, _)| *n == LIST).unwrap().1;
    assert_eq!(list_ops.len(), 8);
    assert!(list_ops.iter().all(|o| matches!(o, ChildOp::Insert { .. })));
}

#[test]
fn second_turn_is_sparse_and_keeps_identity() {
    let mut tree = Tree::new();
    tree.apply_bytes(&first_render().to_bytes()).unwrap();

    // A new message arrives at the end and the oldest scrolls out; the theme
    // recolours the root's background and border.
    let old = [10, 11, 12, 14, 15, 17, 18, 19];
    let new = [11, 12, 14, 15, 17, 18, 19, 16];
    let edits = child_edits(&old, &new).expect("smaller than the list");
    let mut t = Turn::new();
    t.push(create(16, ElementKind::VIEW, vec![]));
    t.push(NodeOp::Update {
        node: ROOT,
        props: vec![
            (ids::BACKGROUND, Value::color(0xFFE6_3333)),
            (ids::BORDER_COLOR, Value::color(0xFF00_0000)),
        ],
    });
    t.push(NodeOp::ChildEdits { node: LIST, edits });
    t.push(NodeOp::Delete {
        nodes: vec![10, 13],
    });
    let bytes = t.to_bytes();
    assert_eq!(Turn::from_bytes(&bytes).unwrap(), t);
    let report = tree.apply_bytes(&bytes).unwrap();

    let ops = &report.child_ops[0].1;
    assert_eq!(
        ops,
        &vec![
            ChildOp::Remove { id: 10 },
            ChildOp::Insert { id: 16, index: 7 }
        ]
    );
    assert_eq!(tree.node(LIST).unwrap().children, new);
    assert!(tree.node(10).is_none() && tree.node(13).is_none());
    assert_eq!(
        tree.props().get(ROOT, ids::BACKGROUND),
        Some(&Value::I64(0xFFE6_3333))
    );
    assert_eq!(tree.props().node_props(ROOT).count(), 2);

    // A removed property is Null, and the cell goes.
    let mut t = Turn::new();
    t.push(NodeOp::Update {
        node: ROOT,
        props: vec![(ids::BORDER_COLOR, Value::Null)],
    });
    tree.apply_bytes(&t.to_bytes()).unwrap();
    assert_eq!(tree.props().get(ROOT, ids::BORDER_COLOR), None);
}

#[test]
fn a_resolved_turn_has_no_child_edits() {
    let mut tree = Tree::new();
    tree.apply_bytes(&first_render().to_bytes()).unwrap();
    let new = [10, 11, 12, 14, 15, 17, 18, 19, 16];
    let t = Turn {
        ops: vec![NodeOp::ChildEdits {
            node: LIST,
            edits: child_edits(&[10, 11, 12, 14, 15, 17, 18, 19], &new).unwrap(),
        }],
    };
    assert_eq!(
        tree.resolve(&t).unwrap().ops,
        vec![NodeOp::Children {
            node: LIST,
            children: new.to_vec()
        }]
    );
}

#[test]
fn an_unknown_kind_decodes() {
    let t = Turn {
        ops: vec![create(3, ElementKind::new("navigator"), vec![])],
    };
    assert_eq!(Turn::from_bytes(&t.to_bytes()).unwrap(), t);
}

#[test]
fn malformed_turns_error() {
    assert!(Turn::from_bytes(&[]).unwrap().ops.is_empty());
    assert!(Turn::from_bytes(&[2, 8]).is_err(), "version");
    assert!(Turn::from_bytes(&[1, 11]).is_err(), "op code");
    assert!(Turn::from_bytes(&[1, 5, 1, 0]).is_err(), "truncated");
    assert!(
        Turn::from_bytes(&[1, 4, 0xff, 0xff, 0xff, 0xff]).is_err(),
        "count"
    );
}
