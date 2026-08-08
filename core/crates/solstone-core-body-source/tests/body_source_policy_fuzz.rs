// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{
    BodyRawRetention, BodySourceFamily, BodySourcePolicyError, BodySourcePolicyField, BodyString,
};

const SOURCE_FAMILY_LITERALS: &[&[u8]] = &[b"apple_health", b"oura_api"];
const RAW_RETENTION_LITERALS: &[&[u8]] = &[b"discard", b"retain_complete", b"retain_parsed"];

fn body_string(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("test code points are valid")
}

fn assert_distinct_lengths(literals: &[&[u8]]) {
    for (index, literal) in literals.iter().enumerate() {
        for other in &literals[index + 1..] {
            assert_ne!(literal.len(), other.len());
        }
    }
}

fn assert_body_string_rejected_without_panic<F>(value: BodyString, from_body_string: F)
where
    F: Fn(&BodyString) -> Result<(), BodySourcePolicyError>,
{
    let result = catch_unwind(AssertUnwindSafe(|| from_body_string(&value)));
    assert!(result.is_ok());
    assert!(result.expect("constructor did not panic").is_err());
}

fn assert_literal_validation<F, G>(literals: &[&[u8]], from_bytes: F, from_body_string: G)
where
    F: Fn(&[u8]) -> bool,
    G: Fn(&BodyString) -> bool,
{
    for literal in literals {
        for position in 0..literal.len() {
            for byte in u8::MIN..=u8::MAX {
                let mut substituted = literal.to_vec();
                substituted[position] = byte;
                assert_eq!(
                    from_bytes(&substituted),
                    byte == literal[position],
                    "raw byte {byte:#04x} at position {position}"
                );

                let mut code_points: Vec<u32> = literal.iter().copied().map(u32::from).collect();
                code_points[position] = u32::from(byte);
                assert_eq!(
                    from_body_string(&body_string(code_points)),
                    byte == literal[position],
                    "code point {byte:#04x} at position {position}"
                );
            }
        }

        for length in 0..literal.len() {
            assert!(!from_bytes(&literal[..length]));
        }

        for position in 0..=literal.len() {
            for byte in u8::MIN..=u8::MAX {
                let mut inserted = literal.to_vec();
                inserted.insert(position, byte);
                assert!(
                    !from_bytes(&inserted),
                    "raw insertion {byte:#04x} at position {position}"
                );
            }
        }
    }

    let mut oversized = literals[0].to_vec();
    oversized.extend(vec![b'x'; 1_048_576]);
    assert!(!from_bytes(&oversized));
}

#[test]
fn source_policy_validation_is_exhaustive_over_closed_wire_literals() {
    assert_distinct_lengths(SOURCE_FAMILY_LITERALS);
    assert_distinct_lengths(RAW_RETENTION_LITERALS);

    assert_literal_validation(
        SOURCE_FAMILY_LITERALS,
        |bytes| BodySourceFamily::from_bytes(bytes).is_ok(),
        |value| BodySourceFamily::from_body_string(value).is_ok(),
    );
    assert_literal_validation(
        RAW_RETENTION_LITERALS,
        |bytes| BodyRawRetention::from_bytes(bytes).is_ok(),
        |value| BodyRawRetention::from_body_string(value).is_ok(),
    );

    for literal in [SOURCE_FAMILY_LITERALS[0], RAW_RETENTION_LITERALS[0]] {
        let mut invalid_utf8 = literal.to_vec();
        invalid_utf8[0] = 0xff;
        invalid_utf8[1] = 0xfe;
        let result = catch_unwind(AssertUnwindSafe(|| {
            if literal == SOURCE_FAMILY_LITERALS[0] {
                BodySourceFamily::from_bytes(&invalid_utf8).is_err()
            } else {
                BodyRawRetention::from_bytes(&invalid_utf8).is_err()
            }
        }));
        assert!(result.is_ok());
        assert!(result.expect("constructor did not panic"));
    }

    for code_point in [0x1f600, 0x2603, 0xd800, 0xdfff] {
        let mut source_code_points: Vec<u32> = SOURCE_FAMILY_LITERALS[0]
            .iter()
            .copied()
            .map(u32::from)
            .collect();
        source_code_points[0] = code_point;
        assert_body_string_rejected_without_panic(body_string(source_code_points), |value| {
            BodySourceFamily::from_body_string(value).map(|_| ())
        });

        let mut retention_code_points: Vec<u32> = RAW_RETENTION_LITERALS[0]
            .iter()
            .copied()
            .map(u32::from)
            .collect();
        retention_code_points[0] = code_point;
        assert_body_string_rejected_without_panic(body_string(retention_code_points), |value| {
            BodyRawRetention::from_body_string(value).map(|_| ())
        });
    }

    for family in [BodySourceFamily::AppleHealth, BodySourceFamily::OuraApi] {
        for retention in [
            BodyRawRetention::Discard,
            BodyRawRetention::RetainComplete,
            BodyRawRetention::RetainParsed,
        ] {
            let result = catch_unwind(AssertUnwindSafe(|| retention.check_compatible(&family)));
            assert!(result.is_ok());
            let expected = if family == BodySourceFamily::OuraApi
                && retention == BodyRawRetention::RetainComplete
            {
                Err(BodySourcePolicyError::Incompatible(
                    BodySourcePolicyField::RawRetention,
                ))
            } else {
                Ok(())
            };
            assert_eq!(result.expect("compatibility check did not panic"), expected);
        }
    }
}
