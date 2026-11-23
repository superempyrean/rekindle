//! Cells copied byte for byte out of the render turns of Phoenix's Android
//! guest (build 356207), run headless to its welcome and login screens.

use rekindle_wire::motion::op::{Anim, Field, Follow, Op, Program, THIS_VIEW, input};
use rekindle_wire::motion::worklet::{Look, PressLook, Worklet, ids as worklets};
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

#[test]
fn view_motion_is_a_follow_of_the_pressed_input() {
    for (slot, cell) in VIEW_MOTION {
        let program = Program::from_value(&decode(&hex(cell)[2..]).unwrap()).unwrap();
        let follow = Follow::from_op(&program.ops[0]).unwrap();
        assert_eq!(
            follow,
            Follow {
                id: slot,
                input: input::PRESSED,
                key: THIS_VIEW,
                anim: None,
            }
        );
        assert_eq!(follow.to_op(), program.ops[0]);
    }
}

/// `worklet` (733) of the buttons whose `viewMotion` is above, each led by
/// its `PropId`, `dd 02`. Floats ride as their bit patterns.
const REGISTER: &str = concat!(
    "dd02073500000003cca19f3b0309000000030a000000030b000000030f0000700300000041030000803f030100000003",
    "0000803f0300009643030000d8410301000000030ad7233c030000004003000000000300000000030000000003000000",
    "00030000000003000000000301000000030800000004ffffffff0000000004c7c2c2ff000000000300000000030000c0",
    "7f030000c07f0302000000030000803f030000c84203000020410300000000030ad7233c030000004003000000000300",
    "00000003000000000300000000030000000003000000000300000000030000803f030000c84203000020410300000000",
    "030ad7233c0300000040030000000003000000000300000000030000000003000000000300000000",
);
const LOG_IN: &str = concat!(
    "dd02073500000003cca19f3b030c000000030d000000030e00000003110000700300000041030000803f030100000003",
    "0000803f0300009643030000d8410301000000030ad7233c030000004003000000000300000000030000000003000000",
    "00030000000003000000000301000000030800000004210c08a800000000040000008f000000000300000000030000c0",
    "7f030000c07f0302000000030000803f030000c84203000020410300000000030ad7233c030000004003000000000300",
    "00000003000000000300000000030000000003000000000300000000030000803f030000c84203000020410300000000",
    "030ad7233c0300000040030000000003000000000300000000030000000003000000000300000000",
);
const VIEW_PASSWORD: &str = concat!(
    "dd02073100000003cca19f3b031c00000004ffffffff00000000031e00000004ffffffff00000000030000c07f030000",
    "803f0301000000030000803f0300009643030000d8410301000000030ad7233c03000000400300000000030000000003",
    "000000000300000000030000000003000000000300000000030000803f03cdcc4c3e0302000000030000803f030000c8",
    "4203000020410300000000030ad7233c0300000040030000000003000000000300000000030000000003000000000300",
    "0000000300000000030000803f030000c84203000020410300000000030ad7233c030000004003000000000300000000",
    "0300000000030000000003000000000300000000",
);
const LOGIN_SUBMIT: &str = concat!(
    "dd02073900000003cca19f3b031f0000000320000000032100000003330000700300000041030000803f030100000003",
    "0000803f0300009643030000d8410301000000030ad7233c030000004003000000000300000000030000000003000000",
    "00030000000003000000000302000000030800000004f26558ff0000000004a3483aff000000000300000000030a0000",
    "0003ffffff1403ffffff140300000000030000c07f030000c07f0302000000030000803f030000c84203000020410300",
    "000000030ad7233c03000000400300000000030000000003000000000300000000030000000003000000000300000000",
    "030000803f030000c84203000020410300000000030ad7233c0300000040030000000003000000000300000000030000",
    "000003000000000300000000",
);

fn press_look(cell: &str) -> (Value, Worklet, PressLook) {
    let bytes = hex(cell);
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 733);
    let value = decode(&bytes[2..]).unwrap();
    let worklet = Worklet::from_value(&value).unwrap();
    assert_eq!(worklet.id, worklets::PRESS_LOOK);
    let look = PressLook::from_worklet(&worklet).unwrap();
    // The decoded worklet re-encodes to the guest's bytes.
    assert_eq!(look.to_worklet(), worklet);
    assert_eq!(look.to_worklet().to_value().to_bytes(), bytes[2..]);
    (value, worklet, look)
}

#[test]
fn the_buttons_worklets_are_press_looks() {
    let (_, _, register) = press_look(REGISTER);
    // The press value is the one viewMotion binds, the next two ease the
    // scale and the look, and the portal slot is the button's.
    assert_eq!((register.press, register.scale, register.look), (9, 10, 11));
    assert_eq!(register.portal_slot, 0x7000_000f);
    assert_eq!((register.shrink, register.pressed_scale), (8.0, 1.0));
    // The scale springs (m 1, k 300, c 27, clamped).
    let t = register.scale_transition;
    assert_eq!(
        (t.kind, t.mass, t.stiffness, t.damping, t.clamp),
        (1, 1.0, 300.0, 27.0, 1)
    );
    // The fill goes from white to grey.
    assert_eq!(
        register.looks,
        [Look {
            channel: 8,
            rest: 0xffff_ffff,
            pressed: 0xffc2_c2c7,
            only_pressed: 0,
        }]
    );
    assert!(register.opacity.iter().all(|o| o.is_nan()));
    assert_eq!(register.press_transition.kind, 2);
    assert_eq!(register.press_transition.ms.to_bits(), 0);
    assert_eq!(register.release_transition.kind, 0);

    let (_, _, log_in) = press_look(LOG_IN);
    assert_eq!(
        (log_in.press, log_in.looks[0].rest, log_in.looks[0].pressed),
        (12, 0xa808_0c21, 0x8f00_0000)
    );

    // A text button: no scale, no colours, pressed opacity 0.2.
    let (_, _, view_password) = press_look(VIEW_PASSWORD);
    assert_eq!(view_password.scale, 0xffff_ffff);
    assert_eq!(view_password.portal_slot, 0xffff_ffff);
    assert!(view_password.looks.is_empty());
    assert_eq!(
        view_password.opacity.map(f32::to_bits),
        [1.0f32, 0.2].map(f32::to_bits)
    );

    // The login screen's button moves its fill and its border.
    let (_, _, submit) = press_look(LOGIN_SUBMIT);
    assert_eq!(
        submit
            .looks
            .iter()
            .map(|l| (l.channel, l.rest, l.pressed))
            .collect::<Vec<_>>(),
        [
            (8, 0xff58_65f2, 0xff3a_48a3),
            (10, 0x14ff_ffff, 0x14ff_ffff)
        ]
    );
    assert!(submit.looks.iter().all(Look::is_color));
}

#[test]
fn a_recolour_rewrites_the_press_looks_fill() {
    let (_, _, mut register) = press_look(REGISTER);
    assert!(register.recolor(|c| (c == 0xffff_ffff).then_some(0xff1e_1f22)));
    let words = register.to_worklet().to_words();
    // Only the rest colour, word 22 of the cell, changed.
    let (_, original, _) = press_look(REGISTER);
    let diff: Vec<usize> = (0..words.len())
        .filter(|&i| words[i] != original.to_words()[i])
        .collect();
    assert_eq!(diff, [22]);
    assert_eq!(words[22], 0xff1e_1f22);
    // A colour no recolour names changes nothing.
    assert!(!register.recolor(|_| None));
    // The worklet's op, as the platform sends it to `apply`.
    let op = original.to_op(15);
    assert_eq!(op.opcode, 3);
    assert_eq!(op.word(1), Some(0));
    assert_eq!(op.word(2), Some(worklets::PRESS_LOOK));
}
