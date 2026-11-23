//! Cells copied byte for byte out of the render turns of Phoenix's Android
//! guest (build 356207), run headless to its welcome and login screens.

use rekindle_wire::motion::op::{Anim, Field, Op, Program, THIS_VIEW};
use rekindle_wire::motion::{Channel, Curve, Easing, Motion, Spec, ids};
use rekindle_wire::{PropId, Value, decode};

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// `viewMotion` (740) of the welcome screen's Register (slot 9) and Log In
/// (12) buttons, and the login screen's View Password (28) and Log In (31)
/// buttons, each led by its `PropId`, `e4 02`.
const VIEW_MOTION: [(u32, &str); 4] = [
    (
        9,
        "e402 07 05000000 03 15000000 03 09000000 03 05000000 04 ffffffff00000000 03 00000000",
    ),
    (
        12,
        "e402 07 05000000 03 15000000 03 0c000000 03 05000000 04 ffffffff00000000 03 00000000",
    ),
    (
        28,
        "e402 07 05000000 03 15000000 03 1c000000 03 05000000 04 ffffffff00000000 03 00000000",
    ),
    (
        31,
        "e402 07 05000000 03 15000000 03 1f000000 03 05000000 04 ffffffff00000000 03 00000000",
    ),
];

#[test]
fn view_motion_is_one_op_21_in_a_tag_7_array() {
    for (slot, cell) in VIEW_MOTION {
        let bytes = hex(cell);
        assert_eq!(
            PropId(u16::from_le_bytes([bytes[0], bytes[1]])),
            ids::VIEW_MOTION
        );
        let value = decode(&bytes[2..]).unwrap();
        assert_eq!(
            value,
            Value::Array(vec![
                Value::I32(21),
                Value::I32(slot as i32),
                Value::I32(5),
                Value::I64(0xffff_ffff),
                Value::I32(0),
            ])
        );
        let program = Program::from_value(&value).unwrap();
        assert_eq!(
            program,
            Program::new(vec![Op {
                opcode: 21,
                fields: vec![
                    Field::Word(slot),
                    Field::Word(5),
                    Field::Word(THIS_VIEW),
                    Field::MaybeAnim(None),
                ],
            }])
        );
        // The program re-encodes to the guest's bytes.
        assert_eq!(program.to_value().to_bytes(), bytes[2..]);
        // None of it is a rekindle spec entry.
        assert_eq!(Spec::from_value(&value), Some(Spec::new()));
    }
}

#[test]
fn the_engine_takes_the_real_program_and_runs_nothing() {
    let words = Value::Array(vec![
        Value::I32(21),
        Value::I32(9),
        Value::I32(5),
        Value::I64(0xffff_ffff),
        Value::I32(0),
    ])
    .as_words()
    .unwrap();
    assert_eq!(words, [21, 9, 5, THIS_VIEW, 0]);
    let mut m = Motion::new();
    m.view_motion(15, &words);
    assert!(!m.wants_frame());
    assert_eq!(m.stats().specs, 0);
}

#[test]
fn a_words_table_cell_decodes_to_the_same_program() {
    let words = [21, 9, 5, THIS_VIEW, 0];
    let tabled = Value::words_tabled(&words);
    assert_eq!(tabled, Value::U32Array(words.to_vec()));
    assert_eq!(tabled.to_bytes()[0], 11);
    let back = decode(&tabled.to_bytes()).unwrap();
    assert_eq!(
        Program::from_value(&back),
        Program::from_value(&Value::words(&words))
    );
    // With no table, the same words are the guest's tag 7.
    assert_eq!(Value::words(&words).to_bytes(), hex(VIEW_MOTION[0].1)[2..]);
}

#[test]
fn a_spec_rides_the_same_grammar() {
    let spec = Spec::new().enter(
        Channel::Opacity,
        0.0,
        Curve::Timing {
            ms: 240.0,
            delay: 0.0,
            easing: Easing::EaseOutCubic,
        },
    );
    let words = spec.to_words();
    let program = Program::decode(&words);
    assert!(program.tail.is_empty());
    assert_eq!(program.ops[0].opcode, 24);
    assert!(matches!(
        program.ops[0].anim(),
        Some(Anim::Timing {
            ms: 240.0,
            easing: Easing::EaseOutCubic,
            ..
        })
    ));
    assert_eq!(spec.to_value().to_bytes()[0], 7);
}

/// `padding` (332) of the welcome screen's column, and `margin` (287) of a
/// login-screen row, `(0, 16, 0, 16)`.
#[test]
fn quads_are_left_top_right_bottom() {
    for (cell, id, quad) in [
        (
            "4c01 09 00008041 0000a042 00008041 0000c041",
            332,
            [16.0, 80.0, 16.0, 24.0],
        ),
        (
            "1f01 09 00000000 00008041 00000000 00008041",
            287,
            [0.0, 16.0, 0.0, 16.0],
        ),
    ] {
        let bytes = hex(cell);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), id);
        let value = decode(&bytes[2..]).unwrap();
        assert_eq!(value, Value::Quad(quad));
        assert_eq!(value.to_bytes(), bytes[2..]);
    }
}
