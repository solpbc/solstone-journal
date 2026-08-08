// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    AuthorityError, BundleClass, DirectoryObservation, ManifestKeySignal, authorize_native_bundle,
    classify_bundle_directory, inspect_body_manifest_signal,
};

const MAX_MANIFEST_BYTES: usize = 1_048_576;
const VALID_BUNDLE: &[u8] = b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y";

#[test]
fn classifies_directory_observation_cartesian_truth_table() {
    let names = [
        ("valid reserved", VALID_BUNDLE),
        ("invalid reserved", b"body-not-a-bundle".as_slice()),
        ("staging", b".body-staging-partial".as_slice()),
        ("nonreserved", b"legacy-directory".as_slice()),
        ("reserved invalid UTF-8", b"body-\xff".as_slice()),
        ("nonreserved invalid UTF-8", b"legacy-\xff".as_slice()),
    ];
    let manifests = [
        ("absent", None, false),
        (
            "body key present",
            Some(br#"{"body_source_schema":null}"#.as_slice()),
            true,
        ),
        (
            "no body key",
            Some(br#"{"import_id":null}"#.as_slice()),
            false,
        ),
        ("unreadable", Some(b"{".as_slice()), true),
    ];

    for (name_label, name) in names {
        for envelope_present in [false, true] {
            for ledger_present in [false, true] {
                for (manifest_label, manifest, manifest_signals_native) in manifests {
                    let expected = if name.starts_with(b".body-staging-") {
                        BundleClass::StagingExcluded
                    } else if name.starts_with(b"body-")
                        || envelope_present
                        || ledger_present
                        || manifest_signals_native
                    {
                        BundleClass::NativeCandidate
                    } else {
                        BundleClass::LegacyCandidate
                    };
                    let actual = classify_bundle_directory(DirectoryObservation {
                        name,
                        envelope_present,
                        ledger_present,
                        manifest,
                    });
                    assert_eq!(
                        actual, expected,
                        "{name_label}; envelope={envelope_present}; ledger={ledger_present}; {manifest_label}"
                    );
                }
            }
        }
    }
}

#[test]
fn classifier_matches_public_manifest_signal_for_existing_scan_edge_inputs() {
    let mut exact_limit = vec![b' '; MAX_MANIFEST_BYTES - 2];
    exact_limit.extend_from_slice(b"{}");
    let mut over_limit = exact_limit.clone();
    over_limit.push(b' ');

    for (name, bytes) in [
        ("literal known", br#"{"body_source_schema":null}"#.as_slice()),
        (
            "escaped known",
            br#"{"\u0062ody_source_schema":null}"#,
        ),
        ("literal unknown", br#"{"body_x":null}"#),
        ("escaped unknown", br#"{"body_\ud800":null}"#),
        (
            "duplicate known",
            br#"{"raw_retention":1,"raw_retention":2,"body_source_schema":1,"body_source_schema":2}"#,
        ),
        ("duplicate unknown", br#"{"body_x":1,"body_x":2}"#),
        ("known null", br#"{"body_bundle_ref":null}"#),
        ("known wrong type", br#"{"body_bundle_ref":1}"#),
        ("nested body key", br#"{"nested":{"body_x":null}}"#),
        ("key-looking string", br#"{"ordinary":"body_x"}"#),
        ("exact limit", exact_limit.as_slice()),
        ("over limit", over_limit.as_slice()),
        ("malformed", b"{".as_slice()),
        ("nonobject", b"null".as_slice()),
    ] {
        let signal = inspect_body_manifest_signal(Some(bytes));
        let expected = match signal {
            ManifestKeySignal::BodyKeyPresent { .. } | ManifestKeySignal::Unreadable => {
                BundleClass::NativeCandidate
            }
            ManifestKeySignal::NoBodyKey => BundleClass::LegacyCandidate,
        };
        assert_eq!(
            classify_bundle_directory(DirectoryObservation {
                name: b"legacy-directory",
                envelope_present: false,
                ledger_present: false,
                manifest: Some(bytes),
            }),
            expected,
            "{name}"
        );
    }
}

#[test]
fn classifier_distinguishes_absent_manifest_from_present_unreadable_manifest() {
    let absent = DirectoryObservation {
        name: b"legacy-directory",
        envelope_present: false,
        ledger_present: false,
        manifest: None,
    };
    let unreadable = DirectoryObservation {
        manifest: Some(b"{"),
        ..absent
    };
    assert_eq!(
        classify_bundle_directory(absent),
        BundleClass::LegacyCandidate
    );
    assert_eq!(
        classify_bundle_directory(unreadable),
        BundleClass::NativeCandidate
    );
}

#[test]
fn classifier_treats_known_and_unknown_top_level_body_keys_as_native_equally() {
    for manifest in [
        br#"{"body_source_schema":null}"#.as_slice(),
        br#"{"body_x":null}"#,
    ] {
        assert_eq!(
            classify_bundle_directory(DirectoryObservation {
                name: b"legacy-directory",
                envelope_present: false,
                ledger_present: false,
                manifest: Some(manifest),
            }),
            BundleClass::NativeCandidate
        );
    }
}

#[test]
fn present_unreadable_manifest_on_complete_native_directory_reaches_manifest_validation() {
    let result = authorize_native_bundle(DirectoryObservation {
        name: VALID_BUNDLE,
        envelope_present: true,
        ledger_present: true,
        manifest: Some(b"{"),
    });
    assert!(matches!(result, Err(AuthorityError::InvalidManifest(_))));
}
