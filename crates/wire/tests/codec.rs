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
            (PropId(90), Value::Vec4([0.1, 0.2, 0.3, 1.0])),
            (PropId(129), Value::I32(5)),
        ]),
        Value::Vec4([1.0, 2.5, -3.0, 0.0]),
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
                Value::Array(vec![Value::Vec4([0.0; 4])]),
            )]),
        )]),
        Value::U32Array(vec![]),
    ]);
    assert_eq!(v, decode(&v.to_bytes()).unwrap());
}

/// The worked tag-8 recolour example: an object
/// setting background (id 90) and borderColor (id 104) to vec4 colours. The
/// exact byte layout is asserted, structure byte-for-byte and colours by their
/// f32 little-endian encodings.
#[test]
fn tag8_recolour_bytes_are_exact() {
    let bg = [0.1f32, 0.2, 0.3, 1.0];
    let border = [0.4f32, 0.5, 0.6, 1.0];
    let v = Value::Object(vec![
        (PropId(90), Value::Vec4(bg)),
        (PropId(104), Value::Vec4(border)),
    ]);

    let mut expected = Vec::new();
    expected.push(0x08); //                         tag 8, object by PropId
    expected.extend_from_slice(&2u32.to_le_bytes()); // count = 2  -> 02 00 00 00
    expected.extend_from_slice(&90u16.to_le_bytes()); // prop_id 90 (background) -> 5a 00
    expected.push(0x09); //                         value: tag 9, Vec4<f32>
    for f in bg {
        expected.extend_from_slice(&f.to_le_bytes());
    }
    expected.extend_from_slice(&104u16.to_le_bytes()); // prop_id 104 (borderColor) -> 68 00
    expected.push(0x09);
    for f in border {
        expected.extend_from_slice(&f.to_le_bytes());
    }

    let actual = v.to_bytes();
    assert_eq!(actual, expected, "tag-8 recolour bytes differ");

    // Spot-check the structural bytes the doc spells out, independent of the
    // f32 encodings: 08 | 02 00 00 00 | 5a 00 | 09 | ...16... | 68 00 | 09
    assert_eq!(&actual[0..7], &[0x08, 0x02, 0x00, 0x00, 0x00, 0x5a, 0x00]);
    assert_eq!(actual[7], 0x09);
    assert_eq!(&actual[24..27], &[0x68, 0x00, 0x09]);
    assert_eq!(actual.len(), 1 + 4 + 2 + 17 + 2 + 17);

    assert_eq!(decode(&actual).unwrap(), v);
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
        Value::Sparse { list: 0, pairs: vec![(1, 20)] },
        Value::Sparse { list: 1, pairs: vec![(2, 30)] },
    ]);
    let bytes = v.to_bytes();
    let mut cur = Cursor::new(&bytes);
    assert_eq!(cur.value().unwrap(), v);
    assert_eq!(cur.words(), &[vec![1, 2, 3], vec![1, 20, 3], vec![1, 20, 30]]);
    let lone = Value::Sparse { list: 0, pairs: vec![] }.to_bytes();
    assert_eq!(decode(&lone), Err(DecodeError::BadWordsRef));
    let past_end = Value::Array(vec![
        Value::U32Array(vec![1]),
        Value::Sparse { list: 0, pairs: vec![(1, 5)] },
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
