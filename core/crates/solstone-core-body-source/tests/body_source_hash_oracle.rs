// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{BodyDay, BodySourceFamily, BodySourceHash, BodyString};

const HASH_LENGTH: usize = 64;
const WINDOW_PREFIX: &[u8] = b"#window:";

fn body_string(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("test code points are valid")
}

fn oracle_valid(bytes: &[u8], family: &BodySourceFamily) -> bool {
    if bytes.len() < HASH_LENGTH
        || !bytes[..HASH_LENGTH]
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return false;
    }

    let suffix = &bytes[HASH_LENGTH..];
    if suffix.is_empty() {
        return true;
    }
    if !matches!(family, BodySourceFamily::AppleHealth) || !suffix.starts_with(WINDOW_PREFIX) {
        return false;
    }

    let mut bounds = suffix[WINDOW_PREFIX.len()..].split(|byte| *byte == b':');
    let (Some(start), Some(end), None) = (bounds.next(), bounds.next(), bounds.next()) else {
        return false;
    };
    let Some(start) = oracle_bound(start) else {
        return false;
    };
    let Some(end) = oracle_bound(end) else {
        return false;
    };
    if start.is_none() && end.is_none() {
        return false;
    }
    !matches!((&start, &end), (Some(start), Some(end)) if start > end)
}

fn oracle_bound(bytes: &[u8]) -> Option<Option<BodyDay>> {
    if bytes == b"open" {
        Some(None)
    } else {
        BodyDay::from_bytes(bytes).ok().map(Some)
    }
}

fn assert_bytes_match_oracle(bytes: &[u8], family: &BodySourceFamily) {
    let expected = oracle_valid(bytes, family);
    let actual = catch_unwind(AssertUnwindSafe(|| {
        BodySourceHash::from_bytes_for_family(bytes, family)
    }));
    assert!(actual.is_ok(), "byte constructor panicked");
    assert_eq!(
        actual.expect("byte constructor did not panic").is_ok(),
        expected,
        "byte result differs for family {:?} and bytes {bytes:?}",
        family
    );
}

fn assert_code_points_match_oracle(code_points: Vec<u32>, family: &BodySourceFamily) {
    let expected = if code_points.iter().any(|code_point| *code_point > 0x7f) {
        false
    } else {
        let bytes = code_points
            .iter()
            .copied()
            .map(|code_point| code_point as u8)
            .collect::<Vec<_>>();
        oracle_valid(&bytes, family)
    };
    let value = body_string(code_points);
    let actual = catch_unwind(AssertUnwindSafe(|| {
        BodySourceHash::from_body_string_for_family(&value, family)
    }));
    assert!(actual.is_ok(), "body-string constructor panicked");
    assert_eq!(
        actual
            .expect("body-string constructor did not panic")
            .is_ok(),
        expected,
        "body-string result differs for family {:?}",
        family
    );
}

fn exercise_anchor(anchor: &[u8]) {
    let families = [BodySourceFamily::AppleHealth, BodySourceFamily::OuraApi];
    for family in &families {
        assert_bytes_match_oracle(anchor, family);
    }

    for position in 0..anchor.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut substituted = anchor.to_vec();
            substituted[position] = byte;
            for family in &families {
                assert_bytes_match_oracle(&substituted, family);
            }
        }
    }
    for position in 0..=anchor.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut inserted = anchor.to_vec();
            inserted.insert(position, byte);
            for family in &families {
                assert_bytes_match_oracle(&inserted, family);
            }
        }
    }
    for position in 0..anchor.len() {
        let mut deleted = anchor.to_vec();
        deleted.remove(position);
        for family in &families {
            assert_bytes_match_oracle(&deleted, family);
        }
    }
    for length in 0..anchor.len() {
        for family in &families {
            assert_bytes_match_oracle(&anchor[..length], family);
        }
    }

    for position in 0..anchor.len() {
        for code_point in 0_u32..=0xff {
            let mut substituted: Vec<u32> = anchor.iter().copied().map(u32::from).collect();
            substituted[position] = code_point;
            for family in &families {
                assert_code_points_match_oracle(substituted.clone(), family);
            }
        }
    }
}

#[test]
fn source_hash_constructors_match_the_independent_family_bound_oracle() {
    let plain = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let two_real = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#window:20260101:20260102";
    let left_open =
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#window:open:20260102";
    for anchor in [plain.as_slice(), two_real.as_slice(), left_open.as_slice()] {
        exercise_anchor(anchor);
    }

    let mut oversized = plain.to_vec();
    oversized.extend(vec![b'x'; 1_048_576]);
    for family in [BodySourceFamily::AppleHealth, BodySourceFamily::OuraApi] {
        assert_bytes_match_oracle(&oversized, &family);
    }
}
