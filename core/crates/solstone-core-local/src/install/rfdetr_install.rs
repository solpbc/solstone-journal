// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Runtime installation for the pinned rf-detr.cpp detector.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_assets::{Artifact, Backend, Platform};
use thiserror::Error;

use super::{archive, download_artifact, ensure_verified, fit_report, select_artifact};

const ENGINE_UNIT: &str = "rfdetr-engine";
const MODEL_UNIT: &str = "rfdetr-model";
const ENGINE_VERSION: &str = "v0.1.0-solpbc.5";
pub const ENGINE_PROVENANCE_REF: &str = "ec73712e";
const BINARY: &str = "rfdetr-cli";
const MODEL_REVISION: &str = "c3dc0c037df499f5503545247df6618415fca643";
const MODEL_FILE: &str = "rfdetr-nano-f16.gguf";
const MODEL_REPO: &str = "mudler/rfdetr-cpp-nano";
const SIDECAR: &str = ".rfdetr-install.json";

#[derive(Debug, Error)]
#[error("{message}")]
pub struct RfdetrInstallError {
    pub reason_code: String,
    message: String,
    pub exit_code: u8,
}
impl RfdetrInstallError {
    pub fn new(reason_code: impl Into<String>, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            reason_code: reason_code.into(),
            message: message.into(),
            exit_code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RfdetrInstallRecord {
    Installed,
    PlatformUnavailable,
}

#[derive(Debug, Serialize, Deserialize)]
struct Sidecar {
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_sha256: Option<String>,
    status: String,
}

pub fn rfdetr_platform_supported(os_name: &str, arch: &str) -> bool {
    rfdetr_artifact_key(os_name, arch).is_some()
}

pub fn rfdetr_artifact_key(os_name: &str, arch: &str) -> Option<&'static str> {
    match (os_name, arch.to_ascii_lowercase().as_str()) {
        ("darwin", "arm64") => Some("macos-metal-arm64"),
        ("linux", "amd64" | "x64" | "x86_64") => Some("linux-cpu-x64"),
        ("linux", "arm64" | "aarch64") => Some("linux-cpu-arm64"),
        _ => None,
    }
}
fn root(journal: &Path) -> PathBuf {
    journal.join("cache/providers/rfdetr")
}
pub fn binary_path(journal: &Path, key: &str) -> PathBuf {
    root(journal)
        .join(ENGINE_VERSION)
        .join("engine")
        .join(key)
        .join(BINARY)
}
pub fn model_path(journal: &Path) -> PathBuf {
    root(journal)
        .join("model")
        .join(MODEL_REVISION)
        .join(MODEL_FILE)
}
fn sidecar_path(journal: &Path) -> PathBuf {
    root(journal).join(SIDECAR)
}
fn extract_path(journal: &Path, key: &str) -> PathBuf {
    binary_path(journal, key).parent().unwrap().join(".extract")
}
fn engine(key: &str) -> Result<&'static Artifact, RfdetrInstallError> {
    let (platform, backend) = match key {
        "linux-cpu-x64" => (Platform::LinuxX64, Backend::Cpu),
        "linux-cpu-arm64" => (Platform::LinuxArm64, Backend::Cpu),
        "macos-metal-arm64" => (Platform::MacosArm64, Backend::Metal),
        _ => {
            return Err(RfdetrInstallError::new(
                "unsupported_platform",
                format!("rf-detr assets unsupported for {key}"),
                69,
            ));
        }
    };
    select_artifact(ENGINE_UNIT, Some(platform), Some(backend), Some(key), None).map_err(|error| {
        RfdetrInstallError::new(
            "artifact_registry_mismatch",
            error
                .envelope
                .error
                .map(|value| value.message)
                .unwrap_or_else(|| "rf-detr engine catalog lookup failed".to_owned()),
            65,
        )
    })
}
fn model() -> Result<&'static Artifact, RfdetrInstallError> {
    select_artifact(MODEL_UNIT, None, None, None, Some(MODEL_FILE)).map_err(|error| {
        RfdetrInstallError::new(
            "artifact_registry_mismatch",
            error
                .envelope
                .error
                .map(|value| value.message)
                .unwrap_or_else(|| "rf-detr model catalog lookup failed".to_owned()),
            65,
        )
    })
}
fn tarball(journal: &Path, key: &str) -> Result<PathBuf, RfdetrInstallError> {
    Ok(tarball_for(journal, key, engine(key)?))
}

fn tarball_for(journal: &Path, key: &str, engine: &Artifact) -> PathBuf {
    binary_path(journal, key)
        .parent()
        .unwrap()
        .join(engine.filename)
}

fn verify(
    path: &Path,
    expected: &str,
    size: Option<u64>,
    label: &str,
) -> Result<(), RfdetrInstallError> {
    let metadata = fs::metadata(path).map_err(|_| {
        RfdetrInstallError::new(
            "file_missing",
            format!("rf-detr asset missing: {label}"),
            65,
        )
    })?;
    if !metadata.is_file() {
        return Err(RfdetrInstallError::new(
            "file_missing",
            format!("rf-detr asset missing: {label}"),
            65,
        ));
    }
    if let Some(size) = size
        && metadata.len() != size
    {
        return Err(RfdetrInstallError::new(
            "size_mismatch",
            format!(
                "size mismatch for {label}: expected {size}, got {}",
                metadata.len()
            ),
            65,
        ));
    }
    archive::verify_sha256(path, expected)
        .map(|_| ())
        .map_err(|_| {
            RfdetrInstallError::new(
                "sha256_mismatch",
                format!("sha256 mismatch for {label}"),
                65,
            )
        })
}
fn installed_sidecar_for(
    key: &str,
    engine: &Artifact,
    model: &Artifact,
) -> Result<Sidecar, RfdetrInstallError> {
    Ok(Sidecar {
        artifact_key: Some(key.to_owned()),
        engine_version: Some(engine.version.to_owned()),
        engine_sha256: Some(engine.sha256.to_owned()),
        model_file: Some(MODEL_FILE.to_owned()),
        model_repo: Some(MODEL_REPO.to_owned()),
        model_revision: Some(MODEL_REVISION.to_owned()),
        model_sha256: Some(model.sha256.to_owned()),
        status: "installed".to_owned(),
    })
}
fn write_sidecar(
    journal: &Path,
    record: &RfdetrInstallRecord,
    artifacts: Option<(&str, &Artifact, &Artifact)>,
) -> Result<(), RfdetrInstallError> {
    let record = match record {
        RfdetrInstallRecord::Installed => {
            let (key, engine, model) = artifacts.ok_or_else(|| {
                RfdetrInstallError::new(
                    "artifact_registry_mismatch",
                    "rf-detr installed sidecar has no resolved artifacts",
                    65,
                )
            })?;
            installed_sidecar_for(key, engine, model)?
        }
        RfdetrInstallRecord::PlatformUnavailable => Sidecar {
            artifact_key: None,
            engine_version: None,
            engine_sha256: None,
            model_file: None,
            model_repo: None,
            model_revision: None,
            model_sha256: None,
            status: "platform_unavailable".to_owned(),
        },
    };
    let path = sidecar_path(journal);
    fs::create_dir_all(path.parent().unwrap())
        .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))?;
    let mut text = serde_json::to_string_pretty(&record).expect("static rf-detr record serializes");
    text.push('\n');
    let tmp = path
        .parent()
        .unwrap()
        .join(format!("tmp{}", uuid::Uuid::new_v4().simple()));
    let result = fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, &path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))
}
fn remove_archive(path: PathBuf) {
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(path.with_file_name(format!(
        "{}.tmp",
        path.file_name().unwrap().to_string_lossy()
    )));
}

fn cleanup(journal: &Path, key: &str, engine: &Artifact, model: &Artifact) {
    let binary = binary_path(journal, key);
    let _ = fs::remove_file(&binary);
    let _ = fs::remove_file(binary.with_file_name(format!("{BINARY}.tmp")));
    if let Ok(path) = tarball(journal, key) {
        remove_archive(path);
    }
    remove_archive(tarball_for(journal, key, engine));
    let _ = fs::remove_dir_all(extract_path(journal, key));
    let model_path = model_path(journal);
    if verify(&model_path, model.sha256, None, MODEL_FILE).is_err() {
        let _ = fs::remove_file(&model_path);
    }
    let _ = fs::remove_file(model_path.with_file_name(format!("{MODEL_FILE}.tmp")));
    let _ = fs::remove_file(sidecar_path(journal));
}
fn find_binary(root: &Path) -> Option<PathBuf> {
    let direct = root.join(BINARY);
    if direct.is_file() {
        return Some(direct);
    }
    for entry in fs::read_dir(root).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary(&path) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|name| name == BINARY) {
            return Some(path);
        }
    }
    None
}

pub fn check_rfdetr_model(
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    let Some(key) = rfdetr_artifact_key(os_name, arch) else {
        return Ok(RfdetrInstallRecord::PlatformUnavailable);
    };
    check_rfdetr_model_with_artifacts(journal, key, engine(key)?, model()?)
}

fn check_rfdetr_model_with_artifacts(
    journal: &Path,
    key: &str,
    engine: &Artifact,
    model: &Artifact,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    let text = fs::read_to_string(sidecar_path(journal)).map_err(|_| {
        RfdetrInstallError::new(
            "sidecar_missing",
            format!(
                "rf-detr sidecar missing: {}",
                sidecar_path(journal).display()
            ),
            65,
        )
    })?;
    let stored: Sidecar = serde_json::from_str(&text).map_err(|error| {
        RfdetrInstallError::new(
            "sidecar_invalid",
            format!(
                "rf-detr sidecar invalid: {}: {error}",
                sidecar_path(journal).display()
            ),
            65,
        )
    })?;
    let expected = installed_sidecar_for(key, engine, model)?;
    if stored.status != expected.status
        || stored.artifact_key != expected.artifact_key
        || stored.engine_version != expected.engine_version
        || stored.engine_sha256 != expected.engine_sha256
        || stored.model_file != expected.model_file
        || stored.model_repo != expected.model_repo
        || stored.model_revision != expected.model_revision
        || stored.model_sha256 != expected.model_sha256
    {
        return Err(RfdetrInstallError::new(
            "sidecar_mismatch",
            "rf-detr sidecar does not match pinned artifacts",
            65,
        ));
    }
    verify(
        &binary_path(journal, key),
        engine.extracted_binary_sha256.expect("catalog digest"),
        None,
        BINARY,
    )?;
    verify(
        &model_path(journal),
        model.sha256,
        Some(model.size_bytes),
        MODEL_FILE,
    )?;
    Ok(RfdetrInstallRecord::Installed)
}
pub fn install_rfdetr(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    install_rfdetr_with_policy(
        journal,
        os_name,
        arch,
        force,
        &archive::PRODUCTION_DOWNLOAD_POLICY,
    )
}
pub(crate) fn install_rfdetr_with_policy(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    install_rfdetr_with_policy_and_report(journal, os_name, arch, force, policy, None, None)
}

/// Internal fixture seam: install caller-supplied rows through the production installer.
#[cfg(feature = "test-hooks")]
pub(crate) fn install_rfdetr_with_artifacts(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
    engine: &Artifact,
    model: &Artifact,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    install_rfdetr_with_policy_and_report(
        journal,
        os_name,
        arch,
        force,
        policy,
        None,
        Some((engine, model)),
    )
}

fn install_rfdetr_with_policy_and_report(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
    report_override: Option<fit_report::FitReport>,
    artifact_override: Option<(&Artifact, &Artifact)>,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    let Some(key) = rfdetr_artifact_key(os_name, arch) else {
        let record = RfdetrInstallRecord::PlatformUnavailable;
        write_sidecar(journal, &record, None)?;
        log::info!("rf-detr.cpp platform unavailable on {os_name}/{arch}");
        return Ok(record);
    };
    let (engine, model) = match artifact_override {
        Some(rows) => rows,
        None => (engine(key)?, model()?),
    };
    let ready = check_rfdetr_model_with_artifacts(journal, key, engine, model);
    if !force && let Ok(record) = ready {
        return Ok(record);
    }
    let report = report_override
        .unwrap_or_else(|| fit_report::build_rfdetr_fit_report(journal, os_name, arch));
    let rendered = fit_report::render_fit_report(&report);
    if report.overall() == fit_report::FitSeverity::Blocked {
        return Err(RfdetrInstallError::new("host_unfit", rendered, 69));
    }
    if report.overall() == fit_report::FitSeverity::Warning {
        log::warn!("rf-detr.cpp host fit warning:\n{rendered}");
    }
    cleanup(journal, key, engine, model);
    let result = (|| {
        let archive_path = tarball_for(journal, key, engine);
        download_artifact(engine, &archive_path, policy, |_, _| {}, "download_failed")
            .map_err(dispatch_error)?;
        let extract = extract_path(journal, key);
        let _ = fs::remove_dir_all(&extract);
        fs::create_dir_all(&extract)
            .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))?;
        archive::extract_tar_gz(&archive_path, &extract).map_err(|error| {
            let path_escape = matches!(error, archive::ArchiveError::PathEscape(_));
            RfdetrInstallError::new(
                if path_escape {
                    "archive_path_traversal"
                } else {
                    "extract_failed"
                },
                error.to_string(),
                if path_escape { 65 } else { 74 },
            )
        })?;
        let found = find_binary(&extract).ok_or_else(|| {
            RfdetrInstallError::new(
                "binary_missing",
                format!("Extracted archive did not contain {BINARY}"),
                65,
            )
        })?;
        verify(
            &found,
            engine.extracted_binary_sha256.expect("catalog digest"),
            None,
            BINARY,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&found)
                .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))?
                .permissions();
            permissions.set_mode(permissions.mode() | 0o111);
            fs::set_permissions(&found, permissions).map_err(|error| {
                RfdetrInstallError::new("install_failed", error.to_string(), 74)
            })?;
        }
        let final_binary = binary_path(journal, key);
        fs::create_dir_all(final_binary.parent().unwrap())
            .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))?;
        fs::rename(&found, &final_binary)
            .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))?;
        let _ = fs::remove_dir_all(&extract);
        let _ = fs::remove_file(&archive_path);
        ensure_verified(
            model,
            &model_path(journal),
            policy,
            |_, _| {},
            "download_failed",
        )
        .map_err(dispatch_error)?;
        let record = RfdetrInstallRecord::Installed;
        write_sidecar(journal, &record, Some((key, engine, model)))?;
        check_rfdetr_model_with_artifacts(journal, key, engine, model)?;
        let legacy_engine = root(journal).join("engine");
        if let Err(error) = fs::remove_dir_all(&legacy_engine)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "rf-detr.cpp could not remove legacy engine layout {}: {error}",
                legacy_engine.display()
            );
        }
        Ok(record)
    })();
    if result.is_err() {
        cleanup(journal, key, engine, model);
    }
    result
}
fn dispatch_error(error: super::DispatchError) -> RfdetrInstallError {
    RfdetrInstallError::new(
        error
            .envelope
            .error
            .as_ref()
            .map(|value| value.reason_code.clone())
            .unwrap_or_else(|| "download_failed".to_owned()),
        error
            .envelope
            .error
            .map(|value| value.message)
            .unwrap_or_else(|| "rf-detr download failed".to_owned()),
        error.exit_code,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::test_support::{leaked_temps, prove_temp_sweep};

    use std::fs::OpenOptions;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    const DENY_ALL_POLICY: archive::DownloadHostPolicy<'static> = archive::DownloadHostPolicy {
        allowed_hosts: &["example.invalid"],
        allow_http: false,
        origin_base_url: "https://updates.solstone.app",
    };
    const TEST_KEY: &str = "linux-cpu-x64";

    #[test]
    fn rfdetr_artifact_keys_match_the_catalog_platforms() {
        assert_eq!(
            rfdetr_artifact_key("darwin", "arm64"),
            Some("macos-metal-arm64")
        );
        assert_eq!(
            rfdetr_artifact_key("linux", "X86_64"),
            Some("linux-cpu-x64")
        );
        assert_eq!(
            rfdetr_artifact_key("linux", "aarch64"),
            Some("linux-cpu-arm64")
        );
        assert_eq!(rfdetr_artifact_key("macos", "aarch64"), None);
    }

    #[test]
    fn unsupported_platform_only_writes_a_sidecar_on_install() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            check_rfdetr_model(temp.path(), "windows", "x86_64").unwrap(),
            RfdetrInstallRecord::PlatformUnavailable
        );
        assert!(!sidecar_path(temp.path()).exists());
        assert_eq!(
            install_rfdetr_with_policy(
                temp.path(),
                "windows",
                "x86_64",
                false,
                &archive::PRODUCTION_DOWNLOAD_POLICY,
            )
            .unwrap(),
            RfdetrInstallRecord::PlatformUnavailable
        );
        assert_eq!(
            fs::read_to_string(sidecar_path(temp.path())).unwrap(),
            "{\n  \"status\": \"platform_unavailable\"\n}\n"
        );
    }

    #[test]
    fn sound_platform_unavailable_sidecar_is_returned_on_a_supported_host() {
        let temp = tempfile::tempdir().unwrap();
        write_sidecar(temp.path(), &RfdetrInstallRecord::PlatformUnavailable, None).unwrap();

        assert_eq!(
            install_rfdetr_with_policy(temp.path(), "linux", "x86_64", false, &DENY_ALL_POLICY,)
                .unwrap_err()
                .reason_code,
            "download_host_refused"
        );
        assert!(!binary_path(temp.path(), TEST_KEY).exists());
        assert!(!model_path(temp.path()).exists());
    }

    #[test]
    fn extracted_binary_digest_is_catalogued_and_rejects_tampering() {
        let temp = tempfile::tempdir().unwrap();
        let engine = engine(TEST_KEY).unwrap();
        assert_eq!(
            engine.extracted_binary_sha256,
            Some("6f225708e4b9dafc39a085f1323bc426ca037b746b3be9c7c571d9be494306af")
        );
        let binary = temp.path().join(BINARY);
        fs::write(&binary, b"tampered rfdetr binary").unwrap();
        assert_eq!(
            verify(
                &binary,
                engine.extracted_binary_sha256.unwrap(),
                None,
                BINARY,
            )
            .unwrap_err()
            .reason_code,
            "sha256_mismatch"
        );
    }

    #[test]
    fn blocked_fit_refuses_before_download_and_leaves_the_tree_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let report = fit_report::build_rfdetr_fit_report_with_free_bytes(
            temp.path(),
            "linux",
            "x86_64",
            Ok(0),
        );
        assert_eq!(report.overall(), fit_report::FitSeverity::Blocked);
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, b"unchanged").unwrap();
        let error = install_rfdetr_with_policy_and_report(
            temp.path(),
            "linux",
            "x86_64",
            true,
            &DENY_ALL_POLICY,
            Some(report),
            None,
        )
        .unwrap_err();
        assert_eq!(error.reason_code, "host_unfit");
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
        assert!(!root(temp.path()).join("engine").exists());
        assert!(!root(temp.path()).join("model").exists());
    }

    #[test]
    fn failed_repair_removes_the_binary_and_model_but_not_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary_path(temp.path(), TEST_KEY);
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"wrong bytes").unwrap();
        let model = model().unwrap();
        let model_path = model_path(temp.path());
        fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&model_path)
            .unwrap()
            .set_len(model.size_bytes)
            .unwrap();
        write_sidecar(
            temp.path(),
            &RfdetrInstallRecord::Installed,
            Some((TEST_KEY, engine(TEST_KEY).unwrap(), model)),
        )
        .unwrap();
        let sentinel = root(temp.path()).join("unrelated-sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        assert_eq!(
            check_rfdetr_model(temp.path(), "linux", "x86_64")
                .unwrap_err()
                .reason_code,
            "sha256_mismatch"
        );

        let error =
            install_rfdetr_with_policy(temp.path(), "linux", "x86_64", false, &DENY_ALL_POLICY)
                .unwrap_err();
        assert_eq!(error.reason_code, "download_host_refused");
        assert!(!binary.exists());
        assert!(!model_path.exists());
        assert!(!sidecar_path(temp.path()).exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }

    #[test]
    fn check_rejects_right_size_wrong_bytes_for_rfdetr_assets() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary_path(temp.path(), TEST_KEY);
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"right-size is not an rf-detr binary").unwrap();
        let model = model().unwrap();
        let model_path = model_path(temp.path());
        fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&model_path)
            .unwrap()
            .set_len(model.size_bytes)
            .unwrap();
        write_sidecar(
            temp.path(),
            &RfdetrInstallRecord::Installed,
            Some((TEST_KEY, engine(TEST_KEY).unwrap(), model)),
        )
        .unwrap();
        assert_eq!(
            check_rfdetr_model(temp.path(), "linux", "x86_64")
                .unwrap_err()
                .reason_code,
            "sha256_mismatch"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sound_no_force_install_keeps_rfdetr_files_and_sweeps_no_temps() {
        let temp = tempfile::tempdir().unwrap();
        let engine = Artifact {
            unit: ENGINE_UNIT,
            version: ENGINE_VERSION,
            filename: "fixture.tar.gz",
            sha256: "unused",
            size_bytes: 0,
            upstream_url: "https://example.invalid/engine",
            origin_key: "test",
            artifact_key: Some("linux-cpu-x64"),
            platform: Some(Platform::LinuxX64),
            backend: Some(Backend::Cpu),
            extracted_binary_sha256: Some(
                "675a67184e2105f6ed183822a42a7d502b0f0d94f0165e21da601acebcb2c196",
            ),
        };
        let model = Artifact {
            unit: MODEL_UNIT,
            version: MODEL_REVISION,
            filename: MODEL_FILE,
            sha256: "3d9211f81f88b0a5b1acff8cd8ada105eab06d0e91707638525b95a0436ff4aa",
            size_bytes: b"sound rf model".len() as u64,
            upstream_url: "https://example.invalid/model",
            origin_key: "test",
            artifact_key: None,
            platform: None,
            backend: None,
            extracted_binary_sha256: None,
        };
        let binary = binary_path(temp.path(), TEST_KEY);
        let model_path = model_path(temp.path());
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        fs::write(&binary, b"sound rf binary").unwrap();
        fs::write(&model_path, b"sound rf model").unwrap();
        let sidecar = installed_sidecar_for(TEST_KEY, &engine, &model).unwrap();
        let sidecar_path = sidecar_path(temp.path());
        fs::write(
            &sidecar_path,
            format!("{}\n", serde_json::to_string_pretty(&sidecar).unwrap()),
        )
        .unwrap();
        let root = root(temp.path());
        prove_temp_sweep(&root, BINARY, "linux-cpu-x64");
        let paths = [binary, model_path, sidecar_path];
        let before = paths.clone().map(|path| fs::metadata(path).unwrap());
        install_rfdetr_with_policy_and_report(
            temp.path(),
            "linux",
            "x86_64",
            false,
            &DENY_ALL_POLICY,
            None,
            Some((&engine, &model)),
        )
        .unwrap();
        for (path, metadata) in paths.iter().zip(before) {
            let after = fs::metadata(path).unwrap();
            assert_eq!(after.ino(), metadata.ino());
            assert_eq!(after.mtime(), metadata.mtime());
        }
        assert!(leaked_temps(&root).is_empty());
    }
}
