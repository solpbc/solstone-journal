// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{BodyDigest, BodyString};

fn valid_digest() -> Vec<u8> {
    b"sha256:dc9b29d0ee818f2ae3cdd600a15066f4404002171a4eb99a39118b88303bd71b".to_vec()
}

fn body_string(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("test code points are valid")
}

fn is_valid_byte_at(position: usize, byte: u8) -> bool {
    match position {
        0..=6 => byte == b"sha256:"[position],
        7..=70 => byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
        _ => unreachable!("digest position is in range"),
    }
}

fn assert_body_string_rejected_without_panic(value: BodyString) {
    let result = catch_unwind(AssertUnwindSafe(|| BodyDigest::from_body_string(&value)));
    assert!(result.is_ok());
    assert!(result.expect("constructor did not panic").is_err());
}

#[test]
fn digest_validation_is_exhaustive_over_wire_bytes_and_ascii_code_points() {
    let valid = valid_digest();

    for position in 0..valid.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut substituted = valid.clone();
            substituted[position] = byte;
            assert_eq!(
                BodyDigest::from_bytes(&substituted).is_ok(),
                is_valid_byte_at(position, byte),
                "raw byte {byte:#04x} at position {position}"
            );

            let mut code_points: Vec<u32> = valid.iter().copied().map(u32::from).collect();
            code_points[position] = u32::from(byte);
            assert_eq!(
                BodyDigest::from_body_string(&body_string(code_points)).is_ok(),
                is_valid_byte_at(position, byte),
                "code point {byte:#04x} at position {position}"
            );
        }
    }

    for position in 0..=valid.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut inserted = valid.clone();
            inserted.insert(position, byte);
            assert!(
                BodyDigest::from_bytes(&inserted).is_err(),
                "raw insertion {byte:#04x} at position {position}"
            );
            let code_points = inserted.iter().copied().map(u32::from).collect();
            assert!(
                BodyDigest::from_body_string(&body_string(code_points)).is_err(),
                "code-point insertion {byte:#04x} at position {position}"
            );
        }
    }

    let minimum = format!("sha256:{}", "0".repeat(64));
    let maximum = format!("sha256:{}", "f".repeat(64));
    assert!(BodyDigest::from_bytes(minimum.as_bytes()).is_ok());
    assert!(BodyDigest::from_bytes(maximum.as_bytes()).is_ok());

    let mut uppercase = valid.clone();
    uppercase[7] = b'A';
    assert!(BodyDigest::from_bytes(&uppercase).is_err());
    for length in 0..valid.len() {
        assert!(BodyDigest::from_bytes(&valid[..length]).is_err());
    }

    let mut oversized = valid.clone();
    oversized.extend(vec![b'x'; 1_048_576]);
    assert!(BodyDigest::from_bytes(&oversized).is_err());

    let mut invalid_utf8 = valid.clone();
    invalid_utf8[7] = 0xff;
    invalid_utf8[8] = 0xfe;
    let result = catch_unwind(AssertUnwindSafe(|| BodyDigest::from_bytes(&invalid_utf8)));
    assert!(result.is_ok());
    assert!(result.expect("constructor did not panic").is_err());

    for code_point in [0x1f600, 0xd800, 0xdfff] {
        let mut code_points: Vec<u32> = valid.iter().copied().map(u32::from).collect();
        code_points[7] = code_point;
        assert_body_string_rejected_without_panic(body_string(code_points));
    }
}
