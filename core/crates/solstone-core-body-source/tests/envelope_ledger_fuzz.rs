// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDigest, BundleId, EnvelopeErrorCode, EnvelopeErrorField, EnvelopeLedger,
};

const MIN_BUNDLE: &str = "body-00000000000000000000000000";
const MAX_BUNDLE: &str = "body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
const BUNDLES: [&str; 2] = [MIN_BUNDLE, MAX_BUNDLE];
const COUNT_TUPLES: [(u64, u64); 10] = [
    (0, 0),
    (0, 1),
    (0, u64::MAX),
    (1, 0),
    (1, 1),
    (1, 2),
    (1, u64::MAX),
    (u64::MAX, 0),
    (u64::MAX, 1),
    (u64::MAX, u64::MAX),
];
const NONEMPTY_SHA256: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EMPTY_CONTENT_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn bundle(value: &str) -> BundleId {
    BundleId::from_bytes(value.as_bytes()).expect("boundary bundle is valid")
}

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("boundary digest is valid")
}

#[test]
fn envelope_ledger_crosses_boundary_counts_and_digest_states() {
    let mut combinations = 0;

    for bundle_text in BUNDLES {
        for (bytes, events) in COUNT_TUPLES {
            for sha256 in [EMPTY_CONTENT_SHA256, NONEMPTY_SHA256] {
                let bundle = bundle(bundle_text);
                let empty_digest = sha256 == EMPTY_CONTENT_SHA256;
                let expected_field = if (events == 0) != (bytes == 0) {
                    Some(EnvelopeErrorField::LedgerBytes)
                } else if events > bytes {
                    Some(EnvelopeErrorField::LedgerEvents)
                } else if empty_digest != (bytes == 0) {
                    Some(EnvelopeErrorField::LedgerSha256)
                } else {
                    None
                };
                let result = EnvelopeLedger::new(&bundle, bytes, events, digest(sha256));

                match expected_field {
                    Some(field) => {
                        let error = result.expect_err("invalid boundary must refuse");
                        assert_eq!(error.code(), EnvelopeErrorCode::IncompatibleField);
                        assert_eq!(error.field(), field);
                        assert_eq!(error.bundle(), Some(&bundle));
                        assert_eq!(error.index(), None);
                    }
                    None => {
                        let ledger = result.expect("valid boundary must bind");
                        assert_eq!(ledger.path(), "body-ledger.jsonl");
                        assert_eq!(ledger.bytes(), bytes);
                        assert_eq!(ledger.events(), events);
                        assert_eq!(ledger.sha256().as_str(), sha256);
                    }
                }
                combinations += 1;
            }
        }
    }

    assert_eq!(combinations, 40);
}
