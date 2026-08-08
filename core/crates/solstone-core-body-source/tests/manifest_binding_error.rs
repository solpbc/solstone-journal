// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::error::Error;

use solstone_core_body_source::{
    BundleId, ManifestBindingError, ManifestBindingErrorCode, ManifestBindingErrorField,
};

mod support;

use support::{MAX_BUNDLE, MIN_BUNDLE};

fn bundles() -> [BundleId; 2] {
    assert_eq!(MIN_BUNDLE.len(), 31);
    assert_eq!(MAX_BUNDLE.len(), 31);
    [
        BundleId::from_bytes(MIN_BUNDLE.as_bytes()).expect("minimum bundle ID is valid"),
        BundleId::from_bytes(MAX_BUNDLE.as_bytes()).expect("maximum bundle ID is valid"),
    ]
}

fn code_spelling(code: ManifestBindingErrorCode) -> &'static str {
    match code {
        ManifestBindingErrorCode::InputTooLarge => "input_too_large",
        ManifestBindingErrorCode::MalformedManifest => "malformed_manifest",
        ManifestBindingErrorCode::DuplicateField => "duplicate_field",
        ManifestBindingErrorCode::UnknownField => "unknown_field",
        ManifestBindingErrorCode::MissingField => "missing_field",
        ManifestBindingErrorCode::WrongType => "wrong_type",
        ManifestBindingErrorCode::InvalidField => "invalid_field",
        ManifestBindingErrorCode::IncompatibleField => "incompatible_field",
    }
}

fn field_spelling(field: ManifestBindingErrorField) -> &'static str {
    match field {
        ManifestBindingErrorField::Manifest => "manifest",
        ManifestBindingErrorField::BodySourceSchema => "body_source_schema",
        ManifestBindingErrorField::BodyBundleRef => "body_bundle_ref",
        ManifestBindingErrorField::BodyBundleSha256 => "body_bundle_sha256",
        ManifestBindingErrorField::ImportId => "import_id",
        ManifestBindingErrorField::SourceType => "source_type",
        ManifestBindingErrorField::SourceHash => "source_hash",
        ManifestBindingErrorField::EntryCount => "entry_count",
        ManifestBindingErrorField::DaysAffected => "days_affected",
        ManifestBindingErrorField::RawRetention => "raw_retention",
    }
}

#[test]
fn manifest_binding_error_vocabulary_is_canonical_exhaustive_copyable_and_hashable() {
    let expected_codes = [
        (ManifestBindingErrorCode::InputTooLarge, "input_too_large"),
        (
            ManifestBindingErrorCode::MalformedManifest,
            "malformed_manifest",
        ),
        (ManifestBindingErrorCode::DuplicateField, "duplicate_field"),
        (ManifestBindingErrorCode::UnknownField, "unknown_field"),
        (ManifestBindingErrorCode::MissingField, "missing_field"),
        (ManifestBindingErrorCode::WrongType, "wrong_type"),
        (ManifestBindingErrorCode::InvalidField, "invalid_field"),
        (
            ManifestBindingErrorCode::IncompatibleField,
            "incompatible_field",
        ),
    ];
    let expected_fields = [
        (ManifestBindingErrorField::Manifest, "manifest"),
        (
            ManifestBindingErrorField::BodySourceSchema,
            "body_source_schema",
        ),
        (ManifestBindingErrorField::BodyBundleRef, "body_bundle_ref"),
        (
            ManifestBindingErrorField::BodyBundleSha256,
            "body_bundle_sha256",
        ),
        (ManifestBindingErrorField::ImportId, "import_id"),
        (ManifestBindingErrorField::SourceType, "source_type"),
        (ManifestBindingErrorField::SourceHash, "source_hash"),
        (ManifestBindingErrorField::EntryCount, "entry_count"),
        (ManifestBindingErrorField::DaysAffected, "days_affected"),
        (ManifestBindingErrorField::RawRetention, "raw_retention"),
    ];

    assert_eq!(ManifestBindingErrorCode::ALL.len(), 8);
    assert_eq!(ManifestBindingErrorField::ALL.len(), 10);
    for ((actual, expected_variant), expected_spelling) in ManifestBindingErrorCode::ALL
        .iter()
        .zip(expected_codes.iter())
        .zip(expected_codes.iter().map(|(_, spelling)| spelling))
    {
        assert_eq!(actual, &expected_variant.0);
        assert_eq!(actual.as_str(), *expected_spelling);
        assert_eq!(code_spelling(*actual), *expected_spelling);
    }
    for ((actual, expected_variant), expected_spelling) in ManifestBindingErrorField::ALL
        .iter()
        .zip(expected_fields.iter())
        .zip(expected_fields.iter().map(|(_, spelling)| spelling))
    {
        assert_eq!(actual, &expected_variant.0);
        assert_eq!(actual.as_str(), *expected_spelling);
        assert_eq!(field_spelling(*actual), *expected_spelling);
    }

    let mut codes = ManifestBindingErrorCode::ALL.to_vec();
    codes.reverse();
    codes.sort();
    assert_eq!(codes, ManifestBindingErrorCode::ALL.to_vec());
    let mut fields = ManifestBindingErrorField::ALL.to_vec();
    fields.reverse();
    fields.sort();
    assert_eq!(fields, ManifestBindingErrorField::ALL.to_vec());

    let codes: HashSet<_> = ManifestBindingErrorCode::ALL.into_iter().collect();
    assert_eq!(codes.len(), 8);
    let fields: HashSet<_> = ManifestBindingErrorField::ALL.into_iter().collect();
    assert_eq!(fields.len(), 10);

    let code = ManifestBindingErrorCode::ALL[0];
    let copied_code = code;
    assert_eq!(code, copied_code);
    let field = ManifestBindingErrorField::ALL[0];
    let copied_field = field;
    assert_eq!(field, copied_field);
}

#[test]
fn manifest_binding_error_constructs_and_clones_every_combination() {
    for bundle in bundles() {
        for code in ManifestBindingErrorCode::ALL {
            for field in ManifestBindingErrorField::ALL {
                let error = ManifestBindingError::new(bundle.clone(), code, field);
                assert_eq!(error.bundle().as_str(), bundle.as_str());
                assert_eq!(error.code(), code);
                assert_eq!(error.field(), field);
                assert_eq!(error.clone(), error);
            }
        }
    }
}

#[test]
fn manifest_binding_error_renders_bounded_source_free_output() {
    for bundle in bundles() {
        for code in ManifestBindingErrorCode::ALL {
            for field in ManifestBindingErrorField::ALL {
                let error = ManifestBindingError::new(bundle.clone(), code, field);
                let display = error.to_string();
                assert_eq!(display, format!("{error:?}"));
                assert!(display.len() <= 160);
                assert!(Error::source(&error).is_none());
                assert!(display.starts_with("body-manifest["));
                assert!(display.contains(bundle.as_str()));
                assert!(display.contains(&format!("] {}: {}", code.as_str(), field.as_str())));
            }
        }
    }

    for (error, expected) in [
        (
            ManifestBindingError::new(
                BundleId::from_bytes(MIN_BUNDLE.as_bytes()).unwrap(),
                ManifestBindingErrorCode::InputTooLarge,
                ManifestBindingErrorField::Manifest,
            ),
            "body-manifest[body-00000000000000000000000000] input_too_large: manifest",
        ),
        (
            ManifestBindingError::new(
                BundleId::from_bytes(MIN_BUNDLE.as_bytes()).unwrap(),
                ManifestBindingErrorCode::DuplicateField,
                ManifestBindingErrorField::BodyBundleSha256,
            ),
            "body-manifest[body-00000000000000000000000000] duplicate_field: body_bundle_sha256",
        ),
        (
            ManifestBindingError::new(
                BundleId::from_bytes(MAX_BUNDLE.as_bytes()).unwrap(),
                ManifestBindingErrorCode::WrongType,
                ManifestBindingErrorField::EntryCount,
            ),
            "body-manifest[body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ] wrong_type: entry_count",
        ),
        (
            ManifestBindingError::new(
                BundleId::from_bytes(MAX_BUNDLE.as_bytes()).unwrap(),
                ManifestBindingErrorCode::InvalidField,
                ManifestBindingErrorField::SourceHash,
            ),
            "body-manifest[body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ] invalid_field: source_hash",
        ),
        (
            ManifestBindingError::new(
                BundleId::from_bytes(MAX_BUNDLE.as_bytes()).unwrap(),
                ManifestBindingErrorCode::IncompatibleField,
                ManifestBindingErrorField::RawRetention,
            ),
            "body-manifest[body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ] incompatible_field: raw_retention",
        ),
    ] {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn manifest_binding_error_resists_vocabulary_and_equality_drift() {
    let bundles = bundles();
    let min = &bundles[0];
    let max = &bundles[1];
    let code = ManifestBindingErrorCode::MissingField;
    let field = ManifestBindingErrorField::ImportId;
    assert_ne!(
        ManifestBindingError::new(min.clone(), code, field),
        ManifestBindingError::new(max.clone(), code, field)
    );
    assert_ne!(
        ManifestBindingError::new(min.clone(), code, field),
        ManifestBindingError::new(min.clone(), ManifestBindingErrorCode::WrongType, field)
    );
    assert_ne!(
        ManifestBindingError::new(min.clone(), code, field),
        ManifestBindingError::new(min.clone(), code, ManifestBindingErrorField::SourceType)
    );

    let mut rendered = Vec::new();
    for bundle in &bundles {
        for code in ManifestBindingErrorCode::ALL {
            for field in ManifestBindingErrorField::ALL {
                rendered.push(ManifestBindingError::new(bundle.clone(), code, field).to_string());
            }
        }
    }
    assert!(rendered.iter().all(|value| !value.contains("<invalid>")));
    for code in ManifestBindingErrorCode::ALL {
        assert!(rendered.iter().any(|value| value.contains(code.as_str())));
    }
    for field in ManifestBindingErrorField::ALL {
        assert!(rendered.iter().any(|value| value.contains(field.as_str())));
    }

    assert_eq!(
        ManifestBindingErrorCode::ALL
            .iter()
            .map(|code| code.as_str())
            .collect::<Vec<_>>(),
        vec![
            "input_too_large",
            "malformed_manifest",
            "duplicate_field",
            "unknown_field",
            "missing_field",
            "wrong_type",
            "invalid_field",
            "incompatible_field",
        ]
    );
    assert_eq!(
        ManifestBindingErrorField::ALL
            .iter()
            .map(|field| field.as_str())
            .collect::<Vec<_>>(),
        vec![
            "manifest",
            "body_source_schema",
            "body_bundle_ref",
            "body_bundle_sha256",
            "import_id",
            "source_type",
            "source_hash",
            "entry_count",
            "days_affected",
            "raw_retention",
        ]
    );
}
