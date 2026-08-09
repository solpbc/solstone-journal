// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::canonicalize::{CanonicalSink, CanonicalizeValueError, canonicalize_value};
use crate::ledger_event_encode::{MAX_LEDGER_EVENT_FRAME_BYTES, MAX_LEDGER_EVENT_OBJECT_BYTES};
use crate::{
    BodyObject, BodyValue, BundleId, LedgerEventError, LedgerEventErrorCode, LedgerEventErrorField,
};

/// A parsed body-ledger event whose bytes are exact canonical JSONL.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScannedBodyLedgerEvent {
    object: BodyObject,
}

impl ScannedBodyLedgerEvent {
    /// Returns the parsed top-level ledger-event object.
    pub(crate) fn object(&self) -> &BodyObject {
        &self.object
    }
}

/// Scans bounded raw bytes as an exact canonical body-ledger JSONL object.
pub(crate) fn scan_body_ledger_event(
    frame: &[u8],
    bundle_id: &BundleId,
    expected_sequence: u64,
) -> Result<ScannedBodyLedgerEvent, LedgerEventError> {
    if frame.len() > MAX_LEDGER_EVENT_FRAME_BYTES {
        return Err(ledger_error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InputTooLarge,
        ));
    }

    let candidate = frame.strip_suffix(b"\n").unwrap_or(frame);
    if candidate.len() > MAX_LEDGER_EVENT_OBJECT_BYTES {
        return Err(ledger_error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InputTooLarge,
        ));
    }
    let value = crate::parser::parse(candidate).map_err(|_| {
        ledger_error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::MalformedJson,
        )
    })?;
    if !matches!(value, BodyValue::Object(_)) {
        return Err(ledger_error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::WrongType,
        ));
    }

    let mut sink = ComparingSink::new(frame);
    let outcome = canonicalize_value(&value, 0, &mut sink).and_then(|()| {
        sink.write_bytes(b"\n")
            .map_err(CanonicalizeValueError::Sink)
    });
    match outcome {
        Ok(()) if sink.is_fully_consumed() => {
            let BodyValue::Object(object) = value else {
                unreachable!("checked above")
            };
            Ok(ScannedBodyLedgerEvent { object })
        }
        Ok(()) | Err(CanonicalizeValueError::Sink(_)) => Err(ledger_error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::NoncanonicalJson,
        )),
        Err(CanonicalizeValueError::Canonicalize(_)) => unreachable!(
            "value already parsed within MAX_NESTING, so re-canonicalizing cannot exceed it"
        ),
    }
}

fn ledger_error(
    bundle_id: &BundleId,
    expected_sequence: u64,
    code: LedgerEventErrorCode,
) -> LedgerEventError {
    LedgerEventError::new(
        Some(bundle_id.clone()),
        code,
        LedgerEventErrorField::Ledger,
        expected_sequence,
    )
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

    use crate::decode_body_envelope;
    use crate::parser::test_corpus::{fixture, generated_corpus};

    use super::*;

    fn envelope() -> crate::BodyEnvelope {
        let fixture = fixture("body_source_native_bundle_v1.json");
        let case = &fixture["cases"][0];
        decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("fixture envelope")
                .as_bytes(),
        )
        .expect("fixture envelope decodes")
    }

    #[test]
    fn pinned_ledger_frame_scan_contract() {
        let envelope = envelope();
        let fixture = fixture("body_source_native_bundle_v1.json");
        let frame = fixture["cases"][0]["expected_ledger_jsonl"]
            .as_str()
            .expect("fixture ledger");
        assert!(scan_body_ledger_event(frame.as_bytes(), envelope.bundle_id(), 1).is_ok());
        for (input, code) in [
            (b"{}".as_slice(), LedgerEventErrorCode::NoncanonicalJson),
            (b"{}\n\n", LedgerEventErrorCode::NoncanonicalJson),
            (b"{}\r\n", LedgerEventErrorCode::NoncanonicalJson),
            (b"\xef\xbb\xbf{}\n", LedgerEventErrorCode::MalformedJson),
            (b"{}\n!", LedgerEventErrorCode::MalformedJson),
        ] {
            let error =
                scan_body_ledger_event(input, envelope.bundle_id(), 1).expect_err("refuses");
            assert_eq!(error.code(), code);
            assert_eq!(error.field(), LedgerEventErrorField::Ledger);
            assert_eq!(error.bundle(), Some(envelope.bundle_id()));
            assert_eq!(error.line(), 1);
        }
        for input in [b"null\n".as_slice(), b"true\n", b"1\n", b"\"\"\n", b"[]\n"] {
            assert_scan_error(input, &envelope, LedgerEventErrorCode::WrongType);
        }
        for input in [
            b"{\"b\":2,\"a\":1}\n".as_slice(),
            b"{\"a\": 1,\"b\":2}\n",
            b"{\"a\":\"\xc3\xa9\"}\n",
            b"{\"a\":-0}\n",
        ] {
            assert_scan_error(input, &envelope, LedgerEventErrorCode::NoncanonicalJson);
        }
        for input in [b"{\"a\":0}\n".as_slice(), b"{\"a\":\"\\u00e9\"}\n"] {
            assert!(scan_body_ledger_event(input, envelope.bundle_id(), 1).is_ok());
        }
        for input in [
            b"{\"a\":1,\"a\":2,\"z\":3}\n".as_slice(),
            b"{\"a\":1,\"\\u0061\":2,\"z\":3}\n",
            b"{\"a\":1,\"m\":2,\"m\":3,\"z\":4}\n",
            b"{\"a\":1,\"m\":2,\"\\u006d\":3,\"z\":4}\n",
            b"{\"a\":1,\"m\":2,\"z\":3,\"z\":4}\n",
            b"{\"a\":1,\"m\":2,\"z\":3,\"\\u007a\":4}\n",
        ] {
            assert_scan_error(input, &envelope, LedgerEventErrorCode::NoncanonicalJson);
        }
    }

    #[test]
    fn generated_prefixes_and_adversarial_bytes_are_panic_free() {
        let envelope = envelope();
        let corpus = generated_corpus();
        for name in [
            "nested arrays and objects",
            "member order",
            "escaped key characters",
            "astral and lone surrogate keys",
            "4300-digit integer",
        ] {
            let input = corpus
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, input)| input)
                .expect("generated input");
            for length in 1..input.len() {
                assert!(
                    catch_unwind(AssertUnwindSafe(|| scan_body_ledger_event(
                        &input[..length],
                        envelope.bundle_id(),
                        1,
                    )))
                    .is_ok(),
                    "{name} prefix {length} panicked"
                );
            }
        }
        for (name, frame) in fixture_ledger_frames() {
            for length in 1..frame.len() {
                assert!(
                    catch_unwind(AssertUnwindSafe(|| scan_body_ledger_event(
                        &frame[..length],
                        envelope.bundle_id(),
                        1,
                    )))
                    .is_ok(),
                    "{name} prefix {length} panicked"
                );
            }
        }
        for byte in 0_u8..=u8::MAX {
            assert!(
                catch_unwind(AssertUnwindSafe(|| scan_body_ledger_event(
                    &[byte],
                    envelope.bundle_id(),
                    1,
                )))
                .is_ok(),
                "one-byte input {byte:#04x} panicked"
            );
        }
        for pair in 0_u16..=u16::MAX {
            let input = [(pair >> 8) as u8, pair as u8];
            assert!(
                catch_unwind(AssertUnwindSafe(|| scan_body_ledger_event(
                    &input,
                    envelope.bundle_id(),
                    1,
                )))
                .is_ok(),
                "two-byte input {pair:#06x} panicked"
            );
        }
        let byte_range = (0_u8..=u8::MAX).collect::<Vec<_>>();
        for input in [
            b"".as_slice(),
            b"\n",
            b"\xff",
            b"{\"a\":\"\xc3",
            b"{",
            b"[",
            byte_range.as_slice(),
        ] {
            assert!(
                catch_unwind(AssertUnwindSafe(|| scan_body_ledger_event(
                    input,
                    envelope.bundle_id(),
                    1,
                )))
                .is_ok(),
                "input {input:?} panicked"
            );
        }
    }

    #[test]
    fn frame_and_object_caps_are_distinct() {
        let envelope = envelope();
        for input in [
            vec![b'x'; MAX_LEDGER_EVENT_FRAME_BYTES + 1],
            vec![b'x'; MAX_LEDGER_EVENT_OBJECT_BYTES + 1],
        ] {
            let error =
                scan_body_ledger_event(&input, envelope.bundle_id(), 1).expect_err("refuses");
            assert_eq!(error.code(), LedgerEventErrorCode::InputTooLarge);
            assert_eq!(error.field(), LedgerEventErrorField::Ledger);
        }
    }

    fn assert_scan_error(input: &[u8], envelope: &crate::BodyEnvelope, code: LedgerEventErrorCode) {
        let error = scan_body_ledger_event(input, envelope.bundle_id(), 1).expect_err("refuses");
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), LedgerEventErrorField::Ledger);
        assert_eq!(error.bundle(), Some(envelope.bundle_id()));
        assert_eq!(error.line(), 1);
    }

    fn fixture_ledger_frames() -> Vec<(String, Vec<u8>)> {
        let native = fixture("body_source_native_bundle_v1.json");
        let mut frames = native["cases"]
            .as_array()
            .expect("native cases")
            .iter()
            .filter_map(|case| {
                let frame = case["expected_ledger_jsonl"]
                    .as_str()
                    .expect("ledger frame");
                (!frame.is_empty()).then(|| {
                    (
                        case["name"].as_str().expect("case name").to_owned(),
                        frame.as_bytes().to_vec(),
                    )
                })
            })
            .collect::<Vec<_>>();
        let ledger = fixture("body_source_ledger_events_v1.json");
        frames.extend(
            ledger["cases"][0]["expected_ledger_jsonl"]
                .as_str()
                .expect("multishard frames")
                .lines()
                .enumerate()
                .map(|(index, frame)| {
                    (
                        format!("multishard line {}", index + 1),
                        format!("{frame}\n").into_bytes(),
                    )
                }),
        );
        assert_eq!(frames.len(), 5);
        frames
    }
}
