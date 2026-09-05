// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Atomic installation and verification of the Parakeet CoreML model tree.

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_assets::{Artifact, Platform, canonical_host_pair, catalog};
use solstone_core_journal_config::{
    JournalConfigRead,
    parakeet_coreml::{
        ParakeetCoremlSentinel, parakeet_coreml_cache_dir, parakeet_coreml_model_root,
        parakeet_coreml_sentinel_path, read_valid_parakeet_coreml_sentinel,
    },
};
use thiserror::Error;

use super::{archive, publish_staged_tree};

const UNIT: &str = "parakeet-coreml";
const FLUIDAUDIO_VERSION: &str = "0.14.0";

/// Printed by `install-models` before it asks this installer to fetch assets.
pub const PARAKEET_COREML_DOWNLOAD_DISCLOSURE: &str = "parakeet assets: downloading the parakeet-tdt-0.6b-v3 Core ML model (CC-BY-4.0) from updates.solstone.app. see THIRD_PARTY_NOTICES.md.";

#[derive(Debug, Error)]
#[error("{message}")]
pub struct CoremlInstallError {
    pub reason_code: String,
    message: String,
    pub exit_code: u8,
}

impl CoremlInstallError {
    fn new(reason_code: impl Into<String>, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            reason_code: reason_code.into(),
            message: message.into(),
            exit_code,
        }
    }
}

fn rows() -> Result<Vec<&'static Artifact>, CoremlInstallError> {
    let rows = catalog()
        .iter()
        .filter(|artifact| artifact.unit == UNIT && artifact.platform == Some(Platform::MacosArm64))
        .collect::<Vec<_>>();
    Ok(rows)
}

fn require_coreml_host(os_name: &str, arch: &str) -> Result<(), CoremlInstallError> {
    if os_name == "darwin" && arch == "arm64" {
        Ok(())
    } else {
        Err(CoremlInstallError::new(
            "platform_unsupported",
            format!("parakeet CoreML is only supported on darwin/arm64, not {os_name}/{arch}"),
            69,
        ))
    }
}

fn archive_error_reason_code(error: &archive::ArchiveError) -> &'static str {
    match error {
        archive::ArchiveError::HostRefused { .. } => "download_host_refused",
        archive::ArchiveError::InsecureScheme { .. } => "download_insecure_scheme",
        archive::ArchiveError::UrlUserinfoRefused { .. } => "download_url_userinfo_refused",
        archive::ArchiveError::SizeMismatch { .. } => "download_size_mismatch",
        archive::ArchiveError::DigestMismatch { .. } => "download_digest_mismatch",
        archive::ArchiveError::RedirectHopLimitExceeded { .. } => "download_redirect_hop_limit",
        archive::ArchiveError::OriginUnavailable { .. } => "download_origin_unreachable",
        archive::ArchiveError::Io(_) | archive::ArchiveError::Download(_) => "download_failed",
        archive::ArchiveError::PathEscape(_) => "download_failed",
    }
}

fn archive_error(error: archive::ArchiveError) -> CoremlInstallError {
    CoremlInstallError::new(archive_error_reason_code(&error), error.to_string(), 74)
}

fn write_sentinel(path: &Path, sentinel: &ParakeetCoremlSentinel) -> std::io::Result<()> {
    let parent = path.parent().expect("sentinel has a parent");
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".install-complete.tmp{}",
        uuid::Uuid::new_v4().simple()
    ));
    let bytes = serde_json::to_vec(sentinel).expect("static CoreML sentinel serializes");
    let result = fs::write(&temporary, bytes).and_then(|_| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Install the catalogued CoreML tree using the production origin policy.
pub fn install_parakeet_coreml_model(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
) -> Result<PathBuf, CoremlInstallError> {
    let (os_name, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    install_parakeet_coreml_model_with_policy(
        home_dir,
        config,
        force,
        &archive::PRODUCTION_DOWNLOAD_POLICY,
        os_name,
        arch,
    )
}

/// Verify the model tree and install sentinel without fetching.
pub fn check_parakeet_coreml_install(
    home_dir: &Path,
    _config: &JournalConfigRead,
) -> Result<(), CoremlInstallError> {
    let (os_name, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    check_parakeet_coreml_install_with_platform(home_dir, os_name, arch)
}

fn install_parakeet_coreml_model_with_policy(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
    os_name: &str,
    arch: &str,
) -> Result<PathBuf, CoremlInstallError> {
    let rows = rows()?;
    install_with_rows(home_dir, config, force, policy, (os_name, arch), &rows)
}

fn install_with_rows(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
    platform: (&str, &str),
    rows: &[&Artifact],
) -> Result<PathBuf, CoremlInstallError> {
    let mut publish = |staging: &Path, target: &Path| publish_staged_tree(staging, target);
    let mut write = write_sentinel;
    install_with_rows_and_seams(
        home_dir,
        config,
        force,
        policy,
        platform,
        rows,
        &mut publish,
        &mut write,
    )
}

/// Internal fixture seam: run the CoreML installer with injected publish and sentinel writers.
#[allow(clippy::too_many_arguments)] // Test-only publish and sentinel seams exercise ordering.
pub(crate) fn install_with_rows_and_seams(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
    platform: (&str, &str),
    rows: &[&Artifact],
    publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
    write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
) -> Result<PathBuf, CoremlInstallError> {
    let (os_name, arch) = platform;
    require_coreml_host(os_name, arch)?;
    let cache_dir = parakeet_coreml_cache_dir(config, home_dir);
    let target = parakeet_coreml_model_root(&cache_dir);
    if !force && check_with_rows(home_dir, os_name, arch, rows).is_ok() {
        return Ok(target);
    }

    let parent = target.parent().expect("CoreML model root has a parent");
    fs::create_dir_all(parent)
        .map_err(|error| CoremlInstallError::new("install_failed", error.to_string(), 74))?;
    let staging = parent.join(format!(
        ".parakeet-tdt-0.6b-v3.stage.{}",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        fs::create_dir(&staging)
            .map_err(|error| CoremlInstallError::new("install_failed", error.to_string(), 74))?;
        for row in rows {
            archive::download_verified(row, &staging.join(row.filename), policy, |_, _| {})
                .map_err(archive_error)?;
        }
        publish(&staging, &target)
            .map_err(|error| CoremlInstallError::new("publish_failed", error.to_string(), 74))?;

        fs::create_dir_all(&cache_dir)
            .map_err(|error| CoremlInstallError::new("install_failed", error.to_string(), 74))?;
        let path = parakeet_coreml_sentinel_path(home_dir);
        let _ = fs::remove_file(&path);
        let record =
            ParakeetCoremlSentinel::new(cache_dir.clone(), os_name, arch, FLUIDAUDIO_VERSION);
        write(&path, &record).map_err(|error| {
            CoremlInstallError::new("sentinel_write_failed", error.to_string(), 74)
        })?;
        Ok(target.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn check_parakeet_coreml_install_with_platform(
    home_dir: &Path,
    os_name: &str,
    arch: &str,
) -> Result<(), CoremlInstallError> {
    let rows = rows()?;
    check_with_rows(home_dir, os_name, arch, &rows)
}

fn check_with_rows(
    home_dir: &Path,
    os_name: &str,
    arch: &str,
    rows: &[&Artifact],
) -> Result<(), CoremlInstallError> {
    require_coreml_host(os_name, arch)?;
    let sentinel =
        read_valid_parakeet_coreml_sentinel(home_dir, os_name, arch).ok_or_else(|| {
            CoremlInstallError::new(
                "sentinel_not_ready",
                format!(
                    "parakeet CoreML sentinel not ready: {}; install it with: journal install-models",
                    parakeet_coreml_sentinel_path(home_dir).display(),
                ),
                65,
            )
        })?;
    let model_root = parakeet_coreml_model_root(sentinel.cache_dir());
    for row in rows {
        if !model_root.join(row.filename).is_file() {
            return Err(CoremlInstallError::new(
                "model_incomplete",
                format!(
                    "parakeet CoreML asset missing: {}; install it with: journal install-models",
                    row.filename
                ),
                65,
            ));
        }
    }
    Ok(())
}

/// Internal fixture seam: install rows on the darwin/arm64 host the production guard accepts.
#[cfg(feature = "test-hooks")]
pub(crate) fn install_with_rows_for_test(
    home_dir: &Path,
    config: &JournalConfigRead,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
    rows: &[&Artifact],
) -> Result<PathBuf, CoremlInstallError> {
    install_with_rows(home_dir, config, force, policy, ("darwin", "arm64"), rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn coreml_catalog_rows_match_the_filtered_catalog_set() {
        let rows = rows().unwrap();
        let expected = catalog()
            .iter()
            .filter(|artifact| {
                artifact.unit == UNIT && artifact.platform == Some(Platform::MacosArm64)
            })
            .map(|artifact| artifact.filename)
            .collect::<std::collections::BTreeSet<_>>();
        let actual = rows
            .iter()
            .map(|artifact| artifact.filename)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!expected.is_empty());
        assert_eq!(actual, expected);
    }

    #[test]
    fn coreml_archive_error_reason_codes_cover_every_archive_error() {
        use archive::ArchiveError;
        assert_eq!(
            archive_error_reason_code(&ArchiveError::HostRefused {
                host: "blocked.test".to_owned()
            }),
            "download_host_refused"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::InsecureScheme {
                scheme: "http".to_owned(),
                host: "example.test".to_owned()
            }),
            "download_insecure_scheme"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::UrlUserinfoRefused {
                authority: "user@host".to_owned()
            }),
            "download_url_userinfo_refused"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::SizeMismatch {
                expected: 1,
                actual: 2
            }),
            "download_size_mismatch"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::DigestMismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
            }),
            "download_digest_mismatch"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::RedirectHopLimitExceeded { limit: 5 }),
            "download_redirect_hop_limit"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::OriginUnavailable {
                host: "origin.test".to_owned(),
                message: "refused".to_owned()
            }),
            "download_origin_unreachable"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::Io(std::io::Error::other("io"))),
            "download_failed"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::Download("failed".to_owned())),
            "download_failed"
        );
        assert_eq!(
            archive_error_reason_code(&ArchiveError::PathEscape("..".to_owned())),
            "download_failed"
        );
    }

    fn config(cache_dir: &Path) -> JournalConfigRead {
        let config = serde_json::json!({"transcribe": {"parakeet": {"cache_dir": cache_dir}}})
            .as_object()
            .unwrap()
            .clone();
        JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(config),
        }
    }

    fn row(filename: &'static str, bytes: &[u8]) -> Artifact {
        Artifact {
            unit: UNIT,
            version: "test",
            filename,
            sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
            size_bytes: bytes.len() as u64,
            upstream_url: "https://upstream.invalid/test",
            origin_key: Box::leak(format!("test/{filename}").into_boxed_str()),
            artifact_key: None,
            platform: Some(Platform::MacosArm64),
            backend: None,
            extracted_binary_sha256: None,
        }
    }

    #[test]
    fn check_rejects_a_sentinel_for_a_removed_cache_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let missing = temporary.path().join("removed/cache");
        let record = ParakeetCoremlSentinel::new(missing, "darwin", "arm64", FLUIDAUDIO_VERSION);
        let path = parakeet_coreml_sentinel_path(&home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_sentinel(&path, &record).unwrap();

        let error =
            check_parakeet_coreml_install_with_platform(&home, "darwin", "arm64").unwrap_err();
        assert_eq!(error.reason_code, "sentinel_not_ready");
    }

    #[test]
    fn install_refuses_a_non_solstone_origin_without_writing() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let config = config(&temporary.path().join("cache"));
        let artifact = row("model.mil", b"model");
        let rows = [&artifact];
        let denied = archive::DownloadHostPolicy {
            allowed_hosts: &["updates.solstone.app"],
            allow_http: true,
            origin_base_url: "http://[::1]:9",
        };
        let error = install_with_rows(&home, &config, false, &denied, ("darwin", "arm64"), &rows)
            .unwrap_err();
        assert_eq!(error.reason_code, "download_host_refused");
        assert!(!parakeet_coreml_model_root(&parakeet_coreml_cache_dir(&config, &home)).exists());
        assert!(!parakeet_coreml_sentinel_path(&home).exists());
    }

    #[test]
    fn production_policy_is_used_for_the_public_installer_path() {
        // The public entry point passes this exact static policy. Inspecting it
        // avoids a network request while binding the production authority.
        assert_eq!(
            archive::PRODUCTION_DOWNLOAD_POLICY.origin_base_url,
            "https://updates.solstone.app"
        );
        assert_eq!(
            archive::PRODUCTION_DOWNLOAD_POLICY.allowed_hosts,
            &["updates.solstone.app"]
        );
        const { assert!(!archive::PRODUCTION_DOWNLOAD_POLICY.allow_http) };
    }

    #[test]
    fn check_empty_host_names_the_install_command_without_requests() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let artifact = row("model.mil", b"model");
        let rows = [&artifact];
        let error = check_with_rows(&home, "darwin", "arm64", &rows).unwrap_err();
        assert_eq!(error.reason_code, "sentinel_not_ready");
        assert!(error.to_string().contains("journal install-models"));
    }

    #[test]
    fn the_canonical_coreml_host_is_accepted_by_the_guard() {
        require_coreml_host("darwin", "arm64").unwrap();
    }

    /// Pins that the owner's output is the spelling `require_coreml_host` accepts.
    /// It does not prove `install_parakeet_coreml_model` / `check_parakeet_coreml_install`
    /// call the owner: those read `env::consts`, a compile-time constant, so the
    /// macos/aarch64 hop cannot be exercised from a linux host.
    #[test]
    fn owner_output_for_macos_aarch64_is_accepted_by_the_guard() {
        let (os_name, arch) = canonical_host_pair("macos", "aarch64");
        require_coreml_host(os_name, arch).unwrap();
    }

    #[test]
    fn a_genuinely_unsupported_host_is_still_refused() {
        let error =
            require_coreml_host("linux", "x86_64").expect_err("linux/x86_64 is not a CoreML host");
        assert_eq!(error.reason_code, "platform_unsupported");
    }
}
