// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::body_envelope_encode::MAX_ENVELOPE_BYTES;
use crate::canonicalize::{CanonicalSink, CanonicalizeValueError, canonicalize_value};
use crate::{BodyObject, BodyValue, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField};

/// A parsed body envelope whose bytes are exact canonical JSONL.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScannedBodyEnvelope {
    object: BodyObject,
}

impl ScannedBodyEnvelope {
    /// Returns the parsed top-level envelope object.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by B1g6b envelope decoder")
    )]
    pub(crate) fn object(&self) -> &BodyObject {
        &self.object
    }
}

/// Scans bounded raw bytes as an exact canonical body-envelope JSONL object.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by B1g6b envelope decoder")
)]
pub(crate) fn scan_body_envelope(input: &[u8]) -> Result<ScannedBodyEnvelope, EnvelopeError> {
    if input.len() > MAX_ENVELOPE_BYTES {
        return Err(envelope_error(EnvelopeErrorCode::InputTooLarge));
    }

    let candidate = input.strip_suffix(b"\n").unwrap_or(input);
    let value = crate::parser::parse(candidate)
        .map_err(|_| envelope_error(EnvelopeErrorCode::MalformedJson))?;
    if !matches!(value, BodyValue::Object(_)) {
        return Err(envelope_error(EnvelopeErrorCode::WrongType));
    }

    let mut sink = ComparingSink::new(input);
    let outcome = canonicalize_value(&value, 0, &mut sink).and_then(|()| {
        sink.write_bytes(b"\n")
            .map_err(CanonicalizeValueError::Sink)
    });
    match outcome {
        Ok(()) if sink.is_fully_consumed() => {
            let BodyValue::Object(object) = value else {
                unreachable!("checked above")
            };
            Ok(ScannedBodyEnvelope { object })
        }
        Ok(()) | Err(CanonicalizeValueError::Sink(_)) => {
            Err(envelope_error(EnvelopeErrorCode::NoncanonicalJson))
        }
        Err(CanonicalizeValueError::Canonicalize(_)) => unreachable!(
            "value already parsed within MAX_NESTING, so re-canonicalizing cannot exceed it"
        ),
    }
}

fn envelope_error(code: EnvelopeErrorCode) -> EnvelopeError {
    EnvelopeError::new(None, code, EnvelopeErrorField::Envelope, None)
}

struct ComparingSink<'a> {
    remaining: &'a [u8],
}

impl<'a> ComparingSink<'a> {
    fn new(target: &'a [u8]) -> Self {
        Self { remaining: target }
    }

    fn is_fully_consumed(&self) -> bool {
        self.remaining.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SinkMismatch;

impl CanonicalSink for ComparingSink<'_> {
    type Error = SinkMismatch;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        if !self.remaining.starts_with(bytes) {
            return Err(SinkMismatch);
        }
        self.remaining = &self.remaining[bytes.len()..];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use crate::parser::test_corpus::{
        alternating_containers, assert_body_value_bitwise_eq, fixture, fixture_inputs,
        generated_corpus, repeated_objects, string_field,
    };
    use crate::{BodyValue, EnvelopeErrorCode, EnvelopeErrorField, canonicalize, parse};

    use super::{MAX_ENVELOPE_BYTES, scan_body_envelope};

    const NATIVE_CASES: [&str; 4] = [
        "apple_retain_complete_one_row",
        "oura_retain_parsed_one_row",
        "apple_discard_zero_rows",
        "oura_discard_zero_rows",
    ];

    #[test]
    fn native_fixture_envelope_lines_scan_and_round_trip() {
        let inputs = fixture_inputs();
        for case_name in NATIVE_CASES {
            let expected_name = format!("native {case_name} expected_envelope_jsonl line 1");
            let (_, line) = inputs
                .iter()
                .find(|(name, _)| name == &expected_name)
                .unwrap_or_else(|| panic!("missing fixture input {expected_name}"));
            let mut input = line.clone();
            input.push(b'\n');
            assert_success_and_round_trip(&input);
        }
    }

    #[test]
    fn multimonth_fixture_envelope_scans_and_round_trips() {
        let fixture = fixture("body_source_envelope_multimonth.json");
        let case = &fixture["cases"][0];
        assert_success_and_round_trip(string_field(case, "expected_envelope_jsonl").as_bytes());
    }

    #[test]
    fn python_vectors_and_generated_corpus_classify_canonicality() {
        let vectors = fixture("body_source_python_json_vectors.json");
        for case in vectors["canonical_cases"]
            .as_array()
            .expect("canonical cases")
        {
            let raw = string_field(case, "raw_json");
            let expected = string_field(case, "expected_canonical_json");
            assert_ne!(
                raw,
                expected,
                "{} must be noncanonical",
                string_field(case, "name")
            );
            assert_error(&wrap(raw.as_bytes()), EnvelopeErrorCode::NoncanonicalJson);
            assert_success(&wrap(expected.as_bytes()));
        }

        let corpus = generated_corpus();
        for (name, expected) in [
            ("nested arrays and objects", true),
            ("NaN", true),
            ("Infinity", true),
            ("negative Infinity", true),
            ("fixed lower exponent boundary", false),
            ("scientific lower exponent boundary", false),
            ("fixed upper exponent boundary", false),
            ("scientific upper exponent boundary", false),
            ("4300-digit integer", true),
            ("member order", false),
            ("insignificant whitespace", false),
            ("escaped key characters", false),
            ("astral and lone surrogate keys", false),
        ] {
            let raw = corpus_value(&corpus, name);
            let input = wrap(raw);
            if expected {
                assert_success(&input);
            } else {
                let error = scan_body_envelope(&input).expect_err("case should be rejected");
                assert_eq!(error.code(), EnvelopeErrorCode::NoncanonicalJson, "{name}");
            }
        }

        for name in [
            "malformed exponent",
            "malformed decimal",
            "malformed negative",
            "malformed escape",
            "incomplete Unicode escape",
            "missing object delimiter",
            "missing object colon",
            "trailing array delimiter",
            "trailing object delimiter",
            "trailing data",
        ] {
            assert_error(
                corpus_value(&corpus, name),
                EnvelopeErrorCode::MalformedJson,
            );
        }
        for name in ["NaN", "Infinity", "negative Infinity"] {
            assert_error(corpus_value(&corpus, name), EnvelopeErrorCode::WrongType);
        }

        for (name, input) in [
            ("raw Unicode", wrap("\"é\"".as_bytes())),
            ("negative zero", wrap(b"-0")),
            ("alternate escape", wrap(br#""\/""#)),
            ("reordered twin", b"{\"b\":2,\"a\":1}\n".to_vec()),
        ] {
            let error = scan_body_envelope(&input).expect_err("case should be rejected");
            assert_eq!(error.code(), EnvelopeErrorCode::NoncanonicalJson, "{name}");
        }
        assert_success(&wrap(br#""\ud800""#));
        assert_success(b"{\"a\":1,\"b\":2}\n");
    }

    #[test]
    fn pinned_envelope_error_contract() {
        for (name, input, code) in [
            (
                "BOM",
                b"\xef\xbb\xbf{}\n".as_slice(),
                EnvelopeErrorCode::MalformedJson,
            ),
            (
                "trailing non-JSON bytes",
                b"{}\n!".as_slice(),
                EnvelopeErrorCode::MalformedJson,
            ),
            (
                "trailing JSON whitespace",
                b"{}\n ".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "extra LF",
                b"{}\n\n".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "CRLF",
                b"{}\r\n".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "missing LF",
                b"{}".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "alternate object order",
                b"{\"b\":2,\"a\":1}\n".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "alternate spacing",
                b"{\"a\": 1}\n".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "raw Unicode",
                b"{\"a\":\"\xc3\xa9\"}\n".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "escape hex case",
                b"{\"a\":\"\\u00E9\"}\n".as_slice(),
                EnvelopeErrorCode::NoncanonicalJson,
            ),
            (
                "empty input",
                b"".as_slice(),
                EnvelopeErrorCode::MalformedJson,
            ),
            (
                "LF-only input",
                b"\n".as_slice(),
                EnvelopeErrorCode::MalformedJson,
            ),
            (
                "truncated object",
                b"{".as_slice(),
                EnvelopeErrorCode::MalformedJson,
            ),
            (
                "invalid UTF-8",
                b"{\"a\":\"\xff\"}".as_slice(),
                EnvelopeErrorCode::MalformedJson,
            ),
            (
                "truncated UTF-8",
                b"{\"a\":\"\xc3".as_slice(),
                EnvelopeErrorCode::MalformedJson,
            ),
        ] {
            assert_error(input, code);
            assert!(!name.is_empty());
        }

        for input in [
            b"null".as_slice(),
            b"null\n",
            b"true",
            b"true\n",
            b"false",
            b"false\n",
            b"1",
            b"1\n",
            b"\"\"",
            b"\"\"\n",
            b"[]",
            b"[]\n",
        ] {
            assert_error(input, EnvelopeErrorCode::WrongType);
        }
    }

    #[test]
    fn duplicate_keys_are_noncanonical_at_every_position() {
        for (input, expected) in [
            (
                br#"{"a":1,"a":1}
"#
                .as_slice(),
                r#"{"a":1}"#,
            ),
            (
                br#"{"a":1,"\u0061":2}
"#,
                r#"{"a":2}"#,
            ),
            (
                br#"{"a":1,"a":2}
"#,
                r#"{"a":2}"#,
            ),
            (
                br#"{"shards":[{"path":"first","path":"second"}]}
"#,
                r#"{"shards":[{"path":"second"}]}"#,
            ),
            (
                br#"{"ledger":{"path":"first","path":"second"}}
"#,
                r#"{"ledger":{"path":"second"}}"#,
            ),
            (
                br#"{"summary_plan":{"days":[],"days":[]}}
"#,
                r#"{"summary_plan":{"days":[]}}"#,
            ),
            (
                br#"{"unknown":{"x":1,"x":2}}
"#,
                r#"{"unknown":{"x":2}}"#,
            ),
            (
                br#"{"items":[{"x":1,"x":2}]}
"#,
                r#"{"items":[{"x":2}]}"#,
            ),
        ] {
            let candidate = input.strip_suffix(b"\n").expect("test input has LF");
            let parsed = parse(candidate).expect("duplicate JSON parses");
            assert_eq!(
                canonicalize(&parsed).expect("parsed duplicate canonicalizes"),
                expected
            );
            assert_error(input, EnvelopeErrorCode::NoncanonicalJson);
        }

        for input in [
            br#"{"a":1,"b":2}
"#
            .as_slice(),
            br#"{"shards":[{"path":"second"}]}
"#,
            br#"{"ledger":{"path":"second"}}
"#,
            br#"{"summary_plan":{"days":[]}}
"#,
        ] {
            assert_success(input);
        }
    }

    #[test]
    fn strings_with_key_looking_text_are_not_duplicate_keys() {
        assert_success(b"{\"a\":\"{\\\"a\\\":1,\\\"a\\\":2}\"}\n");
    }

    #[test]
    fn boundary_objects_preserve_the_terminal_framing_boundary() {
        let mut below_cap = canonical_string_object(MAX_ENVELOPE_BYTES - 1);
        below_cap.push(b'\n');
        assert_eq!(below_cap.len(), MAX_ENVELOPE_BYTES);
        assert_success(&below_cap);

        let at_cap_without_lf = canonical_string_object(MAX_ENVELOPE_BYTES);
        assert_eq!(at_cap_without_lf.len(), MAX_ENVELOPE_BYTES);
        assert_error(&at_cap_without_lf, EnvelopeErrorCode::NoncanonicalJson);
    }

    #[test]
    fn raw_unicode_is_noncanonical_without_a_growing_canonical_buffer() {
        let mut input = Vec::with_capacity(16_384);
        input.extend_from_slice(b"{\"k\":\"");
        for _ in 0..8_000 {
            input.extend_from_slice("é".as_bytes());
        }
        input.extend_from_slice(b"\"}\n");
        assert!(input.len() < MAX_ENVELOPE_BYTES);
        let info = allocation_counter::measure(|| {
            assert_error(&input, EnvelopeErrorCode::NoncanonicalJson);
        });
        assert!(
            info.bytes_max <= 524_288,
            "raw-Unicode scan peak was {} bytes",
            info.bytes_max
        );
    }

    #[test]
    fn oversized_inputs_fail_before_parsing_or_allocation() {
        let at_limit_plus_one = vec![0xff; MAX_ENVELOPE_BYTES + 1];
        let substantially_larger = vec![0xff; MAX_ENVELOPE_BYTES * 3];
        for input in [&at_limit_plus_one, &substantially_larger] {
            let info = allocation_counter::measure(|| {
                assert_error(input, EnvelopeErrorCode::InputTooLarge);
            });
            assert!(
                info.bytes_max <= 4_096,
                "oversized input peak was {} bytes",
                info.bytes_max
            );
        }
    }

    #[test]
    fn selected_proper_prefixes_are_panic_free() {
        let corpus = generated_corpus();
        for name in [
            "nested arrays and objects",
            "member order",
            "escaped key characters",
            "astral and lone surrogate keys",
            "4300-digit integer",
        ] {
            let input = corpus_value(&corpus, name);
            for length in 1..input.len() {
                assert!(
                    catch_unwind(AssertUnwindSafe(|| scan_body_envelope(&input[..length]))).is_ok(),
                    "{name} prefix {length} panicked"
                );
            }
        }
    }

    #[test]
    fn adversarial_byte_inputs_are_panic_free() {
        let (at_depth, _) = repeated_objects(128);
        let (over_depth, _) = alternating_containers(129);
        let byte_range = (0_u8..=u8::MAX).collect::<Vec<_>>();
        for input in [
            b"".as_slice(),
            b"\n",
            b"\xef\xbb\xbf{}\n",
            b"\xff",
            b"{\"a\":\"\xc3",
            b"{",
            b"[",
            at_depth.as_slice(),
            over_depth.as_slice(),
            byte_range.as_slice(),
        ] {
            assert!(
                catch_unwind(AssertUnwindSafe(|| scan_body_envelope(input))).is_ok(),
                "input {input:?} panicked"
            );
        }
    }

    fn assert_success_and_round_trip(input: &[u8]) {
        let scanned = scan_body_envelope(input).expect("canonical envelope scans");
        let candidate = input.strip_suffix(b"\n").expect("canonical JSONL has LF");
        let BodyValue::Object(expected) = parse(candidate).expect("fixture parses") else {
            panic!("fixture envelope is an object");
        };
        assert_body_value_bitwise_eq(
            &BodyValue::Object(scanned.object().clone()),
            &BodyValue::Object(expected),
        );
        let canonical = canonicalize(&BodyValue::Object(scanned.object().clone()))
            .expect("object canonicalizes");
        let mut round_trip = canonical.into_bytes();
        round_trip.push(b'\n');
        assert_eq!(round_trip, input);
    }

    fn assert_success(input: &[u8]) {
        scan_body_envelope(input).unwrap_or_else(|error| {
            panic!("input should scan: {error:?}; input={input:?}");
        });
    }

    fn assert_error(input: &[u8], expected_code: EnvelopeErrorCode) {
        let error = scan_body_envelope(input).expect_err("input should fail");
        assert_eq!(error.code(), expected_code);
        assert_eq!(error.field(), EnvelopeErrorField::Envelope);
        assert!(error.bundle().is_none());
        assert!(error.index().is_none());
    }

    fn corpus_value<'a>(corpus: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
        corpus
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, input)| input.as_slice())
            .unwrap_or_else(|| panic!("missing generated corpus input {name}"))
    }

    fn wrap(value: &[u8]) -> Vec<u8> {
        let mut input = Vec::with_capacity(value.len() + 7);
        input.extend_from_slice(b"{\"k\":");
        input.extend_from_slice(value);
        input.extend_from_slice(b"}\n");
        input
    }

    fn canonical_string_object(target_object_len: usize) -> Vec<u8> {
        assert!(target_object_len >= 8);
        let mut object = Vec::with_capacity(target_object_len);
        object.extend_from_slice(b"{\"k\":\"");
        object.resize(6 + target_object_len - 8, b'x');
        object.extend_from_slice(b"\"}");
        assert_eq!(object.len(), target_object_len);
        object
    }
}
