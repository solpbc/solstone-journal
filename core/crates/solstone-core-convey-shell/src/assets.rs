// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A compile-time embedded response asset.
pub struct EmbeddedAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

/// How a dashboard asset's response relates to its embedded bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    /// The response is exactly the bytes, with references rewritten.
    File,
    /// The owning crate fills in values fixed for this build at response time.
    Template,
    /// The response depends on the request or this computer; never cached.
    Dynamic,
}

/// Build-time metadata for every dashboard asset, from every crate that serves
/// one. Each crate's build script lists its own; this crate's build script
/// hashes the trees and computes the rewrites. Routes keep their own handlers.
pub struct AssetRecord {
    pub path: &'static str,
    pub content_type: &'static str,
    pub byte_len: usize,
    /// SHA-256 of the source bytes, before any rewrite or substitution.
    pub sha256: &'static str,
    pub kind: AssetKind,
    /// The hashed tree this file belongs to, if any.
    pub tree: Option<&'static str>,
    /// Where this file is served for good, under its tree's hash.
    pub hashed: Option<&'static str>,
    /// Strong validator of the bytes served (empty unless `File`).
    pub etag: &'static str,
    /// Byte ranges of the source rewritten to hashed paths, in order.
    pub edits: &'static [(usize, usize, &'static str)],
    /// The served bytes gzipped at build time, for a compressible `File` the
    /// server would gzip anyway; empty otherwise.
    pub gzip: &'static [u8],
    #[cfg(test)]
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

/// The record for a stable dashboard path.
pub fn record(path: &str) -> Option<&'static AssetRecord> {
    INVENTORY
        .binary_search_by_key(&path, |record| record.path)
        .ok()
        .map(|index| &INVENTORY[index])
}

/// The record served at a hashed path of this build.
pub fn hashed_record(path: &str) -> Option<&'static AssetRecord> {
    HASHED
        .binary_search_by_key(&path, |(hashed, _)| *hashed)
        .ok()
        .map(|index| &INVENTORY[HASHED[index].1])
}

/// Every hashed path of this build, sorted.
pub fn hashed_paths() -> impl Iterator<Item = &'static str> {
    HASHED.iter().map(|(hashed, _)| *hashed)
}

/// One name for this build of the dashboard: the journal's version and every
/// embedded shell asset's own hash. A page compares the name its stream
/// reports on each connect, so it learns when the journal was upgraded under it.
pub fn server_build() -> &'static str {
    use sha2::{Digest, Sha256};
    static BUILD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BUILD.get_or_init(|| {
        let mut digest = Sha256::new();
        digest.update(env!("CARGO_PKG_VERSION").as_bytes());
        digest.update(b"\n");
        for record in INVENTORY {
            digest.update(record.path.as_bytes());
            digest.update(b"\0");
            digest.update(record.sha256.as_bytes());
            digest.update(b"\n");
        }
        format!("{:x}", digest.finalize())[..16].to_owned()
    })
}

/// Each file the shell page loads through a bundle: (bundle, file), in the
/// page's order, stylesheets first.
pub fn shell_bundle_segments() -> &'static [(&'static str, &'static str)] {
    SHELL_BUNDLE_SEGMENTS
}

/// Each asset tree and its hash.
pub fn trees() -> &'static [(&'static str, &'static str)] {
    TREES
}

/// The bytes the dashboard serves for a stable asset path: the source with its
/// references rewritten to this build's hashed paths.
#[cfg(test)]
pub fn served_bytes(path: &str) -> Vec<u8> {
    let record = record(path).unwrap_or_else(|| panic!("no asset record for {path}"));
    crate::asset_cache::rewrite_source(record.bytes, record.edits)
}

/// The bytes the composed dashboard serves for an embedded file, given the
/// file's own bytes: its references rewritten to this build's hashed paths.
/// Lets a crate's tests compare what the dashboard answers with its file.
pub fn served_from_source(source: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(source));
    match INVENTORY
        .iter()
        .find(|record| record.byte_len == source.len() && record.sha256 == hash)
    {
        Some(record) => crate::asset_cache::rewrite_source(source, record.edits),
        None => source.to_vec(),
    }
}
