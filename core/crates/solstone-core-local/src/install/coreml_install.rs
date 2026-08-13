// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Atomic installation and verification of the Parakeet CoreML model tree.

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_assets::{Artifact, Platform, catalog};
use solstone_core_journal_config::{
    JournalConfigRead,
    parakeet_coreml::{
        ParakeetCoremlSentinel, parakeet_coreml_cache_dir, parakeet_coreml_model_root,
        parakeet_coreml_sentinel_path, read_valid_parakeet_coreml_sentinel,
    },
};
use thiserror::Error;

#[cfg(test)]
use super::publish_staged_tree_with;
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
    if rows.len() != 23 {
        return Err(CoremlInstallError::new(
            "artifact_registry_mismatch",
            format!("expected 23 {UNIT} catalog rows, found {}", rows.len()),
            65,
        ));
    }
    Ok(rows)
}

fn normalize_os(os_name: &str) -> &str {
    if os_name == "macos" {
        "darwin"
    } else {
        os_name
    }
}

/// Rust spells Apple Silicon `aarch64`; every platform string this installer
/// compares against, and the one `install-models` resolves its variant from,
/// spells it `arm64`. Normalizing only the OS and not the arch is why a real
/// Apple Silicon host refused its own supported platform.
fn normalize_arch(arch: &str) -> &str {
    if arch == "aarch64" { "arm64" } else { arch }
}

/// Split from `current_platform` so the normalization can be tested over the
/// raw values a host actually reports. A test that composes the normalizers
/// itself proves they are correct and says nothing about whether the caller
/// uses them -- which is precisely how the arch half stayed unnormalized.
fn platform_from(os_name: &'static str, arch: &'static str) -> (&'static str, &'static str) {
    (normalize_os(os_name), normalize_arch(arch))
}

fn current_platform() -> (&'static str, &'static str) {
    platform_from(std::env::consts::OS, std::env::consts::ARCH)
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

fn archive_error(error: archive::ArchiveError) -> CoremlInstallError {
    let reason_code = match error {
        archive::ArchiveError::HostRefused { .. } => "download_host_refused",
        archive::ArchiveError::InsecureScheme { .. } => "download_insecure_scheme",
        archive::ArchiveError::UrlUserinfoRefused { .. } => "download_url_userinfo_refused",
        archive::ArchiveError::SizeMismatch { .. } => "download_size_mismatch",
        archive::ArchiveError::DigestMismatch => "download_digest_mismatch",
        archive::ArchiveError::RedirectHopLimitExceeded { .. } => "download_redirect_hop_limit",
        archive::ArchiveError::OriginUnavailable { .. } => "download_origin_unreachable",
        archive::ArchiveError::Io(_) | archive::ArchiveError::Download(_) => "download_failed",
        archive::ArchiveError::PathEscape(_) => "download_failed",
    };
    CoremlInstallError::new(reason_code, error.to_string(), 74)
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
    let (os_name, arch) = current_platform();
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
    let (os_name, arch) = current_platform();
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

#[allow(clippy::too_many_arguments)] // Test-only publish and sentinel seams exercise ordering.
fn install_with_rows_and_seams(
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

#[cfg(test)]
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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

    fn server(bytes: Vec<u8>, requests: usize) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .unwrap();
                stream.write_all(&bytes).unwrap();
            }
        });
        (format!("http://{address}"), handle)
    }

    fn response_server(responses: Vec<String>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request).unwrap();
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}"), handle)
    }

    fn policy(base: &str) -> archive::DownloadHostPolicy<'_> {
        archive::DownloadHostPolicy {
            allowed_hosts: &["127.0.0.1"],
            allow_http: true,
            origin_base_url: base,
        }
    }

    #[test]
    fn install_uses_configured_tree_but_default_sentinel_and_writes_atomically() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let configured = temporary.path().join("override/cache");
        let config = config(&configured);
        let bytes = b"model";
        let artifact = row("Encoder.mlmodelc/weights/weight.bin", bytes);
        let rows = [&artifact];
        let (base, server) = server(bytes.to_vec(), 1);
        let policy = archive::DownloadHostPolicy {
            allowed_hosts: &["127.0.0.1"],
            allow_http: true,
            origin_base_url: &base,
        };

        let target =
            install_with_rows(&home, &config, false, &policy, ("darwin", "arm64"), &rows).unwrap();
        server.join().unwrap();
        assert_eq!(
            target,
            configured.parent().unwrap().join("parakeet-tdt-0.6b-v3")
        );
        assert!(target.join(artifact.filename).is_file());
        let sentinel = parakeet_coreml_sentinel_path(&home);
        assert!(sentinel.is_file());
        assert!(
            !configured
                .parent()
                .unwrap()
                .join(".install-complete")
                .exists()
        );
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(sentinel).unwrap()).unwrap();
        assert_eq!(record["cache_dir"], configured.display().to_string());
        assert!(check_with_rows(&home, "darwin", "arm64", &rows).is_ok());
    }

    #[test]
    fn sentinel_write_failure_after_publish_leaves_no_sentinel() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let configured = temporary.path().join("cache");
        let config = config(&configured);
        let bytes = b"model";
        let artifact = row("Encoder.mlmodelc/weights/weight.bin", bytes);
        let rows = [&artifact];
        let (base, server) = server(bytes.to_vec(), 1);
        let policy = archive::DownloadHostPolicy {
            allowed_hosts: &["127.0.0.1"],
            allow_http: true,
            origin_base_url: &base,
        };
        let mut publish = |staging: &Path, target: &Path| publish_staged_tree(staging, target);
        let error = install_with_rows_and_seams(
            &home,
            &config,
            false,
            &policy,
            ("darwin", "arm64"),
            &rows,
            &mut publish,
            &mut |_, _| Err(std::io::Error::other("injected sentinel failure")),
        )
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.reason_code, "sentinel_write_failed");
        assert!(
            parakeet_coreml_model_root(&configured)
                .join(artifact.filename)
                .is_file()
        );
        assert!(!parakeet_coreml_sentinel_path(&home).exists());
    }

    #[test]
    fn check_complete_install_succeeds_without_requests() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let configured = temporary.path().join("cache");
        let config = config(&configured);
        let bytes = b"model";
        let artifact = row("Encoder.mlmodelc/weights/weight.bin", bytes);
        let rows = [&artifact];
        let (base, server) = server(bytes.to_vec(), 1);
        let download_policy = policy(&base);
        install_with_rows(
            &home,
            &config,
            false,
            &download_policy,
            ("darwin", "arm64"),
            &rows,
        )
        .unwrap();
        server.join().unwrap();

        check_with_rows(&home, "darwin", "arm64", &rows).unwrap();
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
            origin_base_url: "http://localhost:9",
        };
        let error = install_with_rows(&home, &config, false, &denied, ("darwin", "arm64"), &rows)
            .unwrap_err();
        assert_eq!(error.reason_code, "download_host_refused");
        assert!(!parakeet_coreml_model_root(&parakeet_coreml_cache_dir(&config, &home)).exists());
        assert!(!parakeet_coreml_sentinel_path(&home).exists());
    }

    #[test]
    fn install_refuses_a_foreign_redirect_hop_without_writing() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let config = config(&temporary.path().join("cache"));
        let artifact = row("model.mil", b"model");
        let rows = [&artifact];
        let (base, server) = response_server(vec![
            "HTTP/1.1 302 Found\r\nLocation: http://localhost:9/foreign\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
        ]);
        let download_policy = policy(&base);
        let error = install_with_rows(
            &home,
            &config,
            false,
            &download_policy,
            ("darwin", "arm64"),
            &rows,
        )
        .unwrap_err();
        server.join().unwrap();
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
    fn failed_download_preserves_a_preexisting_complete_tree_and_sentinel() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let configured = temporary.path().join("cache");
        let config = config(&configured);
        let first = row("one", b"old one");
        let second = row("two", b"old two");
        let rows = [&first, &second];
        let target = parakeet_coreml_model_root(&configured);
        for row in rows {
            let path = target.join(row.filename);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                if row.filename == "one" {
                    b"old one"
                } else {
                    b"old two"
                },
            )
            .unwrap();
        }
        fs::create_dir_all(&configured).unwrap();
        let record =
            ParakeetCoremlSentinel::new(configured.clone(), "darwin", "arm64", FLUIDAUDIO_VERSION);
        write_sentinel(&parakeet_coreml_sentinel_path(&home), &record).unwrap();
        let (base, server) = response_server(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nnew one".to_owned(),
        ]);
        let download_policy = policy(&base);
        let error = install_with_rows(
            &home,
            &config,
            true,
            &download_policy,
            ("darwin", "arm64"),
            &rows,
        )
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.reason_code, "download_digest_mismatch");
        assert_eq!(fs::read(target.join("one")).unwrap(), b"old one");
        assert!(parakeet_coreml_sentinel_path(&home).is_file());
        check_with_rows(&home, "darwin", "arm64", &rows).unwrap();
    }

    #[test]
    fn interrupted_publish_leaves_no_partial_tree() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let configured = temporary.path().join("cache");
        let config = config(&configured);
        let artifact = row("model.mil", b"model");
        let rows = [&artifact];
        let (base, server) = server(b"model".to_vec(), 1);
        let download_policy = policy(&base);
        let mut publish = |staging: &Path, target: &Path| {
            publish_staged_tree_with(staging, target, &mut |_, _| {
                Err(std::io::Error::other("interrupted publish"))
            })
        };
        let mut write = write_sentinel;
        let error = install_with_rows_and_seams(
            &home,
            &config,
            false,
            &download_policy,
            ("darwin", "arm64"),
            &rows,
            &mut publish,
            &mut write,
        )
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.reason_code, "publish_failed");
        assert!(!parakeet_coreml_model_root(&configured).exists());
        assert!(!parakeet_coreml_sentinel_path(&home).exists());
    }

    #[test]
    fn force_reinstalls_an_incomplete_tree_and_verifies_it() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let configured = temporary.path().join("cache");
        let config = config(&configured);
        let artifact = row("model.mil", b"model");
        let rows = [&artifact];
        fs::create_dir_all(parakeet_coreml_model_root(&configured)).unwrap();
        let (base, server) = server(b"model".to_vec(), 1);
        let download_policy = policy(&base);
        install_with_rows(
            &home,
            &config,
            true,
            &download_policy,
            ("darwin", "arm64"),
            &rows,
        )
        .unwrap();
        server.join().unwrap();
        check_with_rows(&home, "darwin", "arm64", &rows).unwrap();
    }

    #[test]
    fn fluidaudio_version_matches_the_helper_package_declaration() {
        let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../solstone/observe/transcribe/parakeet_helper/Package.swift");
        let contents = fs::read_to_string(&package)
            .unwrap_or_else(|error| panic!("read {}: {error}", package.display()));
        let version = contents
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("exact: \"")
                    .and_then(|value| value.strip_suffix('\"'))
            })
            .unwrap_or_else(|| panic!("parse exact FluidAudio version from {}", package.display()));
        assert_eq!(FLUIDAUDIO_VERSION, version);
    }

    /// The spellings `std::env::consts` actually produces on the hosts this
    /// installer supports must be accepted by the guard, and the arch half is
    /// the one that was wrong: every unit test injected an already-normalized
    /// `("darwin", "arm64")`, so the assertions were right and the input was
    /// not, and a real Apple Silicon host refused itself with
    /// `platform_unsupported`. Compose the same normalizers `current_platform`
    /// uses over the raw values Rust reports, so this stays host-independent.
    #[test]
    fn the_raw_host_spellings_rust_reports_are_accepted_by_the_guard() {
        for (raw_os, raw_arch) in [("macos", "aarch64"), ("macos", "arm64")] {
            let (os_name, arch) = platform_from(raw_os, raw_arch);
            assert_eq!(
                (os_name, arch),
                ("darwin", "arm64"),
                "{raw_os}/{raw_arch} must normalize to the spelling the guard compares against"
            );
            require_coreml_host(os_name, arch).unwrap_or_else(|error| {
                panic!("raw host {raw_os}/{raw_arch} must be a supported CoreML host: {error:?}")
            });
        }
    }

    #[test]
    fn a_genuinely_unsupported_host_is_still_refused() {
        let (os_name, arch) = platform_from("linux", "x86_64");
        let error =
            require_coreml_host(os_name, arch).expect_err("linux/x86_64 is not a CoreML host");
        assert_eq!(error.reason_code, "platform_unsupported");
    }
}
