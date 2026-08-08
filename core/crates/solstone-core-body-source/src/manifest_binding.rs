// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use crate::{
    BODY_BUNDLE_REF_KEY, BODY_BUNDLE_SHA256_KEY, BODY_SOURCE_SCHEMA_KEY, BodyDay, BodyDigest,
    BodyInteger, BodyObject, BodyRawRetention, BodySourceFamily, BodySourceHash, BodyString,
    BodyValue, BundleId, DAYS_AFFECTED_KEY, ENTRY_COUNT_KEY, IMPORT_ID_KEY, ManifestBindingError,
    ManifestBindingErrorCode, ManifestBindingErrorField, RAW_RETENTION_KEY, SOURCE_HASH_KEY,
    SOURCE_TYPE_KEY,
};

pub(crate) const BODY_SOURCE_SCHEMA_VALUE: &str = "solstone.body.bundle.v1";
pub(crate) const BODY_BUNDLE_REF_VALUE: &str = "body-bundle.json";

/// Checked native body-manifest values bound to one bundle.
pub struct BodyManifestBinding {
    body_source_schema: &'static str,
    body_bundle_ref: &'static str,
    body_bundle_sha256: BodyDigest,
    import_id: BundleId,
    source_type: BodySourceFamily,
    source_hash: BodySourceHash,
    entry_count: u64,
    days_affected: Vec<BodyDay>,
    raw_retention: BodyRawRetention,
}

impl BodyManifestBinding {
    /// Binds checked native body-manifest values for one bundle.
    pub fn new(
        body_bundle_sha256: BodyDigest,
        import_id: BundleId,
        source_type: BodySourceFamily,
        source_hash: BodySourceHash,
        entry_count: u64,
        days_affected: Vec<BodyDay>,
        raw_retention: BodyRawRetention,
    ) -> Result<Self, ManifestBindingError> {
        if source_hash.family() != source_type {
            return Err(ManifestBindingError::new(
                import_id.clone(),
                ManifestBindingErrorCode::IncompatibleField,
                ManifestBindingErrorField::SourceHash,
            ));
        }
        if !days_affected.windows(2).all(|window| window[0] < window[1]) {
            return Err(ManifestBindingError::new(
                import_id.clone(),
                ManifestBindingErrorCode::InvalidField,
                ManifestBindingErrorField::DaysAffected,
            ));
        }
        let empty = days_affected.is_empty();
        if (entry_count == 0) != empty || (days_affected.len() as u64) > entry_count {
            return Err(ManifestBindingError::new(
                import_id.clone(),
                ManifestBindingErrorCode::IncompatibleField,
                ManifestBindingErrorField::DaysAffected,
            ));
        }
        raw_retention.check_compatible(&source_type).map_err(|_| {
            ManifestBindingError::new(
                import_id.clone(),
                ManifestBindingErrorCode::IncompatibleField,
                ManifestBindingErrorField::RawRetention,
            )
        })?;

        Ok(Self {
            body_source_schema: BODY_SOURCE_SCHEMA_VALUE,
            body_bundle_ref: BODY_BUNDLE_REF_VALUE,
            body_bundle_sha256,
            import_id,
            source_type,
            source_hash,
            entry_count,
            days_affected,
            raw_retention,
        })
    }

    /// Returns the fixed body-source schema spelling.
    pub fn body_source_schema(&self) -> &str {
        self.body_source_schema
    }

    /// Returns the fixed bundle-reference spelling.
    pub fn body_bundle_ref(&self) -> &str {
        self.body_bundle_ref
    }

    /// Returns the checked body-bundle digest.
    pub fn body_bundle_sha256(&self) -> &BodyDigest {
        &self.body_bundle_sha256
    }

    /// Returns the checked import identifier.
    pub fn import_id(&self) -> &BundleId {
        &self.import_id
    }

    /// Returns the checked source family.
    pub fn source_type(&self) -> BodySourceFamily {
        self.source_type
    }

    /// Returns the checked source hash.
    pub fn source_hash(&self) -> &BodySourceHash {
        &self.source_hash
    }

    /// Returns the declared entry count.
    pub fn entry_count(&self) -> u64 {
        self.entry_count
    }

    /// Returns the strictly ordered affected days.
    pub fn days_affected(&self) -> &[BodyDay] {
        &self.days_affected
    }

    /// Returns the checked raw-retention policy.
    pub fn raw_retention(&self) -> BodyRawRetention {
        self.raw_retention
    }

    /// Emits the checked values as a native body-manifest object.
    pub fn to_body_object(&self) -> BodyObject {
        let mut object = BTreeMap::new();
        object.insert(
            body_string(BODY_SOURCE_SCHEMA_KEY),
            BodyValue::String(body_string(self.body_source_schema)),
        );
        object.insert(
            body_string(BODY_BUNDLE_REF_KEY),
            BodyValue::String(body_string(self.body_bundle_ref)),
        );
        object.insert(
            body_string(BODY_BUNDLE_SHA256_KEY),
            BodyValue::String(self.body_bundle_sha256.to_body_string()),
        );
        object.insert(
            body_string(IMPORT_ID_KEY),
            BodyValue::String(self.import_id.to_body_string()),
        );
        object.insert(
            body_string(SOURCE_TYPE_KEY),
            BodyValue::String(self.source_type.to_body_string()),
        );
        object.insert(
            body_string(SOURCE_HASH_KEY),
            BodyValue::String(self.source_hash.to_body_string()),
        );
        object.insert(
            body_string(ENTRY_COUNT_KEY),
            BodyValue::Integer(BodyInteger::from_u64(self.entry_count)),
        );
        object.insert(
            body_string(DAYS_AFFECTED_KEY),
            BodyValue::Array(
                self.days_affected
                    .iter()
                    .map(|day| BodyValue::String(day.to_body_string()))
                    .collect(),
            ),
        );
        object.insert(
            body_string(RAW_RETENTION_KEY),
            BodyValue::String(self.raw_retention.to_body_string()),
        );
        object
    }
}

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.chars().map(u32::from).collect())
        .expect("native body-manifest literals are valid body strings")
}
