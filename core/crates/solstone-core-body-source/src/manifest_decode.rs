// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::manifest_projection::{manifest_binding_error_field, project_manifest_binding};
use crate::{
    BodyManifestBinding, BundleId, ManifestBindingError, ManifestBindingErrorCode,
    ManifestBindingErrorField, ManifestScanError, scan_body_manifest,
};

/// Decodes a raw body manifest into checked values for an expected bundle.
pub fn decode_body_manifest(
    input: &[u8],
    expected_bundle_id: &BundleId,
) -> Result<BodyManifestBinding, ManifestBindingError> {
    let scanned = match scan_body_manifest(input) {
        Ok(scanned) => scanned,
        Err(ManifestScanError::InputTooLarge) => {
            return Err(ManifestBindingError::new(
                expected_bundle_id.clone(),
                ManifestBindingErrorCode::InputTooLarge,
                ManifestBindingErrorField::Manifest,
            ));
        }
        Err(ManifestScanError::MalformedManifest) => {
            return Err(ManifestBindingError::new(
                expected_bundle_id.clone(),
                ManifestBindingErrorCode::MalformedManifest,
                ManifestBindingErrorField::Manifest,
            ));
        }
    };

    if let Some(&key) = scanned.duplicated_known_keys().first() {
        return Err(ManifestBindingError::new(
            expected_bundle_id.clone(),
            ManifestBindingErrorCode::DuplicateField,
            manifest_binding_error_field(key),
        ));
    }
    if scanned.has_unknown_body_prefixed_key() {
        return Err(ManifestBindingError::new(
            expected_bundle_id.clone(),
            ManifestBindingErrorCode::UnknownField,
            ManifestBindingErrorField::Manifest,
        ));
    }

    project_manifest_binding(scanned.object(), expected_bundle_id)
}
