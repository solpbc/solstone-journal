// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    AuthorityError, BundleId, DirectoryObservation, NativeAuthority, authorize_native_bundle,
    decode_body_manifest,
};

const VALID_BUNDLE: &[u8] = b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y";

fn assert_authority_error(
    result: Result<NativeAuthority, AuthorityError>,
    expected: AuthorityError,
) {
    let Err(actual) = result else {
        panic!("authority should refuse");
    };
    assert_eq!(actual, expected);
}

#[test]
fn authorization_rejects_non_native_candidate_before_directory_validation() {
    for observation in [
        DirectoryObservation {
            name: b"legacy-directory",
            envelope_present: false,
            ledger_present: false,
            manifest: None,
        },
        DirectoryObservation {
            name: b".body-staging-partial",
            envelope_present: true,
            ledger_present: true,
            manifest: Some(b"{"),
        },
    ] {
        assert_authority_error(
            authorize_native_bundle(observation),
            AuthorityError::NotNativeCandidate,
        );
    }
}

#[test]
fn authorization_rejects_invalid_native_candidate_directory_before_sidecars() {
    for observation in [
        DirectoryObservation {
            name: b"body-not-a-bundle",
            envelope_present: false,
            ledger_present: false,
            manifest: None,
        },
        DirectoryObservation {
            name: b"legacy-\xff",
            envelope_present: true,
            ledger_present: true,
            manifest: Some(b"{"),
        },
    ] {
        assert_authority_error(
            authorize_native_bundle(observation),
            AuthorityError::InvalidDirectory,
        );
    }
}

#[test]
fn authorization_requires_envelope_before_all_later_proofs() {
    assert_authority_error(
        authorize_native_bundle(DirectoryObservation {
            name: VALID_BUNDLE,
            envelope_present: false,
            ledger_present: false,
            manifest: None,
        }),
        AuthorityError::MissingEnvelope,
    );
}

#[test]
fn authorization_requires_ledger_after_envelope() {
    assert_authority_error(
        authorize_native_bundle(DirectoryObservation {
            name: VALID_BUNDLE,
            envelope_present: true,
            ledger_present: false,
            manifest: None,
        }),
        AuthorityError::MissingLedger,
    );
}

#[test]
fn authorization_requires_manifest_after_sidecars() {
    assert_authority_error(
        authorize_native_bundle(DirectoryObservation {
            name: VALID_BUNDLE,
            envelope_present: true,
            ledger_present: true,
            manifest: None,
        }),
        AuthorityError::MissingManifest,
    );
}

#[test]
fn authorization_wraps_exact_manifest_decoder_errors_after_outer_proofs() {
    let bundle = BundleId::from_bytes(VALID_BUNDLE).expect("fixture bundle is valid");
    for bytes in [b"{".as_slice(), br#"{"ordinary":true}"#] {
        let Err(expected) = decode_body_manifest(bytes, &bundle) else {
            panic!("manifest should refuse");
        };
        let Err(actual) = authorize_native_bundle(DirectoryObservation {
            name: VALID_BUNDLE,
            envelope_present: true,
            ledger_present: true,
            manifest: Some(bytes),
        }) else {
            panic!("authority should refuse");
        };
        let AuthorityError::InvalidManifest(actual) = actual else {
            panic!("authority should wrap the decoder error");
        };
        assert_eq!(actual.code(), expected.code());
        assert_eq!(actual.field(), expected.field());
        assert_eq!(actual.bundle(), expected.bundle());
    }
}
