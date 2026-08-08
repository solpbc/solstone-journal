// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    ManifestKeySignal, inspect_body_manifest_signal, scan_body_manifest,
};

const MAX_MANIFEST_BYTES: usize = 1_048_576;

#[test]
fn manifest_signals_follow_readable_scanner_facts() {
    assert_signal(br#"{"body":null}"#, ManifestKeySignal::NoBodyKey);
    assert_signal(
        br#"{"body_":null}"#,
        ManifestKeySignal::BodyKeyPresent {
            unknown_body_key: true,
        },
    );
    assert_signal(
        br#"{"body_x":null}"#,
        ManifestKeySignal::BodyKeyPresent {
            unknown_body_key: true,
        },
    );

    for key in [
        "body_source_schema",
        "body_bundle_ref",
        "body_bundle_sha256",
    ] {
        assert_signal(
            format!(r#"{{"{key}":null}}"#).as_bytes(),
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: false,
            },
        );
    }
    for key in [
        "import_id",
        "source_type",
        "source_hash",
        "entry_count",
        "days_affected",
        "raw_retention",
    ] {
        assert_signal(
            format!(r#"{{"{key}":null}}"#).as_bytes(),
            ManifestKeySignal::NoBodyKey,
        );
    }
    assert_signal(
        br#"{"\u0062ody_source_schema":null}"#,
        ManifestKeySignal::BodyKeyPresent {
            unknown_body_key: false,
        },
    );

    for (input, expected) in [
        (
            "{\"body_λ\":null}".as_bytes(),
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: true,
            },
        ),
        (
            "{\"body_🫀\":null}".as_bytes(),
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: true,
            },
        ),
        (
            br#"{"body_\ud800":null}"#,
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: true,
            },
        ),
        (
            br#"{"body_x":1,"body_x":2}"#,
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: true,
            },
        ),
        (
            br#"{"body_source_schema":null}"#,
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: false,
            },
        ),
        (
            br#"{"body_source_schema":1}"#,
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: false,
            },
        ),
    ] {
        assert_signal(input, expected);
    }

    assert_signal(
        br#"{"nested":{"body_x":null}}"#,
        ManifestKeySignal::NoBodyKey,
    );
    assert_eq!(
        inspect_body_manifest_signal(Some(b"\"body_x\"")),
        ManifestKeySignal::Unreadable
    );
}

#[test]
fn manifest_signal_marks_unreadable_inputs() {
    assert_eq!(
        inspect_body_manifest_signal(None),
        ManifestKeySignal::Unreadable
    );

    let mut exact_limit = vec![b' '; MAX_MANIFEST_BYTES - 2];
    exact_limit.extend_from_slice(b"{}");
    assert_eq!(
        inspect_body_manifest_signal(Some(&exact_limit)),
        ManifestKeySignal::NoBodyKey
    );

    exact_limit.push(b' ');
    for input in [exact_limit.as_slice(), b"{".as_slice(), b"null".as_slice()] {
        assert_eq!(
            inspect_body_manifest_signal(Some(input)),
            ManifestKeySignal::Unreadable
        );
    }
}

fn assert_signal(input: &[u8], expected: ManifestKeySignal) {
    let scanned = scan_body_manifest(input).expect("signal matrix input scans");
    let expected_facts = match expected {
        ManifestKeySignal::BodyKeyPresent { unknown_body_key } => (true, unknown_body_key),
        ManifestKeySignal::NoBodyKey => (false, false),
        ManifestKeySignal::Unreadable => panic!("matrix input must be readable"),
    };
    assert_eq!(
        (
            scanned.has_body_prefixed_key(),
            scanned.has_unknown_body_prefixed_key(),
        ),
        expected_facts
    );
    assert_eq!(inspect_body_manifest_signal(Some(input)), expected);
}
