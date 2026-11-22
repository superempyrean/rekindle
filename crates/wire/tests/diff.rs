//! The keyed sibling diff (`ops.rs` apply shape): minimal insert / move /
//! remove over two `&[u32]` child-id lists.

use rekindle_wire::{ChildOp, diff_children};

#[test]
fn identical_lists_produce_no_ops() {
    assert_eq!(diff_children(&[1, 2, 3], &[1, 2, 3]), vec![]);
    assert_eq!(diff_children(&[], &[]), vec![]);
}

#[test]
fn pure_insert_at_front() {
    // 4 is new; 1,2,3 keep their order, so only one insert.
    assert_eq!(
        diff_children(&[1, 2, 3], &[4, 1, 2, 3]),
        vec![ChildOp::Insert { id: 4, index: 0 }]
    );
}

#[test]
fn pure_insert_in_middle() {
    assert_eq!(
        diff_children(&[1, 2, 3], &[1, 9, 2, 3]),
        vec![ChildOp::Insert { id: 9, index: 1 }]
    );
}

#[test]
fn pure_remove() {
    assert_eq!(
        diff_children(&[1, 2, 3], &[1, 3]),
        vec![ChildOp::Remove { id: 2 }]
    );
}

#[test]
fn swapping_two_moves_only_one() {
    // [1,2,3] -> [1,3,2]: keep the longest increasing run (1,2 or 1,3); exactly
    // one node has to move, not two.
    let ops = diff_children(&[1, 2, 3], &[1, 3, 2]);
    let moves: Vec<_> = ops
        .iter()
        .filter(|o| matches!(o, ChildOp::Move { .. }))
        .collect();
    assert_eq!(moves.len(), 1, "a single swap is one move, got {ops:?}");
}

#[test]
fn reversal_moves_all_but_one() {
    // [1,2,3] -> [3,2,1]: the LIS is length 1, so two nodes move.
    let ops = diff_children(&[1, 2, 3], &[3, 2, 1]);
    let moves = ops
        .iter()
        .filter(|o| matches!(o, ChildOp::Move { .. }))
        .count();
    assert_eq!(moves, 2);
}

#[test]
fn mixed_insert_move_remove() {
    // old [1,2,3,4]  new [2,5,1,3]
    //  - 4 removed
    //  - 5 inserted
    //  - 1,2,3 present; sources into old = [1,_,0,2]; LIS over {1,0,2} keeps
    //    (2 at pos0 -> src1) then (3 at pos3 -> src2): ids 2 and 3 stay, 1 moves.
    let ops = diff_children(&[1, 2, 3, 4], &[2, 5, 1, 3]);
    assert!(ops.contains(&ChildOp::Remove { id: 4 }));
    assert!(ops.contains(&ChildOp::Insert { id: 5, index: 1 }));
    let moves: Vec<_> = ops
        .iter()
        .filter_map(|o| {
            if let ChildOp::Move { id, .. } = o {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    // Two longest increasing runs tie ({2,3} and {1,3}); exactly one node moves.
    assert!(
        moves == vec![1] || moves == vec![2],
        "exactly one of 1 or 2 should move, got {ops:?}"
    );
}
