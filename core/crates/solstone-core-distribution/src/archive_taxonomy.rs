// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Byte-level archive container taxonomy for the macOS producer.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContainerKind {
    RawTar,
    GzipTar,
    Bzip2Tar,
    XzTar,
    ZstdTar,
    Zip,
}

/// Classify a recognized archive container by bytes alone.
///
/// This deliberately ignores filenames and extensions: an archive hidden
/// behind an arbitrary staged path is still an archive the producer must
/// account for.
#[must_use]
pub fn classify_container(bytes: &[u8]) -> Option<ContainerKind> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Some(ContainerKind::GzipTar);
    }
    if bytes.starts_with(b"BZh") {
        return Some(ContainerKind::Bzip2Tar);
    }
    if bytes.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
        return Some(ContainerKind::XzTar);
    }
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Some(ContainerKind::ZstdTar);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Some(ContainerKind::Zip);
    }
    if bytes.get(257..262) == Some(b"ustar".as_slice())
        || bytes.get(257..261) == Some(b"GNU ".as_slice())
    {
        return Some(ContainerKind::RawTar);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ContainerKind, classify_container};

    #[test]
    fn recognizes_each_declared_container_signature() {
        let mut raw_tar = vec![0; 262];
        raw_tar[257..262].copy_from_slice(b"ustar");

        for (bytes, expected) in [
            (raw_tar, ContainerKind::RawTar),
            (vec![0x1f, 0x8b], ContainerKind::GzipTar),
            (b"BZh9".to_vec(), ContainerKind::Bzip2Tar),
            (
                vec![0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00],
                ContainerKind::XzTar,
            ),
            (vec![0x28, 0xb5, 0x2f, 0xfd], ContainerKind::ZstdTar),
            (b"PK\x03\x04".to_vec(), ContainerKind::Zip),
        ] {
            assert_eq!(classify_container(&bytes), Some(expected));
        }
    }

    #[test]
    fn recognizes_gnu_raw_tar_marker() {
        let mut raw_tar = vec![0; 261];
        raw_tar[257..261].copy_from_slice(b"GNU ");
        assert_eq!(classify_container(&raw_tar), Some(ContainerKind::RawTar));
    }

    #[test]
    fn ignores_non_archives() {
        assert_eq!(classify_container(b"plain text"), None);
        assert_eq!(classify_container(&0xfeed_facfu32.to_le_bytes()), None);
        assert_eq!(classify_container(&vec![0; 261]), None);
    }
}
