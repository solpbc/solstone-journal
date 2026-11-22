// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The signed Linux release catalogue, read and verified in-process.
//!
//! Checking for an update fetches `latest`, `platform.json` and its minisign
//! signature, verifies the signature against the compiled platform release
//! key, and reads the versions. It runs no downloaded code.

use std::path::Path;
use std::sync::Mutex;

use minisign::PublicKey;

use crate::fetch::{download_bytes, update_origin};

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
    pub desktop_version: Option<String>,
    /// The installer copy published beside this catalogue, as the signed
    /// catalogue names it. Catalogues from before installer revision 22 name
    /// none, and nothing runs an installer for them unattended.
    pub installer: Option<InstallerCopy>,
}

/// An installer copy a signed catalogue names: published at
/// `solstone/<lane>/<coordinate>/<filename>` on the update origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerCopy {
    pub filename: String,
    pub sha256: String,
    pub revision: u64,
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
    fn listing_for(
        &self,
        _locale: &str,
        lane: &str,
        version: Option<&str>,
    ) -> Result<CatalogueListing, CatalogueRefusal> {
        self.listing(lane, version)
    }
}

/// Reads one catalogue document by its path under the update origin.
pub trait CatalogueBytes {
    fn get(&self, path: &str, max_bytes: u64) -> std::io::Result<Vec<u8>>;
}

struct HttpCatalogueBytes;

impl CatalogueBytes for HttpCatalogueBytes {
    fn get(&self, path: &str, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let (origin, host) = update_origin();
        download_bytes(&format!("{origin}{path}"), host, max_bytes)
    }
}

pub struct RealPlatformCatalogue;

impl PlatformCatalogue for RealPlatformCatalogue {
    fn listing(
        &self,
        lane: &str,
        version: Option<&str>,
    ) -> Result<CatalogueListing, CatalogueRefusal> {
        self.listing_for(solstone_core_cli_copy::ENGLISH, lane, version)
    }

    fn listing_for(
        &self,
        locale: &str,
        lane: &str,
        version: Option<&str>,
    ) -> Result<CatalogueListing, CatalogueRefusal> {
        let key = crate::fetch::update_platform_key().ok_or_else(|| {
            CatalogueRefusal::new(
                "pin-invalid",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.catalogue.key",
                    &[],
                ),
            )
        })?;
        verified_listing_in(locale, &HttpCatalogueBytes, &key, lane, version)
    }
}

/// The answer a scheduled check last got. Reading it never touches the network,
/// so what reads it leaves no trace at the update origin.
#[derive(Debug, Default)]
pub struct StoredCatalogue {
    answer: Mutex<Option<StoredAnswer>>,
}

#[derive(Debug)]
struct StoredAnswer {
    lane: String,
    listing: Result<CatalogueListing, CatalogueRefusal>,
}

impl StoredCatalogue {
    pub const fn new() -> Self {
        Self {
            answer: Mutex::new(None),
        }
    }

    /// Ask `source` and keep what it says, a refusal included. Returns whether
    /// it answered with a verified listing.
    pub fn refresh(&self, source: &dyn PlatformCatalogue, lane: &str) -> bool {
        let listing = source.listing(lane, None);
        let verified = listing.is_ok();
        *self.answer.lock().unwrap_or_else(|p| p.into_inner()) = Some(StoredAnswer {
            lane: lane.to_owned(),
            listing,
        });
        verified
    }
}

impl PlatformCatalogue for StoredCatalogue {
    fn listing(
        &self,
        lane: &str,
        version: Option<&str>,
    ) -> Result<CatalogueListing, CatalogueRefusal> {
        let answer = self.answer.lock().unwrap_or_else(|p| p.into_inner());
        match answer.as_ref() {
            Some(answer) if version.is_none() && answer.lane == lane => answer.listing.clone(),
            _ => Err(CatalogueRefusal::new(
                "update-check-pending",
                "the journal has not checked for updates yet.",
            )),
        }
    }
}

/// Fetch and verify the catalogue for `lane`, at `version` or at `latest`.
pub fn verified_listing(
    bytes: &dyn CatalogueBytes,
    key: &PublicKey,
    lane: &str,
    version: Option<&str>,
) -> Result<CatalogueListing, CatalogueRefusal> {
    verified_listing_in(solstone_core_cli_copy::ENGLISH, bytes, key, lane, version)
}

pub fn verified_listing_in(
    locale: &str,
    bytes: &dyn CatalogueBytes,
    key: &PublicKey,
    lane: &str,
    version: Option<&str>,
) -> Result<CatalogueListing, CatalogueRefusal> {
    if !matches!(lane, "release" | "staging" | "dev") {
        return Err(CatalogueRefusal::new(
            "lane-invalid",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.sol.update.catalogue.lane",
                [("lane", &(lane))],
            ),
        ));
    }
    let resolved = match version {
        Some(version) if is_catalogue_coordinate(version) => version.to_owned(),
        Some(_) => {
            return Err(CatalogueRefusal::new(
                "version-invalid",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.catalogue.version",
                    &[],
                ),
            ));
        }
        None => {
            let latest = fetch(
                locale,
                bytes,
                &format!("/solstone/{lane}/latest"),
                LATEST_MAX_BYTES,
            )?;
            parse_latest(locale, &latest)?
        }
    };
    let base = format!("/solstone/{lane}/{resolved}");
    let manifest = fetch(
        locale,
        bytes,
        &format!("{base}/platform.json"),
        MANIFEST_MAX_BYTES,
    )?;
    let signature = fetch(
        locale,
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
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.signature",
                &[],
            ),
        )
    })?;
    read_manifest(locale, &manifest, key, lane, &resolved)
}

fn fetch(
    locale: &str,
    bytes: &dyn CatalogueBytes,
    path: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, CatalogueRefusal> {
    bytes.get(path, max_bytes).map_err(|_| {
        CatalogueRefusal::new(
            "fetch-failed",
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.download",
                &[],
            ),
        )
    })
}

fn parse_latest(locale: &str, bytes: &[u8]) -> Result<String, CatalogueRefusal> {
    let text = std::str::from_utf8(bytes).ok();
    let coordinate = text
        .map(|text| {
            text.strip_suffix("\r\n")
                .or_else(|| text.strip_suffix('\n'))
                .unwrap_or(text)
        })
        .filter(|coordinate| is_catalogue_coordinate(coordinate));
    coordinate.map(str::to_owned).ok_or_else(|| {
        CatalogueRefusal::new(
            "latest-invalid",
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.latest",
                &[],
            ),
        )
    })
}

fn read_manifest(
    locale: &str,
    bytes: &[u8],
    key: &PublicKey,
    lane: &str,
    resolved: &str,
) -> Result<CatalogueListing, CatalogueRefusal> {
    let invalid = |message| CatalogueRefusal::new("schema-invalid", message);
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| {
        invalid(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.sol.update.catalogue.schema.not.json",
            &[],
        ))
    })?;
    if value["protocol_version"].as_u64() != Some(1) {
        return Err(invalid(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.sol.update.catalogue.schema.unsupported.protocol",
            &[],
        )));
    }
    let version = value["version"].as_str().ok_or_else(|| {
        invalid(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.sol.update.catalogue.schema.no.version",
            &[],
        ))
    })?;
    let journal = value["components"]["journal"]["version"]
        .as_str()
        .ok_or_else(|| {
            invalid(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.schema.no.journal.version",
                &[],
            ))
        })?;
    let desktop_version = match value["components"]["desktop"]["version"].as_str() {
        Some(v) if is_canonical_version(v) => Some(v.to_owned()),
        Some(_) => {
            return Err(invalid(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.schema.bad.desktop.version",
                &[],
            )));
        }
        None => None,
    };
    let coordinate = match value["schema_version"].as_u64() {
        Some(1) if value.get("catalogue_revision").is_none() => version.to_owned(),
        Some(2) => match value["catalogue_revision"].as_u64() {
            Some(revision) if revision > 0 && version == journal => {
                format!("{version}-r{revision}")
            }
            _ => {
                return Err(invalid(solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.catalogue.schema.bad.catalogue.revision",
                    &[],
                )));
            }
        },
        _ => {
            return Err(invalid(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.schema.unsupported.schema",
                &[],
            )));
        }
    };
    if !is_canonical_version(version) || !is_canonical_version(journal) {
        return Err(invalid(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.sol.update.catalogue.schema.bad.version",
            &[],
        )));
    }
    if coordinate != resolved || value["lane"].as_str() != Some(lane) {
        return Err(CatalogueRefusal::new(
            "release-coherence",
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.release",
                &[],
            ),
        ));
    }
    let installer = match value.get("installer") {
        None => None,
        Some(block) if value["schema_version"].as_u64() == Some(2) => {
            Some(read_installer_copy(block).ok_or_else(|| {
                invalid(solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.catalogue.schema.bad.installer",
                    &[],
                ))
            })?)
        }
        Some(_) => {
            return Err(invalid(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.catalogue.schema.installer.in.an.unsupported.schema",
                &[],
            )));
        }
    };
    let keynum: [u8; 8] = key.keynum().try_into().map_err(|_| {
        invalid(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.sol.update.catalogue.schema.key.id",
            &[],
        ))
    })?;
    if value["platform_key_id"].as_str()
        != Some(format!("{:016X}", u64::from_le_bytes(keynum)).as_str())
    {
        return Err(CatalogueRefusal::new(
            "pin-mismatch",
            solstone_core_cli_copy::render_for(locale, "journal.cli.sol.update.catalogue.pin", &[]),
        ));
    }
    Ok(CatalogueListing {
        platform_version: coordinate,
        journal_version: journal.to_owned(),
        desktop_version,
        installer,
    })
}

fn read_installer_copy(block: &serde_json::Value) -> Option<InstallerCopy> {
    let fields = block.as_object()?;
    if fields.len() != 3 {
        return None;
    }
    let filename = fields.get("filename")?.as_str()?;
    let sha256 = fields.get("sha256")?.as_str()?;
    let revision = fields.get("revision")?.as_u64()?;
    let digest_ok = sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (filename == "install.sh" && digest_ok && revision > 0).then(|| InstallerCopy {
        filename: filename.to_owned(),
        sha256: sha256.to_owned(),
        revision,
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

    struct Answers(Mutex<Vec<Result<CatalogueListing, CatalogueRefusal>>>);

    impl PlatformCatalogue for Answers {
        fn listing(&self, _: &str, _: Option<&str>) -> Result<CatalogueListing, CatalogueRefusal> {
            self.0.lock().unwrap().remove(0)
        }
    }

    #[test]
    fn the_stored_answer_is_the_newest_one_for_its_lane() {
        let listing = CatalogueListing {
            platform_version: "2.0.65-r1".to_owned(),
            journal_version: "2.0.65".to_owned(),
            desktop_version: None,
            installer: None,
        };
        let refusal = CatalogueRefusal::new("signature-invalid", "refused");
        let origin = Answers(Mutex::new(vec![Ok(listing.clone()), Err(refusal.clone())]));
        let stored = StoredCatalogue::new();
        let code = |answer: Result<CatalogueListing, CatalogueRefusal>| answer.unwrap_err().code;

        assert_eq!(
            code(stored.listing("release", None)),
            "update-check-pending"
        );

        assert!(stored.refresh(&origin, "release"));
        assert_eq!(stored.listing("release", None), Ok(listing));
        // It answers only for the lane it holds, and only about the latest release.
        assert_eq!(code(stored.listing("dev", None)), "update-check-pending");
        assert_eq!(
            code(stored.listing("release", Some("2.0.65-r1"))),
            "update-check-pending"
        );

        // A refused catalogue replaces a good answer instead of hiding behind it.
        assert!(!stored.refresh(&origin, "release"));
        assert_eq!(stored.listing("release", None), Err(refusal));
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
                desktop_version: Some("2.0.29".to_owned()),
                installer: None,
            })
        );
    }

    #[test]
    fn a_signed_catalogue_names_its_installer_copy_and_a_malformed_name_is_refused() {
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        let with = |installer: serde_json::Value| {
            let mut value: serde_json::Value = serde_json::from_slice(&manifest(&pair.pk)).unwrap();
            value["installer"] = installer;
            serde_json::to_vec(&value).unwrap()
        };
        let digest = "ab".repeat(32);
        let bytes =
            with(serde_json::json!({"filename": "install.sh", "revision": 22, "sha256": digest}));
        let site = served(&bytes, &sign(&pair, &bytes));
        assert_eq!(
            verified_listing(&site, &pair.pk, "release", None)
                .unwrap()
                .installer,
            Some(InstallerCopy {
                filename: "install.sh".to_owned(),
                sha256: digest.clone(),
                revision: 22,
            })
        );
        for block in [
            serde_json::json!({"filename": "other.sh", "revision": 22, "sha256": digest}),
            serde_json::json!({"filename": "install.sh", "revision": 22, "sha256": "AB".repeat(32)}),
            serde_json::json!({"filename": "install.sh", "revision": "22", "sha256": digest}),
            serde_json::json!({"filename": "install.sh", "revision": 22, "sha256": digest, "url": "x"}),
            serde_json::json!("install.sh"),
        ] {
            let bytes = with(block);
            let site = served(&bytes, &sign(&pair, &bytes));
            assert_eq!(
                verified_listing(&site, &pair.pk, "release", None)
                    .unwrap_err()
                    .code,
                "schema-invalid"
            );
        }
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
        assert_eq!(
            parse_latest(solstone_core_cli_copy::ENGLISH, b"2.0.59-r1\r\n").unwrap(),
            "2.0.59-r1"
        );
        assert!(parse_latest(solstone_core_cli_copy::ENGLISH, b"2.0.59-r1\n\n").is_err());
    }
}
