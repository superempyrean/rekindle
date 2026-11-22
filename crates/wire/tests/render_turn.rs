//! End-to-end proof: a representative mock render turn crosses as real bytes,
//! is decoded and applied against a retained tree, and a later recolour turn
//! changes only the cells it names while retained nodes keep their identity.
//!
//! The value codec and `PropId` table are recovered; the turn framing and the
//! node ids below are rekindle's model, pending a read of the guest code
//! (compiled into the host in build 356207, so recoverable from that binary).

use rekindle_wire::{ChildOp, ElementKind, NodeOp, PropId, Tree, Turn, Value, ids};

const ROOT: u32 = 1;
const LIST: u32 = 2;

fn first_render() -> Turn {
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: ROOT,
        kind: ElementKind::Div,
    });
    t.push(NodeOp::Create {
        node: LIST,
        kind: ElementKind::List,
    });
    for (id, kind) in [
        (10, ElementKind::Div),
        (11, ElementKind::Div),
        (12, ElementKind::Div),
    ] {
        t.push(NodeOp::Create { node: id, kind });
    }
    t.push(NodeOp::Create {
        node: 13,
        kind: ElementKind::Text,
    });
    t.push(NodeOp::SetProps {
        node: ROOT,
        props: vec![(ids::BACKGROUND, Value::Vec4([0.1, 0.1, 0.12, 1.0]))],
    });
    t.push(NodeOp::SetProps {
        node: 13,
        props: vec![(ids::COLOR, Value::Vec4([1.0, 1.0, 1.0, 1.0]))],
    });
    t.push(NodeOp::SetChildren {
        node: 10,
        children: vec![13],
    });
    t.push(NodeOp::SetChildren {
        node: LIST,
        children: vec![10, 11, 12],
    });
    t.push(NodeOp::SetChildren {
        node: ROOT,
        children: vec![LIST],
    });
    t
}

#[test]
fn render_turn_survives_the_wire_and_applies() {
    let bytes = first_render().to_bytes();
    assert_eq!(Turn::from_bytes(&bytes).unwrap(), first_render());

    let mut tree = Tree::new();
    let report = tree.apply_bytes(&bytes).unwrap();
    assert_eq!(tree.len(), 6);
    assert_eq!(tree.node(LIST).unwrap().children, vec![10, 11, 12]);
    assert_eq!(tree.node(13).unwrap().kind, ElementKind::Text);
    assert_eq!(
        tree.props().get(ROOT, ids::BACKGROUND),
        Some(&Value::Vec4([0.1, 0.1, 0.12, 1.0]))
    );
    // First mount: every child is an insert.
    let list_ops = &report.child_ops.iter().find(|(n, _)| *n == LIST).unwrap().1;
    assert_eq!(list_ops.len(), 3);
    assert!(list_ops.iter().all(|o| matches!(o, ChildOp::Insert { .. })));
}

#[test]
fn second_turn_is_sparse_and_keeps_identity() {
    let mut tree = Tree::new();
    tree.apply_bytes(&first_render().to_bytes()).unwrap();

    // A new message arrives at the end and the oldest scrolls out; the theme
    // recolours the root's background and border with a tag-8 property set.
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: 14,
        kind: ElementKind::Div,
    });
    t.push(NodeOp::SetProps {
        node: ROOT,
        props: vec![
            (ids::BACKGROUND, Value::Vec4([0.9, 0.2, 0.2, 1.0])),
            (ids::BORDER_COLOR, Value::Vec4([0.0, 0.0, 0.0, 1.0])),
        ],
    });
    t.push(NodeOp::SetChildren {
        node: LIST,
        children: vec![11, 12, 14],
    });
    t.push(NodeOp::Remove { node: 10 });
    let report = tree.apply_bytes(&t.to_bytes()).unwrap();

    let ops = &report.child_ops[0].1;
    assert_eq!(
        ops,
        &vec![
            ChildOp::Remove { id: 10 },
            ChildOp::Insert { id: 14, index: 2 }
        ]
    );
    assert_eq!(tree.node(LIST).unwrap().children, vec![11, 12, 14]);
    // Retained nodes were not re-created; the removed node's cells are gone.
    assert!(tree.node(10).is_none());
    assert!(
        tree.props().get(13, ids::COLOR).is_some(),
        "untouched node keeps its cells"
    );
    // Only the named cells changed.
    assert_eq!(
        tree.props().get(ROOT, ids::BACKGROUND),
        Some(&Value::Vec4([0.9, 0.2, 0.2, 1.0]))
    );
    assert_eq!(
        tree.props().get(ROOT, PropId(104)),
        Some(&Value::Vec4([0.0, 0.0, 0.0, 1.0]))
    );
    assert_eq!(tree.props().node_props(ROOT).count(), 2);
}

#[test]
fn malformed_turns_error() {
    assert!(Turn::from_bytes(&[]).is_err());
    assert!(Turn::from_bytes(&[1, 0, 0, 0, 9]).is_err());
    assert!(Turn::from_bytes(&[0xff, 0xff, 0xff, 0xff]).is_err());
}
