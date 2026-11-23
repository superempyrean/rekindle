//! The bytes the Android host's writer functions produced under emulation
//! (build 356207), and the press event its decoder reads.

use rekindle_wire::event::{Event, UiEvent};
use rekindle_wire::{NodeOp, PropId, Turn, Value, decode};

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

#[test]
fn values_are_the_encoders_bytes() {
    for (value, bytes) in [
        (Value::Null, "00"),
        (Value::Bool(true), "02"),
        (Value::Bool(false), "01"),
        (Value::int(7), "03 07 00 00 00"),
        (Value::color(0xFF33_6699), "04 99 66 33 ff 00 00 00 00"),
        (Value::color(0x4033_6699), "03 99 66 33 40"),
        (Value::int(-1), "03 ff ff ff ff"),
        (Value::F64(1.5), "05 00 00 00 00 00 00 f8 3f"),
        (Value::Str("hi".into()), "06 02 00 00 00 68 69"),
        (
            Value::Quad([1.0, 2.0, 3.0, 4.0]),
            "09 00 00 80 3f 00 00 00 40 00 00 40 40 00 00 80 40",
        ),
    ] {
        let bytes = hex(bytes);
        assert_eq!(value.to_bytes(), bytes, "{value:?}");
        assert_eq!(decode(&bytes).unwrap(), value);
    }
}

fn sample() -> Turn {
    Turn {
        ops: vec![
            NodeOp::Update {
                node: 7,
                props: vec![
                    (PropId(90), Value::color(0xFF33_6699)),
                    (PropId(322), Value::F64(0.5)),
                    (PropId(104), Value::Null),
                ],
            },
            NodeOp::Children {
                node: 1,
                children: vec![7, 8],
            },
            NodeOp::Delete { nodes: vec![9, 10] },
            NodeOp::ScrollTo {
                node: 7,
                value: Value::F64(120.0),
            },
            NodeOp::Motion { words: vec![1, 2] },
        ],
    }
}

#[test]
fn the_sample_turn_is_the_writers_bytes() {
    let bytes = hex(concat!(
        "01 02 07 00 00 00 03 00 5a 00 04 99 66 33 ff 00 00 00 00 42 01 05 00 00 00 00 00 00 e0 3f ",
        "68 00 00 03 01 00 00 00 02 00 00 00 07 00 00 00 08 00 00 00 04 02 00 00 00 09 00 00 00 0a ",
        "00 00 00 07 07 00 00 00 05 00 00 00 00 00 00 5e 40 09 02 00 00 00 01 00 00 00 02 00 00 00",
    ));
    assert_eq!(sample().to_bytes(), bytes);
    assert_eq!(Turn::from_bytes(&bytes).unwrap(), sample());
}

#[test]
fn request_frame_alone_is_the_fixed_two_bytes() {
    let t = Turn {
        ops: vec![NodeOp::RequestFrame],
    };
    assert_eq!(t.to_bytes(), [0x01, 0x08]);
    assert_eq!(Turn::new().to_bytes(), Vec::<u8>::new());
}

#[test]
fn a_press_is_the_bytes_the_decoder_reads() {
    let bytes = hex("00 00 00 00 01 01 07 00 00 00 66 02 00");
    assert_eq!(Event::press(7).to_bytes(), bytes);
    assert_eq!(
        UiEvent::from_bytes(&bytes),
        Ok(UiEvent::Node(Event::press(7)))
    );
}
