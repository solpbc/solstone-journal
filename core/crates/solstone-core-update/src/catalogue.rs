// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The signed Linux release catalogue, read and verified in-process.
//!
//! Checking for an update fetches `latest`, `platform.json` and its minisign
//! signature, verifies the signature against the compiled platform release
//! key, and reads the versions. It runs no downloaded code.

use std::path::Path;

use minisign::PublicKey;

use crate::fetch::{UPDATE_ORIGIN, download_bytes};

const CATALOGUE_HOST: &str = "updates.solstone.app";
const LATEST_MAX_BYTES: u64 = 86;
const MANIFEST_MAX_BYTES: u64 = 4 * 1024 * 1024;
const SIGNATURE_MAX_BYTES: u64 = 16 * 1024;
const MAX_DIGITS: usize = 20;

/// What a verified catalogue says is current on a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueListing {
    /// The catalogue coordinate, such as `2.0.59-r1`.
    pub platform_version: String,
    pub journal_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueRefusal {
    pub code: &'static str,
    pub message: String,
}

impl CatalogueRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Where the update check learns the latest release. The only production
/// implementation, [`RealPlatformCatalogue`], verifies before it answers.
pub trait PlatformCatalogue: Send + Sync {
    fn listing(
        &self,
        lane: &str,
        version: Option<&str>,
    ) -> Result<CatalogueListing, CatalogueRefusal>;
}

/// Reads one catalogue document by its path under the update origin.
pub trait CatalogueBytes {
    fn get(&self, path: &str, max_bytes: u64) -> std::io::Result<Vec<u8>>;
}

struct HttpCatalogueBytes;

impl CatalogueBytes for HttpCatalogueBytes {
    fn get(&self, path: &str, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        download_bytes(&format!("{UPDATE_ORIGIN}{path}"), CATALOGUE_HOST, max_bytes)
    }
}

pub struct RealPlatformCatalogue;

impl PlatformCatalogue for RealPlatformCatalogue {
    fn listing(
        &self,
        lane: &str,
        version: Option<&str>,
    ) -> Result<CatalogueListing, CatalogueRefusal> {
        let key = solstone_core_installed_payload::platform_pin().map_err(|_| {
            CatalogueRefusal::new(
                "pin-invalid",
                "the release key in this build is unreadable.",
            )
        })?;
        verified_listing(&HttpCatalogueBytes, &key, lane, version)
    }
}

/// Fetch and verify the catalogue for `lane`, at `version` or at `latest`.
pub fn verified_listing(
    bytes: &dyn CatalogueBytes,
    key: &PublicKey,
    lane: &str,
    version: Option<&str>,
) -> Result<CatalogueListing, CatalogueRefusal> {
    if !matches!(lane, "release" | "staging" | "dev") {
        return Err(CatalogueRefusal::new(
            "lane-invalid",
            format!("unknown release lane: {lane}"),
        ));
    }
    let resolved = match version {
        Some(version) if is_catalogue_coordinate(version) => version.to_owned(),
        Some(_) => {
            return Err(CatalogueRefusal::new(
                "version-invalid",
                "the requested platform version is invalid.",
            ));
        }
        None => {
            let latest = fetch(bytes, &format!("/solstone/{lane}/latest"), LATEST_MAX_BYTES)?;
            parse_latest(&latest)?
        }
    };
    let base = format!("/solstone/{lane}/{resolved}");
    let manifest = fetch(bytes, &format!("{base}/platform.json"), MANIFEST_MAX_BYTES)?;
    let signature = fetch(
        bytes,
        &format!("{base}/platform.json.minisig"),
        SIGNATURE_MAX_BYTES,
    )?;
    solstone_core_installed_payload::verify_prehashed_signature_with(
        key,
        &manifest,
        Path::new("platform.json.minisig"),
        &signature,
    )
    .map_err(|_| {
        CatalogueRefusal::new(
            "signature-invalid",
            "the release catalogue failed its signature check.",
        )
    })?;
    read_manifest(&manifest, key, lane, &resolved)
}

fn fetch(
    bytes: &dyn CatalogueBytes,
    path: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, CatalogueRefusal> {
    bytes.get(path, max_bytes).map_err(|_| {
        CatalogueRefusal::new("fetch-failed", "could not fetch the release catalogue.")
    })
}

fn parse_latest(bytes: &[u8]) -> Result<String, CatalogueRefusal> {
    let text = std::str::from_utf8(bytes).ok();
    let coordinate = text
        .map(|text| {
            text.strip_suffix("\r\n")
                .or_else(|| text.strip_suffix('\n'))
                .unwrap_or(text)
        })
        .filter(|coordinate| is_catalogue_coordinate(coordinate));
    coordinate.map(str::to_owned).ok_or_else(|| {
        CatalogueRefusal::new("latest-invalid", "the latest release marker is malformed.")
    })
}

fn read_manifest(
    bytes: &[u8],
    key: &PublicKey,
    lane: &str,
    resolved: &str,
) -> Result<CatalogueListing, CatalogueRefusal> {
    let invalid = |detail: &str| {
        CatalogueRefusal::new(
            "schema-invalid",
            format!("the release catalogue is invalid: {detail}."),
        )
    };
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| invalid("not json"))?;
    if value["protocol_version"].as_u64() != Some(1) {
        return Err(invalid("unsupported protocol"));
    }
    let version = value["version"]
        .as_str()
        .ok_or_else(|| invalid("no version"))?;
    let journal = value["components"]["journal"]["version"]
        .as_str()
        .ok_or_else(|| invalid("no journal version"))?;
    let coordinate = match value["schema_version"].as_u64() {
        Some(1) if value.get("catalogue_revision").is_none() => version.to_owned(),
        Some(2) => match value["catalogue_revision"].as_u64() {
            Some(revision) if revision > 0 && version == journal => {
                format!("{version}-r{revision}")
            }
            _ => return Err(invalid("bad catalogue revision")),
        },
        _ => return Err(invalid("unsupported schema")),
    };
    if !is_canonical_version(version) || !is_canonical_version(journal) {
        return Err(invalid("bad version"));
    }
    if coordinate != resolved || value["lane"].as_str() != Some(lane) {
        return Err(CatalogueRefusal::new(
            "release-coherence",
            "the release catalogue does not match the requested release.",
        ));
    }
    let keynum: [u8; 8] = key.keynum().try_into().map_err(|_| invalid("key id"))?;
    if value["platform_key_id"].as_str()
        != Some(format!("{:016X}", u64::from_le_bytes(keynum)).as_str())
    {
        return Err(CatalogueRefusal::new(
            "pin-mismatch",
            "the release catalogue names a different release key.",
        ));
    }
    Ok(CatalogueListing {
        platform_version: coordinate,
        journal_version: journal.to_owned(),
    })
}

fn is_canonical_version(candidate: &str) -> bool {
    let parts: Vec<&str> = candidate.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= MAX_DIGITS
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part == &"0" || !part.starts_with('0'))
        })
}

fn is_catalogue_coordinate(candidate: &str) -> bool {
    if is_canonical_version(candidate) {
        return true;
    }
    candidate.rsplit_once("-r").is_some_and(|(core, revision)| {
        is_canonical_version(core)
            && !revision.is_empty()
            && revision.len() <= MAX_DIGITS
            && !revision.starts_with('0')
            && revision.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Cursor;

    use minisign::KeyPair;

    use super::*;

    struct Served(HashMap<String, Vec<u8>>);

    impl CatalogueBytes for Served {
        fn get(&self, path: &str, max_bytes: u64) -> std::io::Result<Vec<u8>> {
            let bytes = self
                .0
                .get(path)
                .cloned()
                .ok_or_else(|| std::io::Error::other("404"))?;
            if bytes.len() as u64 > max_bytes {
                return Err(std::io::Error::other("too large"));
            }
            Ok(bytes)
        }
    }

    fn key_id(key: &PublicKey) -> String {
        format!(
            "{:016X}",
            u64::from_le_bytes(key.keynum().try_into().unwrap())
        )
    }

    fn sign(pair: &KeyPair, bytes: &[u8]) -> Vec<u8> {
        minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(bytes),
            Some("solstone platform release manifest"),
            None,
        )
        .unwrap()
        .into_string()
        .into_bytes()
    }

    fn manifest(key: &PublicKey) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "catalogue_revision": 1,
            "components": {
                "journal": {"version": "2.0.59"},
                "desktop": {"version": "2.0.29"},
                "tmux": {"version": "2.0.20"},
            },
            "lane": "release",
            "minimum_installer_revision": 8,
            "platform_key_id": key_id(key),
            "protocol_version": 1,
            "schema_version": 2,
            "version": "2.0.59",
        }))
        .unwrap()
    }

    fn served(manifest: &[u8], signature: &[u8]) -> Served {
        let base = "/solstone/release/2.0.59-r1";
        Served(HashMap::from([
            (
                "/solstone/release/latest".to_owned(),
                b"2.0.59-r1\n".to_vec(),
            ),
            (format!("{base}/platform.json"), manifest.to_vec()),
            (format!("{base}/platform.json.minisig"), signature.to_vec()),
        ]))
    }

    #[test]
    fn a_signed_catalogue_lists_its_versions() {
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        let bytes = manifest(&pair.pk);
        let site = served(&bytes, &sign(&pair, &bytes));
        assert_eq!(
            verified_listing(&site, &pair.pk, "release", None),
            Ok(CatalogueListing {
                platform_version: "2.0.59-r1".to_owned(),
                journal_version: "2.0.59".to_owned(),
            })
        );
    }

    #[test]
    fn altered_bytes_or_another_key_are_refused_before_they_are_read() {
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        let other = KeyPair::generate_unencrypted_keypair().unwrap();
        let bytes = manifest(&pair.pk);
        let mut altered = bytes.clone();
        let at = altered.len() - 2;
        altered[at] ^= 1;
        for site in [
            served(&altered, &sign(&pair, &bytes)),
            served(&bytes, &sign(&other, &bytes)),
            served(&bytes, b"not a signature"),
        ] {
            assert_eq!(
                verified_listing(&site, &pair.pk, "release", None)
                    .unwrap_err()
                    .code,
                "signature-invalid"
            );
        }
    }

    #[test]
    fn a_signed_catalogue_for_another_release_is_refused() {
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        let bytes = manifest(&pair.pk);
        let mut site = served(&bytes, &sign(&pair, &bytes));
        site.0.insert(
            "/solstone/release/latest".to_owned(),
            b"2.0.59-r2\n".to_vec(),
        );
        let base = "/solstone/release/2.0.59-r2";
        site.0
            .insert(format!("{base}/platform.json"), bytes.clone());
        site.0
            .insert(format!("{base}/platform.json.minisig"), sign(&pair, &bytes));
        assert_eq!(
            verified_listing(&site, &pair.pk, "release", None)
                .unwrap_err()
                .code,
            "release-coherence"
        );
    }

    #[test]
    fn the_compiled_platform_pin_parses() {
        let key = solstone_core_installed_payload::platform_pin().unwrap();
        assert_eq!(key.keynum().len(), 8);
    }

    #[test]
    fn coordinates_follow_the_catalogue_grammar() {
        for good in ["2.0.59", "2.0.59-r1", "0.0.0", "10.20.30-r12"] {
            assert!(is_catalogue_coordinate(good), "{good}");
        }
        for bad in [
            "2.0",
            "2.0.059",
            "2.0.59-r0",
            "2.0.59-r",
            "2.0.59-r01",
            "../x",
            "",
        ] {
            assert!(!is_catalogue_coordinate(bad), "{bad}");
        }
        assert_eq!(parse_latest(b"2.0.59-r1\r\n").unwrap(), "2.0.59-r1");
        assert!(parse_latest(b"2.0.59-r1\n\n").is_err());
    }
}
