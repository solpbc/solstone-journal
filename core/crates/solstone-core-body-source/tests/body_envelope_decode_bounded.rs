// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{EnvelopeErrorCode, EnvelopeErrorField, decode_body_envelope};

mod support;

use support::native_bundle_fixture;

const MAX_ENVELOPE_BYTES: usize = 1_048_576;

fn valid_jsonl() -> String {
    native_bundle_fixture()["cases"][0]["expected_envelope_jsonl"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn with_unknown_key(key: &str) -> Vec<u8> {
    let base = valid_jsonl();
    format!(r#"{{"{key}":null,{}"#, &base[1..]).into_bytes()
}

fn assert_bounded_redacted(
    input: &[u8],
    sentinel: &str,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
) {
    let error = decode_body_envelope(input).unwrap_err();
    let display = error.to_string();
    assert_eq!(display, format!("{error:?}"));
    assert!(display.len() <= 160);
    assert!(Error::source(&error).is_none());
    assert!(!display.contains(sentinel));
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
}

#[test]
fn decoder_accepts_the_exact_limit_then_rejects_one_byte_over() {
    let base = valid_jsonl();
    let key_len = MAX_ENVELOPE_BYTES - base.len() - 8;
    let exact = with_unknown_key(&"a".repeat(key_len));
    assert_eq!(exact.len(), MAX_ENVELOPE_BYTES);
    let error = decode_body_envelope(&exact).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::UnknownField);
    assert_eq!(error.field(), EnvelopeErrorField::Envelope);

    let mut over = exact.clone();
    over.push(b'x');
    let error = decode_body_envelope(&over).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InputTooLarge);
    assert_eq!(error.field(), EnvelopeErrorField::Envelope);
}

#[test]
fn megabyte_scale_unknown_errors_are_bounded_redacting_and_non_mutating() {
    let sentinel = "body-envelope-decode-private-sentinel";
    let input = with_unknown_key(&sentinel.repeat(20_000));
    let snapshot = input.clone();
    assert_bounded_redacted(
        &input,
        sentinel,
        EnvelopeErrorCode::UnknownField,
        EnvelopeErrorField::Envelope,
    );
    assert_eq!(input, snapshot);
}

#[test]
fn scanner_failures_surface_unchanged_through_the_public_decoder() {
    for (input, code) in [
        (br#"{"#.as_slice(), EnvelopeErrorCode::MalformedJson),
        (b" {}\n".as_slice(), EnvelopeErrorCode::NoncanonicalJson),
        (b"null\n".as_slice(), EnvelopeErrorCode::WrongType),
    ] {
        let error = decode_body_envelope(input).unwrap_err();
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), EnvelopeErrorField::Envelope);
        assert_eq!(error.bundle(), None);
        assert_eq!(error.index(), None);
    }
}

#[test]
fn every_proper_fixture_prefix_is_panic_safe() {
    let input = valid_jsonl();
    for length in 0..input.len() {
        assert!(
            catch_unwind(AssertUnwindSafe(|| decode_body_envelope(
                &input.as_bytes()[..length]
            )))
            .is_ok(),
            "prefix {length} panicked"
        );
    }
}

#[test]
fn megabyte_scale_invalid_values_and_surrogate_paths_remain_redacted() {
    let sentinel = "body-envelope-decode-invalid-sentinel";
    let huge = sentinel.repeat(18_000);
    let invalid_hash = valid_jsonl().replace(
        "\"source_hash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#window:20260102:20260102\"",
        &format!("\"source_hash\":\"{huge}\""),
    );
    assert_bounded_redacted(
        invalid_hash.as_bytes(),
        sentinel,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::SourceHash,
    );

    let surrogate_path = valid_jsonl().replace(
        "\"path\":\"normalized/2026-01.jsonl\"",
        &format!("\"path\":\"normalized/\\ud800{huge}.jsonl\""),
    );
    assert_bounded_redacted(
        surrogate_path.as_bytes(),
        sentinel,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardPath,
    );
}
