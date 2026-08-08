// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{BodyString, BundleId};

const CROCKFORD32: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn valid_bundle_id() -> Vec<u8> {
    b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y".to_vec()
}

fn body_string(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("test code points are valid")
}

fn is_valid_byte_at(position: usize, byte: u8) -> bool {
    match position {
        0..=4 => byte == b"body-"[position],
        5 => matches!(byte, b'0'..=b'7'),
        6..=30 => CROCKFORD32.contains(&byte),
        _ => unreachable!("bundle ID position is in range"),
    }
}

fn assert_body_string_rejected_without_panic(value: BodyString) {
    let result = catch_unwind(AssertUnwindSafe(|| BundleId::from_body_string(&value)));
    assert!(result.is_ok());
    assert!(result.expect("constructor did not panic").is_err());
}

#[test]
fn bundle_id_validation_is_exhaustive_over_wire_bytes_and_ascii_code_points() {
    let valid = valid_bundle_id();

    for position in 0..valid.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut substituted = valid.clone();
            substituted[position] = byte;
            assert_eq!(
                BundleId::from_bytes(&substituted).is_ok(),
                is_valid_byte_at(position, byte),
                "raw byte {byte:#04x} at position {position}"
            );

            let mut code_points: Vec<u32> = valid.iter().copied().map(u32::from).collect();
            code_points[position] = u32::from(byte);
            assert_eq!(
                BundleId::from_body_string(&body_string(code_points)).is_ok(),
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
                BundleId::from_bytes(&inserted).is_err(),
                "raw insertion {byte:#04x} at position {position}"
            );
            let code_points = inserted.iter().copied().map(u32::from).collect();
            assert!(
                BundleId::from_body_string(&body_string(code_points)).is_err(),
                "code-point insertion {byte:#04x} at position {position}"
            );
        }
    }

    let minimum = format!("body-{}", "0".repeat(26));
    let maximum = format!("body-7{}", "Z".repeat(25));
    assert!(BundleId::from_bytes(minimum.as_bytes()).is_ok());
    assert!(BundleId::from_bytes(maximum.as_bytes()).is_ok());

    let mut lowercase = valid.clone();
    lowercase[6] = b'a';
    assert!(BundleId::from_bytes(&lowercase).is_err());
    for length in 0..valid.len() {
        assert!(BundleId::from_bytes(&valid[..length]).is_err());
    }

    let mut oversized = valid.clone();
    oversized.extend(vec![b'x'; 1_048_576]);
    assert!(BundleId::from_bytes(&oversized).is_err());

    let mut invalid_utf8 = valid.clone();
    invalid_utf8[6] = 0xff;
    invalid_utf8[7] = 0xfe;
    let result = catch_unwind(AssertUnwindSafe(|| BundleId::from_bytes(&invalid_utf8)));
    assert!(result.is_ok());
    assert!(result.expect("constructor did not panic").is_err());

    for code_point in [0x1f600, 0xd800, 0xdfff] {
        let mut code_points: Vec<u32> = valid.iter().copied().map(u32::from).collect();
        code_points[6] = code_point;
        assert_body_string_rejected_without_panic(body_string(code_points));
    }
}
