// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::BodyString;

const BODY_PREFIX: [u32; 5] = [
    b'b' as u32,
    b'o' as u32,
    b'd' as u32,
    b'y' as u32,
    b'_' as u32,
];

pub const BODY_SOURCE_SCHEMA_KEY: &str = "body_source_schema";
pub const BODY_BUNDLE_REF_KEY: &str = "body_bundle_ref";
pub const BODY_BUNDLE_SHA256_KEY: &str = "body_bundle_sha256";
pub const IMPORT_ID_KEY: &str = "import_id";
pub const SOURCE_TYPE_KEY: &str = "source_type";
pub const SOURCE_HASH_KEY: &str = "source_hash";
pub const ENTRY_COUNT_KEY: &str = "entry_count";
pub const DAYS_AFFECTED_KEY: &str = "days_affected";
pub const RAW_RETENTION_KEY: &str = "raw_retention";

/// The closed vocabulary of known body manifest keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManifestKnownKey {
    BodySourceSchema,
    BodyBundleRef,
    BodyBundleSha256,
    ImportId,
    SourceType,
    SourceHash,
    EntryCount,
    DaysAffected,
    RawRetention,
}

impl ManifestKnownKey {
    pub(crate) const ALL: [Self; 9] = [
        Self::BodySourceSchema,
        Self::BodyBundleRef,
        Self::BodyBundleSha256,
        Self::ImportId,
        Self::SourceType,
        Self::SourceHash,
        Self::EntryCount,
        Self::DaysAffected,
        Self::RawRetention,
    ];

    /// Returns this key's exact wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::BodySourceSchema => BODY_SOURCE_SCHEMA_KEY,
            Self::BodyBundleRef => BODY_BUNDLE_REF_KEY,
            Self::BodyBundleSha256 => BODY_BUNDLE_SHA256_KEY,
            Self::ImportId => IMPORT_ID_KEY,
            Self::SourceType => SOURCE_TYPE_KEY,
            Self::SourceHash => SOURCE_HASH_KEY,
            Self::EntryCount => ENTRY_COUNT_KEY,
            Self::DaysAffected => DAYS_AFFECTED_KEY,
            Self::RawRetention => RAW_RETENTION_KEY,
        }
    }

    /// Resolves a decoded body-string key from the closed vocabulary.
    pub fn from_body_string(value: &BodyString) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|known| body_string_matches(value, known.as_str()))
    }
}

pub(crate) fn starts_with_body_prefix(key: &BodyString) -> bool {
    key.code_points().starts_with(&BODY_PREFIX)
}

fn body_string_matches(value: &BodyString, literal: &str) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .eq(literal.bytes().map(u32::from))
}
