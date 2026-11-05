// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A compile-time embedded response asset.
pub struct EmbeddedAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

/// Source bytes for a dashboard asset. Template hashes describe the source,
/// before response-time substitutions. Routes continue to use their own handlers.
pub struct AssetRecord {
    pub path: &'static str,
    pub content_type: &'static str,
    pub byte_len: usize,
    pub sha256: &'static str,
    pub template: bool,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));

pub fn lookup(path: &str) -> Option<&'static EmbeddedAsset> {
    GENERATED_ASSETS
        .binary_search_by_key(&path, |asset| asset.path)
        .ok()
        .map(|index| &GENERATED_ASSETS[index])
}

pub fn speaker_copy_json() -> &'static str {
    SPEAKER_COPY_JSON
}

pub fn network_copy_json() -> &'static str {
    NETWORK_COPY_JSON
}

pub fn spl_outcome_strings_json() -> &'static str {
    SPL_OUTCOME_STRINGS_JSON
}

pub fn home_address_strings_json() -> &'static str {
    HOME_ADDRESS_STRINGS_JSON
}

pub fn not_in_new_voices_copy() -> &'static str {
    NOT_IN_NEW_VOICES_COPY
}
