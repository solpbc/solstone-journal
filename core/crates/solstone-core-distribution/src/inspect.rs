// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::Path;

use crate::digest::sha256_hex;
use crate::inventory::{
    OS_LINUX, OS_MACOS, OS_WINDOWS, artifact_sidecars, checksum_members_for_os,
    manifest_members_for_os,
};

/// A binary from this release family may only downgrade to another retained
/// release carrying the same identifier.
pub const UPGRADE_EPOCH: &str = "journal-v2";
/// Number of version directories in the explicit downgrade window.
pub const RETENTION_WINDOW: usize = 3;
/// The minimum install.sh BOOTSTRAP_REVISION a release requires.
/// Invariant: must never exceed the BOOTSTRAP_REVISION of the installer live
/// at https://solstone.app/install.sh at promotion time
/// (core/distribution/install.sh's BOOTSTRAP_REVISION is the counterpart).
pub const MIN_BOOTSTRAP_REVISION: u32 = 2;
/// The contract version governing bootstrap script delivery and role options.
pub const BOOTSTRAP_CONTRACT_VERSION: u32 = 2;
/// Earliest state reader version supported by this release family.
pub const STATE_READER_MIN: &str = "1.0.0";
/// Bit-identical bytes of the canonical install.sh script.
pub const BOOTSTRAP_BYTES: &[u8] = include_bytes!("../../../distribution/install.sh");

#[derive(Clone, Copy)]
pub struct ArchiveChainDigests<'a> {
    pub prebuild_input_sha256: &'a str,
    pub delivery_contract_sha256: &'a str,
    pub final_invocation_sha256: &'a str,
}

pub struct ReleaseInfo<'a> {
    pub product: &'a str,
    pub version: &'a str,
    pub target: &'a str,
    pub commit: &'a str,
    pub lock_sha256: &'a str,
    pub archive_chain: Option<ArchiveChainDigests<'a>>,
}

pub fn write_sidecars(
    out_dir: &Path,
    os: &str,
    release: &ReleaseInfo<'_>,
    basename: &str,
) -> io::Result<()> {
    match os {
        OS_LINUX => {}
        OS_MACOS => {}
        OS_WINDOWS => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "windows archive/signing is not implemented in this lode",
            ));
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpected os {other}"),
            ));
        }
    }
    let bootstrap_name = format!("solstone-journal-{}-install.sh", release.version);
    let bootstrap_path = out_dir.join(&bootstrap_name);
    if bootstrap_path.exists() {
        let existing = fs::read(&bootstrap_path)?;
        if existing != BOOTSTRAP_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bootstrap script mismatch: {bootstrap_name}"),
            ));
        }
    } else {
        write_sidecar(&bootstrap_path, BOOTSTRAP_BYTES)?;
    }

    let [sha256, manifest_name, release_name] = artifact_sidecars(basename);
    write_sidecar(&out_dir.join(&release_name), render_release(release))?;

    let mut checksums = String::new();
    for name in checksum_members_for_os(os, basename)
        .map_err(|msg| io::Error::new(io::ErrorKind::InvalidInput, msg))?
    {
        let path = out_dir.join(&name);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sidecar-member-not-regular: {name}"),
            ));
        }
        let digest = sha256_hex(&fs::read(&path)?);
        checksums.push_str(&format!("{digest}  {name}\n"));
    }
    write_sidecar(&out_dir.join(&sha256), checksums)?;

    let mut manifest = String::from("{\n");
    manifest.push_str(&format!("  \"product\": {:?},\n", release.product));
    manifest.push_str(&format!("  \"version\": {:?},\n", release.version));
    manifest.push_str(&format!("  \"target\": {:?},\n", release.target));
    manifest.push_str("  \"files\": {\n");
    let members = manifest_members_for_os(os, basename)
        .map_err(|msg| io::Error::new(io::ErrorKind::InvalidInput, msg))?;
    for (index, name) in members.iter().enumerate() {
        let path = out_dir.join(name);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sidecar-member-not-regular: {name}"),
            ));
        }
        let digest = sha256_hex(&fs::read(path)?);
        let comma = if index + 1 == members.len() { "" } else { "," };
        manifest.push_str(&format!("    {name:?}: {digest:?}{comma}\n"));
    }
    manifest.push_str("  }\n}\n");
    write_sidecar(&out_dir.join(manifest_name), manifest)?;
    Ok(())
}

fn write_sidecar(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sidecar-not-regular: {}", path.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::write(path, bytes)
}

#[must_use]
pub fn render_release(release: &ReleaseInfo<'_>) -> String {
    let bootstrap_filename = format!("solstone-journal-{}-install.sh", release.version);
    let mut rendered = format!(
        "product={}\nversion={}\ntarget={}\ncommit={}\nlock_sha256={}\nupgrade_epoch={}\nretention_window={}\nmin_bootstrap_revision={}\nbootstrap_contract_version={}\nbootstrap_filename={}\nstate_reader_min={}\nstate_reader_max={}\n",
        release.product,
        release.version,
        release.target,
        release.commit,
        release.lock_sha256,
        UPGRADE_EPOCH,
        RETENTION_WINDOW,
        MIN_BOOTSTRAP_REVISION,
        BOOTSTRAP_CONTRACT_VERSION,
        bootstrap_filename,
        STATE_READER_MIN,
        release.version,
    );
    if let Some(chain) = release.archive_chain {
        rendered.push_str(&format!(
            "archive_prebuild_input_sha256={}\narchive_delivery_contract_sha256={}\narchive_final_invocation_sha256={}\n",
            chain.prebuild_input_sha256,
            chain.delivery_contract_sha256,
            chain.final_invocation_sha256,
        ));
    }
    rendered
}

pub fn self_inspect(out_dir: &Path, basename: &str) -> io::Result<Vec<(String, String)>> {
    let release_name = format!("{basename}.release");
    parse_release(&fs::read_to_string(out_dir.join(release_name))?)
}

pub fn parse_release(text: &str) -> io::Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "release-invalid",
            ));
        };
        pairs.push((key.to_owned(), value.to_owned()));
    }
    if !matches!(pairs.len(), 8 | 11 | 12 | 15) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "release-invalid",
        ));
    }
    Ok(pairs)
}

pub fn derive_bootstrap_contract_version(script_text: &str) -> Result<u32, String> {
    let contract_assignments = script_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && line.starts_with("BOOTSTRAP_CONTRACT_VERSION="))
        .collect::<Vec<_>>();

    if contract_assignments.is_empty() {
        return Err("missing BOOTSTRAP_CONTRACT_VERSION assignment in bootstrap script".to_owned());
    }
    if contract_assignments.len() > 1 {
        return Err(
            "duplicate BOOTSTRAP_CONTRACT_VERSION assignment in bootstrap script".to_owned(),
        );
    }
    let contract_val = contract_assignments[0]
        .strip_prefix("BOOTSTRAP_CONTRACT_VERSION=")
        .unwrap_or("")
        .trim();
    let version = contract_val
        .parse::<u32>()
        .map_err(|_| format!("invalid BOOTSTRAP_CONTRACT_VERSION integer: {contract_val}"))?;
    if version != BOOTSTRAP_CONTRACT_VERSION {
        return Err(format!(
            "BOOTSTRAP_CONTRACT_VERSION mismatch: expected {BOOTSTRAP_CONTRACT_VERSION}, found {version}"
        ));
    }
    Ok(version)
}

pub fn validate_bootstrap_agreement(script_text: &str) -> Result<(), String> {
    derive_bootstrap_contract_version(script_text)?;

    if script_text.as_bytes() != BOOTSTRAP_BYTES {
        return Err("bootstrap script bytes do not match source install.sh".to_owned());
    }

    let rev_line = format!("BOOTSTRAP_REVISION={MIN_BOOTSTRAP_REVISION}");
    let epoch_line = format!("SUPPORTED_UPGRADE_EPOCH={UPGRADE_EPOCH}");
    let window_line = format!("SUPPORTED_RETENTION_WINDOW={RETENTION_WINDOW}");
    let reader_min_line = format!("SUPPORTED_STATE_READER_MIN={STATE_READER_MIN}");

    if !script_text.lines().any(|l| l.trim() == rev_line) {
        return Err(format!("missing {rev_line} in bootstrap script"));
    }
    if !script_text.lines().any(|l| l.trim() == epoch_line) {
        return Err(format!("missing {epoch_line} in bootstrap script"));
    }
    if !script_text.lines().any(|l| l.trim() == window_line) {
        return Err(format!("missing {window_line} in bootstrap script"));
    }
    if !script_text.lines().any(|l| l.trim() == reader_min_line) {
        return Err(format!("missing {reader_min_line} in bootstrap script"));
    }
    Ok(())
}

pub fn validate_bootstrap_coordinate(
    url_or_path: &str,
    expected_lane: &str,
    expected_version: &str,
) -> Result<(), String> {
    let clean = url_or_path.trim_end_matches('/');
    let expected_filename = format!("solstone-journal-{expected_version}-install.sh");
    let expected_suffix =
        format!("solstone-journal/{expected_lane}/{expected_version}/{expected_filename}");

    if clean.ends_with("/install.sh") {
        return Err(format!(
            "mutable lane-root or origin-root install.sh coordinate rejected: {url_or_path}"
        ));
    }
    if !clean.ends_with(&expected_suffix) {
        return Err(format!(
            "bootstrap coordinate must match pattern .../{expected_suffix}, got: {url_or_path}"
        ));
    }
    Ok(())
}

pub fn validate_companion_agreement(
    release_pairs: &[(String, String)],
    companion_json: Option<&str>,
    artifact_manifest_verified: bool,
) -> Result<(), String> {
    let release_map = release_pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();

    match companion_json {
        None => Ok(()),
        Some(json_str) => {
            if !artifact_manifest_verified {
                return Err(
                    "unsigned rust companion manifest present without verified signed artifact manifest"
                        .to_owned(),
                );
            }
            let value: serde_json::Value = serde_json::from_str(json_str)
                .map_err(|e| format!("invalid companion manifest json: {e}"))?;

            let check_field_u64 = |field_name: &str| -> Result<(), String> {
                let release_val = release_map
                    .get(field_name)
                    .ok_or_else(|| format!("missing field {field_name} in .release"))?;
                let json_val = value
                    .get(field_name)
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        format!("missing or non-integer field {field_name} in companion manifest")
                    })?;
                if release_val.parse::<u64>().ok() != Some(json_val) {
                    return Err(format!(
                        "field {field_name} mismatch: .release has {release_val}, companion manifest has {json_val}"
                    ));
                }
                Ok(())
            };

            let check_field_str = |field_name: &str| -> Result<(), String> {
                let release_val = release_map
                    .get(field_name)
                    .ok_or_else(|| format!("missing field {field_name} in .release"))?;
                let json_val = value
                    .get(field_name)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        format!("missing or non-string field {field_name} in companion manifest")
                    })?;
                if *release_val != json_val {
                    return Err(format!(
                        "field {field_name} mismatch: .release has {release_val}, companion manifest has {json_val}"
                    ));
                }
                Ok(())
            };

            check_field_u64("bootstrap_contract_version")?;
            check_field_str("bootstrap_filename")?;
            check_field_str("upgrade_epoch")?;
            check_field_u64("retention_window")?;
            check_field_str("state_reader_min")?;
            check_field_str("state_reader_max")?;

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ArchiveChainDigests, ReleaseInfo, write_sidecars};
    use std::collections::BTreeMap;
    use std::fs;

    use crate::digest::sha256_hex;
    use crate::inventory::{checksum_members_for_os, manifest_members_for_os};

    const CHAIN_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn macos_sidecars_bind_the_signing_receipt_before_manifest_write() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary directory");
        let out = temporary.path();
        let basename = "solstone-journal-1.2.3-darwin-aarch64";
        fs::write(out.join(format!("{basename}.tar.gz")), b"tar bytes").expect("tar");
        fs::write(out.join(format!("{basename}.pkg")), b"pkg bytes").expect("pkg");
        let release = ReleaseInfo {
            product: "solstone-journal",
            version: "1.2.3",
            target: "macos-arm64",
            commit: "commit",
            lock_sha256: "lock",
            archive_chain: Some(ArchiveChainDigests {
                prebuild_input_sha256: CHAIN_DIGEST,
                delivery_contract_sha256: CHAIN_DIGEST,
                final_invocation_sha256: CHAIN_DIGEST,
            }),
        };

        assert!(write_sidecars(out, "macos", &release, basename).is_err());
        fs::write(
            out.join(format!("{basename}.signing.json")),
            b"{\"os\":\"macos\",\"receipt\":\"constructed\"}\n",
        )
        .expect("receipt");
        write_sidecars(out, "macos", &release, basename).expect("sidecars");
        let release_text =
            fs::read_to_string(out.join(format!("{basename}.release"))).expect("release sidecar");
        assert!(release_text.contains("archive_prebuild_input_sha256="));
        assert!(release_text.contains("archive_delivery_contract_sha256="));
        assert!(release_text.contains("archive_final_invocation_sha256="));

        let checksums =
            fs::read_to_string(out.join(format!("{basename}.sha256"))).expect("checksum sidecar");
        let checksum_members = checksums
            .lines()
            .map(|line| {
                let (digest, name) = line.split_once("  ").expect("checksum line");
                (name.to_owned(), digest.to_owned())
            })
            .collect::<BTreeMap<_, _>>();
        let mut expected_checksum = checksum_members_for_os("macos", basename).expect("macos");
        expected_checksum.sort();
        assert_eq!(
            checksum_members.keys().cloned().collect::<Vec<_>>(),
            expected_checksum
        );
        for (name, digest) in &checksum_members {
            assert_eq!(
                digest,
                &sha256_hex(&fs::read(out.join(name)).expect("member"))
            );
        }

        let manifest = fs::read(out.join(format!("{basename}.manifest.json"))).expect("manifest");
        let files = serde_json::from_slice::<serde_json::Value>(&manifest)
            .expect("manifest json")
            .get("files")
            .and_then(serde_json::Value::as_object)
            .expect("files")
            .iter()
            .map(|(name, digest)| (name.clone(), digest.as_str().expect("digest").to_owned()))
            .collect::<BTreeMap<_, _>>();
        let mut expected_manifest = manifest_members_for_os("macos", basename).expect("macos");
        expected_manifest.sort();
        assert_eq!(files.keys().cloned().collect::<Vec<_>>(), expected_manifest);
        for (name, digest) in &files {
            assert_eq!(
                digest,
                &sha256_hex(&fs::read(out.join(name)).expect("member"))
            );
        }
        assert!(!files.contains_key(&format!("{basename}.manifest.json")));
        assert!(!files.contains_key(&format!("{basename}.manifest.json.minisig")));
    }

    #[test]
    fn write_sidecars_refuses_windows() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary directory");
        let out = temporary.path();
        let basename = "solstone-journal-1.0.22-windows-x86_64";
        let release = ReleaseInfo {
            product: "solstone-journal",
            version: "1.0.22",
            target: "windows-x86_64",
            commit: "commit",
            lock_sha256: "lock",
            archive_chain: None,
        };
        let error = write_sidecars(out, "windows", &release, basename).expect_err("windows");
        assert!(
            error
                .to_string()
                .contains("windows archive/signing is not implemented in this lode"),
            "{error}"
        );
        assert!(!out.join(format!("{basename}.release")).exists());
        assert!(!out.join(format!("{basename}.sha256")).exists());
        assert!(!out.join(format!("{basename}.manifest.json")).exists());
    }

    #[test]
    fn bootstrap_agreement_cases_11_12() {
        use super::{
            BOOTSTRAP_BYTES, validate_bootstrap_agreement, validate_bootstrap_coordinate,
            validate_companion_agreement,
        };

        // Canonical BOOTSTRAP_BYTES passes
        let canonical_str = std::str::from_utf8(BOOTSTRAP_BYTES).expect("utf8");
        assert!(validate_bootstrap_agreement(canonical_str).is_ok());

        // Byte mismatch / altered content fails
        let altered = format!("{canonical_str}\n# extra line");
        assert!(validate_bootstrap_agreement(&altered).is_err());

        // Coordinate checks:
        // Valid canonical pattern passes
        assert!(
            validate_bootstrap_coordinate(
                "https://updates.solstone.app/solstone-journal/release/1.0.22/solstone-journal-1.0.22-install.sh",
                "release",
                "1.0.22"
            )
            .is_ok()
        );
        // Mutable lane-root or origin-root fails
        assert!(
            validate_bootstrap_coordinate(
                "https://updates.solstone.app/solstone-journal/release/install.sh",
                "release",
                "1.0.22"
            )
            .is_err()
        );
        assert!(
            validate_bootstrap_coordinate(
                "https://updates.solstone.app/install.sh",
                "release",
                "1.0.22"
            )
            .is_err()
        );
        // Lane / version mismatch fails
        assert!(
            validate_bootstrap_coordinate(
                "https://updates.solstone.app/solstone-journal/staging/1.0.22/solstone-journal-1.0.22-install.sh",
                "release",
                "1.0.22"
            )
            .is_err()
        );
        assert!(
            validate_bootstrap_coordinate(
                "https://updates.solstone.app/solstone-journal/release/1.0.23/solstone-journal-1.0.23-install.sh",
                "release",
                "1.0.22"
            )
            .is_err()
        );

        // Companion agreement:
        let release_pairs = vec![
            ("bootstrap_contract_version".to_owned(), "2".to_owned()),
            (
                "bootstrap_filename".to_owned(),
                "solstone-journal-1.0.22-install.sh".to_owned(),
            ),
            ("upgrade_epoch".to_owned(), "journal-v2".to_owned()),
            ("retention_window".to_owned(), "3".to_owned()),
            ("state_reader_min".to_owned(), "1.0.0".to_owned()),
            ("state_reader_max".to_owned(), "1.0.22".to_owned()),
        ];
        let valid_companion = r#"{
            "bootstrap_contract_version": 2,
            "bootstrap_filename": "solstone-journal-1.0.22-install.sh",
            "upgrade_epoch": "journal-v2",
            "retention_window": 3,
            "state_reader_min": "1.0.0",
            "state_reader_max": "1.0.22"
        }"#;
        // Unsigned companion present without verified artifact manifest fails
        assert!(
            validate_companion_agreement(&release_pairs, Some(valid_companion), false).is_err()
        );
        // Unsigned companion present with verified artifact manifest passes
        assert!(validate_companion_agreement(&release_pairs, Some(valid_companion), true).is_ok());

        // Field mismatch fails
        let mismatch_companion = r#"{
            "bootstrap_contract_version": 1,
            "bootstrap_filename": "solstone-journal-1.0.22-install.sh",
            "upgrade_epoch": "journal-v2",
            "retention_window": 3,
            "state_reader_min": "1.0.0",
            "state_reader_max": "1.0.22"
        }"#;
        assert!(
            validate_companion_agreement(&release_pairs, Some(mismatch_companion), true).is_err()
        );

        // Synthetic derive_bootstrap_contract_version cases:
        use super::derive_bootstrap_contract_version;
        // Comment-only
        assert!(derive_bootstrap_contract_version("# BOOTSTRAP_CONTRACT_VERSION=2\n").is_err());
        // Duplicate
        assert!(
            derive_bootstrap_contract_version(
                "BOOTSTRAP_CONTRACT_VERSION=2\nBOOTSTRAP_CONTRACT_VERSION=2\n"
            )
            .is_err()
        );
        // v1 (value!=2)
        assert!(derive_bootstrap_contract_version("BOOTSTRAP_CONTRACT_VERSION=1\n").is_err());
        // Missing
        assert!(derive_bootstrap_contract_version("SOME_OTHER_VAR=1\n").is_err());
        // Valid
        assert_eq!(
            derive_bootstrap_contract_version("BOOTSTRAP_CONTRACT_VERSION=2\n").unwrap(),
            2
        );
    }
}
