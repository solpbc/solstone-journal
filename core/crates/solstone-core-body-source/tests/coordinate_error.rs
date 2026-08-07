// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{
    CandidateError, CandidateErrorCode, CandidateErrorField, Coordinate,
};

const INVALID_ASCII_PUNCTUATION: &[u8] = b"!\"#$%&'()*+,/:;<=>?@[\\]^`{|}~";

fn assert_invalid_component(bytes: &[u8]) {
    let bundle_invalid = Coordinate::new(bytes, "valid-shard", 1);
    assert_eq!(bundle_invalid.bundle(), "<invalid>");
    assert_eq!(bundle_invalid.shard(), "valid-shard");

    let shard_invalid = Coordinate::new("valid-bundle", bytes, 1);
    assert_eq!(shard_invalid.bundle(), "valid-bundle");
    assert_eq!(shard_invalid.shard(), "<invalid>");
}

#[test]
fn coordinate_accepts_all_valid_component_boundaries() {
    for byte in b'a'..=b'z' {
        let coordinate = Coordinate::new([byte], "shard", 1);
        assert_eq!(coordinate.bundle().as_bytes(), &[byte]);
    }
    for byte in b'A'..=b'Z' {
        let coordinate = Coordinate::new([byte], "shard", 1);
        assert_eq!(coordinate.bundle().as_bytes(), &[byte]);
    }
    for byte in b'0'..=b'9' {
        let coordinate = Coordinate::new([byte], "shard", 1);
        assert_eq!(coordinate.bundle().as_bytes(), &[byte]);
    }
    for byte in *b"_-" {
        let coordinate = Coordinate::new([byte], "shard", 1);
        assert_eq!(coordinate.bundle().as_bytes(), &[byte]);
    }

    let mixed = Coordinate::new("Az09_.-Za", "shard", 1);
    assert_eq!(mixed.bundle(), "Az09_.-Za");
    let longest = "a".repeat(93);
    let coordinate = Coordinate::new(&longest, "shard", 1);
    assert_eq!(coordinate.bundle(), longest);
}

#[test]
fn coordinate_redacts_every_invalid_component_category() {
    for component in [b"".as_slice(), b".".as_slice(), b"..".as_slice()] {
        assert_invalid_component(component);
    }
    assert_invalid_component("a".repeat(94).as_bytes());
    for byte in INVALID_ASCII_PUNCTUATION {
        assert_invalid_component(&[*byte]);
    }
    for byte in 0x00..=0x1f {
        assert_invalid_component(&[byte]);
    }
    assert_invalid_component(&[0x7f]);
    for code_point in 0x0080..=0x009f {
        let control = char::from_u32(code_point)
            .expect("C1 controls are Unicode scalars")
            .to_string();
        assert_invalid_component(control.as_bytes());
    }
    for component in ["café", "日本語", "🫀"] {
        assert_invalid_component(component.as_bytes());
    }
    assert_invalid_component(&[0xff, 0xfe]);
}

#[test]
fn coordinate_line_and_error_rendering_are_bounded_and_redacted() {
    assert_eq!(Coordinate::new("b", "s", 0).line(), None);
    assert_eq!(Coordinate::new("b", "s", 1).line(), Some(1));
    assert_eq!(Coordinate::new("b", "s", u64::MAX).line(), Some(u64::MAX));
    assert_eq!(Coordinate::new("b", "s", 0).to_string(), "b/s#L<invalid>");
    assert_eq!(Coordinate::new("b", "s", 1).to_string(), "b/s#L1");
    assert_eq!(
        Coordinate::new("b", "s", u64::MAX).to_string(),
        format!("b/s#L{}", u64::MAX)
    );

    let invalid_line = CandidateError {
        coordinate: Coordinate::new("b", "s", 0),
        code: CandidateErrorCode::WrongType,
        field: CandidateErrorField::Row,
    };
    assert_eq!(
        invalid_line.to_string(),
        "body-row[b/s#L<invalid>] wrong_type: row"
    );
    assert_eq!(
        format!("{invalid_line:?}"),
        "body-row[b/s#L<invalid>] wrong_type: row"
    );

    let representative = CandidateError {
        coordinate: Coordinate::new("bundle", "shard", 7),
        code: CandidateErrorCode::WrongType,
        field: CandidateErrorField::Row,
    };
    assert_eq!(
        representative.to_string(),
        "body-row[bundle/shard#L7] wrong_type: row"
    );
    assert_eq!(format!("{representative:?}"), format!("{representative}"));
    assert!(Error::source(&representative).is_none());

    let arbitrary = CandidateError {
        coordinate: Coordinate::new("bundle", "shard", 19),
        code: CandidateErrorCode::UnsupportedSchema,
        field: CandidateErrorField::SourceRecordId,
    };
    assert_eq!(
        arbitrary.to_string(),
        "body-row[bundle/shard#L19] unsupported_schema: source_record_id"
    );

    let maximum = CandidateError {
        coordinate: Coordinate::new("a".repeat(93), "b".repeat(93), u64::MAX),
        code: CandidateErrorCode::UnsupportedSchema,
        field: CandidateErrorField::SourceRecordId,
    };
    assert_eq!(maximum.to_string().len(), 256);
    assert_eq!(format!("{maximum:?}").len(), 256);

    let sentinel = "sentinel-bundle-must-not-appear";
    let mut megabyte = vec![b'/'; 1_048_576];
    megabyte[524_288..524_288 + sentinel.len()].copy_from_slice(sentinel.as_bytes());
    let coordinate = Coordinate::new(&megabyte, "shard", 42);
    let coordinate_display = coordinate.to_string();
    let coordinate_debug = format!("{coordinate:?}");
    assert_eq!(coordinate_display, "<invalid>/shard#L42");
    assert!(!coordinate_display.contains(sentinel));
    assert!(!coordinate_debug.contains(sentinel));
    assert!(coordinate_display.len() <= 256 && coordinate_debug.len() <= 256);

    let redacted_error = CandidateError {
        coordinate,
        code: CandidateErrorCode::UnsupportedSchema,
        field: CandidateErrorField::SourceRecordId,
    };
    let error_display = redacted_error.to_string();
    let error_debug = format!("{redacted_error:?}");
    assert_eq!(
        error_display,
        "body-row[<invalid>/shard#L42] unsupported_schema: source_record_id"
    );
    assert!(!error_display.contains(sentinel));
    assert!(!error_debug.contains(sentinel));
    assert!(error_display.len() <= 256 && error_debug.len() <= 256);
}
