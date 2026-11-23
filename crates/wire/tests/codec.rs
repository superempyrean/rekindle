//! Byte-level fidelity of the recovered 13-tag value codec.

use rekindle_wire::{Cursor, DecodeError, PropId, Value, decode};

/// Every tag roundtrips encode -> decode exactly, and re-encodes to the same
/// bytes.
#[test]
fn every_tag_roundtrips() {
    let samples = vec![
        Value::Null,
        Value::Bool(false),
        Value::Bool(true),
        Value::I32(-123_456),
        Value::I32(i32::MIN),
        Value::I64(9_000_000_000),
        Value::I64(i64::MIN),
        Value::F64(core::f64::consts::PI),
        Value::F64(-0.0),
        Value::Str(String::new()),
        Value::Str("héllo wörld \u{1f600}".to_string()),
        Value::Array(vec![Value::Null, Value::Bool(true), Value::I32(7)]),
        Value::Object(vec![
            (PropId(90), Value::Quad([0.1, 0.2, 0.3, 1.0])),
            (PropId(129), Value::I32(5)),
        ]),
        Value::Quad([1.0, 2.5, -3.0, 0.0]),
        Value::Map(vec![
            ("one".to_string(), Value::I32(1)),
            ("two".to_string(), Value::Array(vec![Value::F64(2.0)])),
        ]),
        Value::U32Array(vec![1, 2, 3, 0xdead_beef]),
        Value::Array(vec![
            Value::U32Array(vec![1, 2, 3, 0xdead_beef]),
            Value::Sparse {
                list: 0,
                pairs: vec![(0, 10), (3, 0xffff_ffff)],
            },
        ]),
    ];

    for v in samples {
        let bytes = v.to_bytes();
        let back = decode(&bytes).unwrap_or_else(|e| panic!("decode {v:?}: {e}"));
        assert_eq!(v, back, "roundtrip mismatch for {v:?}");
        assert_eq!(bytes, back.to_bytes(), "re-encode differs for {v:?}");
    }
}

/// Nested recursion (tags 7/8/10) roundtrips.
#[test]
fn nested_roundtrips() {
    let v = Value::Array(vec![
        Value::Object(vec![(
            PropId(90),
            Value::Map(vec![(
                "k".to_string(),
                Value::Array(vec![Value::Quad([0.0; 4])]),
            )]),
        )]),
        Value::U32Array(vec![]),
    ]);
    assert_eq!(v, decode(&v.to_bytes()).unwrap());
}

/// The worked tag-8 recolour example: an object setting background (id 90)
/// and borderColor (id 104) to integer colours, `0xAARRGGBB`. Both are
/// opaque, so both are tag 4.
#[test]
fn tag8_recolour_bytes_are_exact() {
    let v = Value::Object(vec![
        (PropId(90), Value::color(0xFF33_6699)),
        (PropId(104), Value::color(0xFF00_0000)),
    ]);
    let expected = [
        0x08, 0x02, 0x00, 0x00, 0x00, // tag 8, count 2
        0x5a, 0x00, 0x04, 0x99, 0x66, 0x33, 0xff, 0, 0, 0, 0, // background
        0x68, 0x00, 0x04, 0x00, 0x00, 0x00, 0xff, 0, 0, 0, 0, // borderColor
    ];
    assert_eq!(v.to_bytes(), expected);
    assert_eq!(decode(&expected).unwrap(), v);
}

/// An integer goes out as tag 3 when it fits an `i32`, so a colour with
/// alpha below `0x80` is four bytes and an opaque one eight.
#[test]
fn integers_take_the_narrower_tag() {
    assert_eq!(Value::int(7), Value::I32(7));
    assert_eq!(Value::int(-1), Value::I32(-1));
    assert_eq!(Value::int(1 << 31), Value::I64(1 << 31));
    assert_eq!(Value::color(0x4033_6699), Value::I32(0x4033_6699));
    assert_eq!(Value::color(0xFF33_6699).as_color(), Some(0xFF33_6699));
    assert_eq!(Value::I32(-1).as_color(), None);
}

/// Known fixed-width encodings, asserted literally.
#[test]
fn scalar_bytes_are_exact() {
    assert_eq!(Value::Null.to_bytes(), vec![0x00]);
    assert_eq!(Value::Bool(false).to_bytes(), vec![0x01]);
    assert_eq!(Value::Bool(true).to_bytes(), vec![0x02]);
    assert_eq!(Value::I32(1).to_bytes(), vec![0x03, 0x01, 0x00, 0x00, 0x00]);
    assert_eq!(Value::I64(1).to_bytes(), vec![0x04, 1, 0, 0, 0, 0, 0, 0, 0]);
    // "hi" -> tag 6, len 2, 'h' 'i'
    assert_eq!(
        Value::Str("hi".to_string()).to_bytes(),
        vec![0x06, 0x02, 0, 0, 0, b'h', b'i']
    );
    // list 7, one pair (index 3 -> word 9); encoding doesn't check the table
    assert_eq!(
        Value::Sparse {
            list: 7,
            pairs: vec![(3, 9)]
        }
        .to_bytes(),
        vec![
            0x0c, 7, 0, 0, 0, /*npairs*/ 1, 0, /*index*/ 3, 0, /*value*/ 9, 0, 0, 0
        ]
    );
}

/// Trailing bytes after one value are not consumed; the cursor reports them.
#[test]
fn trailing_bytes_left_on_cursor() {
    let mut buf = Value::I32(42).to_bytes();
    buf.extend_from_slice(&[0xaa, 0xbb]);
    let mut cur = Cursor::new(&buf);
    assert_eq!(cur.value().unwrap(), Value::I32(42));
    assert_eq!(cur.remaining(), 2);
}

/// The decoder tolerates hostile input: errors, never panics (mirrors the
/// host's `-1` return).
#[test]
fn malformed_input_is_an_error() {
    assert_eq!(decode(&[]), Err(DecodeError::UnexpectedEof));
    assert_eq!(decode(&[13]), Err(DecodeError::BadTag(13)));
    assert_eq!(decode(&[255]), Err(DecodeError::BadTag(255)));
    // tag 3 (i32) with only 2 payload bytes
    assert_eq!(decode(&[3, 1, 2]), Err(DecodeError::UnexpectedEof));
    // tag 6 string claiming 4GB of bytes
    assert_eq!(
        decode(&[6, 0xff, 0xff, 0xff, 0xff]),
        Err(DecodeError::UnexpectedEof)
    );
    // tag 7 array claiming more elements than bytes remain
    assert_eq!(
        decode(&[7, 0xff, 0xff, 0xff, 0xff]),
        Err(DecodeError::LengthOverflow)
    );
    // tag 6 with invalid UTF-8
    assert_eq!(decode(&[6, 1, 0, 0, 0, 0xff]), Err(DecodeError::BadUtf8));
}

/// Tag 12 copies a words list the buffer defined earlier, patches it and
/// defines the result as the next list. A missing list or an index past the
/// list's end fails the decode, as it does on the host.
#[test]
fn tag12_patches_an_earlier_words_list() {
    let v = Value::Array(vec![
        Value::U32Array(vec![1, 2, 3]),
        Value::Sparse {
            list: 0,
            pairs: vec![(1, 20)],
        },
        Value::Sparse {
            list: 1,
            pairs: vec![(2, 30)],
        },
    ]);
    let bytes = v.to_bytes();
    let mut cur = Cursor::new(&bytes);
    assert_eq!(cur.value().unwrap(), v);
    assert_eq!(
        cur.words(),
        &[vec![1, 2, 3], vec![1, 20, 3], vec![1, 20, 30]]
    );
    let lone = Value::Sparse {
        list: 0,
        pairs: vec![],
    }
    .to_bytes();
    assert_eq!(decode(&lone), Err(DecodeError::BadWordsRef));
    let past_end = Value::Array(vec![
        Value::U32Array(vec![1]),
        Value::Sparse {
            list: 0,
            pairs: vec![(1, 5)],
        },
    ]);
    assert_eq!(decode(&past_end.to_bytes()), Err(DecodeError::BadWordsRef));
}

/// A hostile deeply-nested buffer is rejected before it can overflow the
/// stack, while a wide-but-shallow one still decodes.
#[test]
fn deeply_nested_values_are_rejected() {
    let mut deep = Value::I32(1);
    for _ in 0..200 {
        deep = Value::Array(vec![deep]);
    }
    assert_eq!(decode(&deep.to_bytes()), Err(DecodeError::TooDeep));

    let wide = Value::Array((0..500).map(Value::I32).collect());
    assert!(decode(&wide.to_bytes()).is_ok());
}
