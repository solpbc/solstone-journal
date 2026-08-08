// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{BodySourceFamily, BodySourceHash, BodyString};

fn body_string(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("test code points are valid")
}

fn assert_case(bytes: &[u8], family: &BodySourceFamily, expected: bool) {
    let from_bytes = catch_unwind(AssertUnwindSafe(|| {
        BodySourceHash::from_bytes_for_family(bytes, family)
    }));
    assert!(
        from_bytes.is_ok(),
        "byte constructor panicked for {bytes:?}"
    );
    assert_eq!(
        from_bytes.expect("byte constructor did not panic").is_ok(),
        expected,
        "unexpected byte result for family {:?} and bytes {bytes:?}",
        family
    );

    let value = body_string(bytes.iter().copied().map(u32::from).collect());
    let from_body_string = catch_unwind(AssertUnwindSafe(|| {
        BodySourceHash::from_body_string_for_family(&value, family)
    }));
    assert!(
        from_body_string.is_ok(),
        "body-string constructor panicked for {bytes:?}"
    );
    assert_eq!(
        from_body_string
            .expect("body-string constructor did not panic")
            .is_ok(),
        expected,
        "unexpected body-string result for family {:?} and bytes {bytes:?}",
        family
    );
}

#[test]
fn source_hash_validation_rejects_every_hand_authored_adversarial_case() {
    let base = "a".repeat(64);
    let apple = BodySourceFamily::AppleHealth;
    let oura = BodySourceFamily::OuraApi;
    let valid_windows = [
        format!("{base}#window:open:20260102"),
        format!("{base}#window:20260101:open"),
        format!("{base}#window:20260101:20260102"),
        format!("{base}#window:20260102:20260102"),
    ];

    for spelling in [
        &base,
        &valid_windows[0],
        &valid_windows[1],
        &valid_windows[2],
        &valid_windows[3],
    ] {
        assert_case(spelling.as_bytes(), &apple, true);
    }
    assert_case(base.as_bytes(), &oura, true);
    for spelling in &valid_windows {
        assert_case(spelling.as_bytes(), &oura, false);
    }

    let mut uppercase = base.clone().into_bytes();
    uppercase[0] = b'A';
    assert_case(&uppercase, &apple, false);
    let mut nonhex = base.clone().into_bytes();
    nonhex[1] = b'g';
    assert_case(&nonhex, &apple, false);
    assert_case("a".repeat(63).as_bytes(), &apple, false);
    assert_case("a".repeat(65).as_bytes(), &apple, false);

    for invalid_day in ["20240230", "20241301", "20240001", "20240100", "20230229"] {
        assert_case(
            format!("{base}#window:{invalid_day}:20260102").as_bytes(),
            &apple,
            false,
        );
    }
    for spelling in [
        format!("{base}#window:open:open"),
        format!("{base}#window:20260103:20260102"),
        format!("{base}window:20260101:20260102"),
        format!("{base}#windows:20260101:20260102"),
        format!("{base}#window"),
        format!("{base}#window:20260101"),
        format!("{base}#window::20260102"),
        format!("{base}#window:20260101:20260102:extra"),
        format!(" {base}"),
        format!("{base} "),
        format!("{base}#window:20260101:20260102 "),
        format!("{base}trailing"),
    ] {
        assert_case(spelling.as_bytes(), &apple, false);
    }

    let mut invalid_utf8 = base.clone().into_bytes();
    invalid_utf8[0] = 0xff;
    invalid_utf8[1] = 0xfe;
    assert_case(&invalid_utf8, &apple, false);
}

#[test]
fn source_hash_body_strings_reject_non_ascii_code_points_without_panicking() {
    let anchor = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    for code_point in [0x0100, 0xffff, 0x1f600, 0xd800] {
        let mut code_points: Vec<u32> = anchor.iter().copied().map(u32::from).collect();
        code_points[0] = code_point;
        let value = body_string(code_points);
        for family in [BodySourceFamily::AppleHealth, BodySourceFamily::OuraApi] {
            let result = catch_unwind(AssertUnwindSafe(|| {
                BodySourceHash::from_body_string_for_family(&value, &family)
            }));
            assert!(result.is_ok(), "body-string constructor panicked");
            assert!(
                result
                    .expect("body-string constructor did not panic")
                    .is_err()
            );
        }
    }
}
