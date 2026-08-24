// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Runtime installation for the bundled rf-detr.cpp detector.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{archive, fit_report};

pub const ENGINE_VERSION: &str = "v0.1.0-solpbc.5";
pub const ENGINE_PROVENANCE_REF: &str = "ec73712e";
pub const RFDETR_ENGINE_LINUX_CPU_X64_TARBALL_SHA256: &str =
    "56231d6675395ed790dba882e0335e4c79616427af558b1820975951cd9d14a7";
pub const RFDETR_ENGINE_LINUX_CPU_X64_BINARY_SHA256: &str =
    "6f225708e4b9dafc39a085f1323bc426ca037b746b3be9c7c571d9be494306af";
pub const RFDETR_ENGINE_LINUX_CPU_ARM64_TARBALL_SHA256: &str =
    "2c11e1af6986571d4d9f4d2cf377018973095b10c234a9da40a3edf45cf11f9d";
pub const RFDETR_ENGINE_LINUX_CPU_ARM64_BINARY_SHA256: &str =
    "14c47251ffd61a3ef0dc358c4b6a88d8718c5c3f266f4d79db9ae1440e3b6ecc";
pub const RFDETR_ENGINE_MACOS_METAL_ARM64_TARBALL_SHA256: &str =
    "46b497950c7a73000007abdb9ef54bc8b46ba0a46dcf26f6c0ae51fccd21ad71";
pub const RFDETR_ENGINE_MACOS_METAL_ARM64_BINARY_SHA256: &str =
    "f15d89e24d44245e2288e0d9839e54d4495d6ebf1071e1f906805f2989d18c9e";
pub const RFDETR_MODEL_SHA256: &str =
    "d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee";

const BINARY: &str = "rfdetr-cli";
const MODEL_REVISION: &str = "c3dc0c037df499f5503545247df6618415fca643";
const MODEL_FILE: &str = "rfdetr-nano-f16.gguf";
const MODEL_REPO: &str = "mudler/rfdetr-cpp-nano";
const MODEL_SIZE: u64 = 63_439_488;
const SIDECAR: &str = ".rfdetr-install.json";

#[derive(Debug, Clone, Copy)]
pub struct EngineSpec {
    pub filename: &'static str,
    pub tarball_sha256: &'static str,
    pub tarball_size: u64,
    pub binary_sha256: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub sha256: &'static str,
    pub size: u64,
}

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

fn engine_spec(key: &str) -> &'static EngineSpec {
    const X64: EngineSpec = EngineSpec {
        filename: "rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz",
        tarball_sha256: RFDETR_ENGINE_LINUX_CPU_X64_TARBALL_SHA256,
        tarball_size: 952_974,
        binary_sha256: RFDETR_ENGINE_LINUX_CPU_X64_BINARY_SHA256,
    };
    const ARM64: EngineSpec = EngineSpec {
        filename: "rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64.tar.gz",
        tarball_sha256: RFDETR_ENGINE_LINUX_CPU_ARM64_TARBALL_SHA256,
        tarball_size: 869_316,
        binary_sha256: RFDETR_ENGINE_LINUX_CPU_ARM64_BINARY_SHA256,
    };
    const MACOS: EngineSpec = EngineSpec {
        filename: "rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz",
        tarball_sha256: RFDETR_ENGINE_MACOS_METAL_ARM64_TARBALL_SHA256,
        tarball_size: 994_991,
        binary_sha256: RFDETR_ENGINE_MACOS_METAL_ARM64_BINARY_SHA256,
    };
    match key {
        "linux-cpu-x64" => &X64,
        "linux-cpu-arm64" => &ARM64,
        "macos-metal-arm64" => &MACOS,
        _ => unreachable!("only rfdetr_artifact_key outputs reach engine_spec"),
    }
}

fn model_spec() -> &'static ModelSpec {
    const MODEL: ModelSpec = ModelSpec {
        sha256: RFDETR_MODEL_SHA256,
        size: MODEL_SIZE,
    };
    &MODEL
}

/// Locate a bundled RF-DETR payload without creating or changing any state.
pub fn resolve_rfdetr_asset(filename: &str) -> Result<PathBuf, RfdetrInstallError> {
    resolve_rfdetr_asset_from(
        filename,
        Path::new(env!("CARGO_MANIFEST_DIR")),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
    )
}

fn resolve_rfdetr_asset_from(
    filename: &str,
    manifest_dir: &Path,
    executable_parent: Option<PathBuf>,
) -> Result<PathBuf, RfdetrInstallError> {
    for ancestor in manifest_dir.ancestors() {
        let candidate = ancestor.join("core/models/assets/rfdetr").join(filename);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(parent) = executable_parent {
        for ancestor in parent.ancestors() {
            let candidate = ancestor
                .join("lib/solstone_journal_models/assets/rfdetr")
                .join(filename);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(RfdetrInstallError::new(
        "bundled_payload_missing",
        format!("bundled rf-detr asset missing: {filename}; reinstall the journal package"),
        65,
    ))
}

#[cfg(any(test, feature = "test-hooks"))]
fn resolve_rfdetr_asset_in(root: &Path, filename: &str) -> Result<PathBuf, RfdetrInstallError> {
    let candidate = root.join(filename);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(RfdetrInstallError::new(
            "bundled_payload_missing",
            format!("bundled rf-detr asset missing: {filename}; reinstall the journal package"),
            65,
        ))
    }
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

fn installed_sidecar_for(key: &str, engine: &EngineSpec, model: &ModelSpec) -> Sidecar {
    Sidecar {
        artifact_key: Some(key.to_owned()),
        engine_version: Some(ENGINE_VERSION.to_owned()),
        engine_sha256: Some(engine.tarball_sha256.to_owned()),
        model_file: Some(MODEL_FILE.to_owned()),
        model_repo: Some(MODEL_REPO.to_owned()),
        model_revision: Some(MODEL_REVISION.to_owned()),
        model_sha256: Some(model.sha256.to_owned()),
        status: "installed".to_owned(),
    }
}

fn write_sidecar(
    journal: &Path,
    record: &RfdetrInstallRecord,
    artifacts: Option<(&str, &EngineSpec, &ModelSpec)>,
) -> Result<(), RfdetrInstallError> {
    let record = match record {
        RfdetrInstallRecord::Installed => {
            let (key, engine, model) = artifacts.expect("installed sidecars have specs");
            installed_sidecar_for(key, engine, model)
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
    atomic_write(&path, text.as_bytes())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RfdetrInstallError> {
    let tmp = path
        .parent()
        .unwrap()
        .join(format!("tmp{}", uuid::Uuid::new_v4().simple()));
    let result = fs::write(&tmp, bytes).and_then(|_| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))
}

fn cleanup(journal: &Path, key: &str, model: &ModelSpec) {
    let binary = binary_path(journal, key);
    let _ = fs::remove_file(&binary);
    let _ = fs::remove_file(binary.with_file_name(format!("{BINARY}.tmp")));
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
    check_rfdetr_model_with_rows(journal, key, engine_spec(key), model_spec())
}

fn check_rfdetr_model_with_rows(
    journal: &Path,
    key: &str,
    engine: &EngineSpec,
    model: &ModelSpec,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    let path = sidecar_path(journal);
    let text = fs::read_to_string(&path).map_err(|_| {
        RfdetrInstallError::new(
            "sidecar_missing",
            format!("rf-detr sidecar missing: {}", path.display()),
            65,
        )
    })?;
    let stored: Sidecar = serde_json::from_str(&text).map_err(|error| {
        RfdetrInstallError::new(
            "sidecar_invalid",
            format!("rf-detr sidecar invalid: {}: {error}", path.display()),
            65,
        )
    })?;
    let expected = installed_sidecar_for(key, engine, model);
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
        engine.binary_sha256,
        None,
        BINARY,
    )?;
    verify(
        &model_path(journal),
        model.sha256,
        Some(model.size),
        MODEL_FILE,
    )?;
    Ok(RfdetrInstallRecord::Installed)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn check_rfdetr_model_with_artifacts(
    journal: &Path,
    key: &str,
    engine: &EngineSpec,
    model: &ModelSpec,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    check_rfdetr_model_with_rows(journal, key, engine, model)
}

pub fn install_rfdetr(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    install_rfdetr_with_sources(journal, os_name, arch, force, None, None, None)
}

fn install_rfdetr_with_sources(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
    report_override: Option<fit_report::FitReport>,
    spec_override: Option<(&EngineSpec, &ModelSpec)>,
    #[cfg(any(test, feature = "test-hooks"))] fixture_assets: Option<&Path>,
    #[cfg(not(any(test, feature = "test-hooks")))] _fixture_assets: Option<&Path>,
) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
    let Some(key) = rfdetr_artifact_key(os_name, arch) else {
        let record = RfdetrInstallRecord::PlatformUnavailable;
        write_sidecar(journal, &record, None)?;
        log::info!("rf-detr.cpp platform unavailable on {os_name}/{arch}");
        return Ok(record);
    };
    let (engine, model) = spec_override.unwrap_or((engine_spec(key), model_spec()));
    if !force && let Ok(record) = check_rfdetr_model_with_rows(journal, key, engine, model) {
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
    cleanup(journal, key, model);
    let result = (|| {
        #[cfg(any(test, feature = "test-hooks"))]
        let tarball = match fixture_assets {
            Some(root) => resolve_rfdetr_asset_in(root, engine.filename),
            None => resolve_rfdetr_asset(engine.filename),
        }?;
        #[cfg(not(any(test, feature = "test-hooks")))]
        let tarball = resolve_rfdetr_asset(engine.filename)?;
        verify(
            &tarball,
            engine.tarball_sha256,
            Some(engine.tarball_size),
            engine.filename,
        )?;
        let extract = extract_path(journal, key);
        let _ = fs::remove_dir_all(&extract);
        fs::create_dir_all(&extract)
            .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))?;
        archive::extract_tar_gz(&tarball, &extract).map_err(|error| {
            let escaped = matches!(error, archive::ArchiveError::PathEscape(_));
            RfdetrInstallError::new(
                if escaped {
                    "archive_path_traversal"
                } else {
                    "extract_failed"
                },
                error.to_string(),
                if escaped { 65 } else { 74 },
            )
        })?;
        let found = find_binary(&extract).ok_or_else(|| {
            RfdetrInstallError::new(
                "binary_missing",
                format!("Extracted archive did not contain {BINARY}"),
                65,
            )
        })?;
        verify(&found, engine.binary_sha256, None, BINARY)?;
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
        #[cfg(any(test, feature = "test-hooks"))]
        let bundled_model = match fixture_assets {
            Some(root) => resolve_rfdetr_asset_in(root, MODEL_FILE),
            None => resolve_rfdetr_asset(MODEL_FILE),
        }?;
        #[cfg(not(any(test, feature = "test-hooks")))]
        let bundled_model = resolve_rfdetr_asset(MODEL_FILE)?;
        verify(&bundled_model, model.sha256, Some(model.size), MODEL_FILE)?;
        copy_verified(&bundled_model, &model_path(journal), model)?;
        let record = RfdetrInstallRecord::Installed;
        write_sidecar(journal, &record, Some((key, engine, model)))?;
        check_rfdetr_model_with_rows(journal, key, engine, model)?;
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
        cleanup(journal, key, model);
    }
    result
}

fn copy_verified(
    source: &Path,
    destination: &Path,
    model: &ModelSpec,
) -> Result<(), RfdetrInstallError> {
    fs::create_dir_all(destination.parent().unwrap())
        .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))?;
    let tmp = destination
        .parent()
        .unwrap()
        .join(format!("tmp{}", uuid::Uuid::new_v4().simple()));
    let result = fs::copy(source, &tmp)
        .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))
        .and_then(|_| verify(&tmp, model.sha256, Some(model.size), MODEL_FILE))
        .and_then(|_| {
            fs::rename(&tmp, destination)
                .map_err(|error| RfdetrInstallError::new("install_failed", error.to_string(), 74))
        });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use sha2::{Digest, Sha256};

    fn digest(bytes: &[u8]) -> &'static str {
        Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str())
    }
    fn fixture(binary: &[u8], model: &[u8]) -> (tempfile::TempDir, EngineSpec, ModelSpec) {
        let root = tempfile::tempdir().unwrap();
        let archive_path = root.path().join("fixture.tar.gz");
        let file = fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(binary.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append_data(&mut header, BINARY, binary).unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        let archive_bytes = fs::read(&archive_path).unwrap();
        fs::write(root.path().join(MODEL_FILE), model).unwrap();
        (
            root,
            EngineSpec {
                filename: "fixture.tar.gz",
                tarball_sha256: digest(&archive_bytes),
                tarball_size: archive_bytes.len() as u64,
                binary_sha256: digest(binary),
            },
            ModelSpec {
                sha256: digest(model),
                size: model.len() as u64,
            },
        )
    }
    fn install_fixture(
        journal: &Path,
        assets: &Path,
        engine: &EngineSpec,
        model: &ModelSpec,
        force: bool,
    ) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
        install_rfdetr_with_sources(
            journal,
            "linux",
            "x86_64",
            force,
            None,
            Some((engine, model)),
            Some(assets),
        )
    }
    #[test]
    fn rfdetr_artifact_keys_select_bundled_specs() {
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
        assert_eq!(engine_spec("linux-cpu-x64").tarball_size, 952_974);
    }
    #[test]
    fn unsupported_platform_only_writes_a_sidecar_on_install() {
        let journal = tempfile::tempdir().unwrap();
        assert_eq!(
            check_rfdetr_model(journal.path(), "windows", "x86_64").unwrap(),
            RfdetrInstallRecord::PlatformUnavailable
        );
        assert!(!sidecar_path(journal.path()).exists());
        assert_eq!(
            install_rfdetr(journal.path(), "windows", "x86_64", false).unwrap(),
            RfdetrInstallRecord::PlatformUnavailable
        );
        assert!(sidecar_path(journal.path()).is_file());
    }
    #[test]
    fn missing_bundled_payload_is_reported() {
        let journal = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let (_, engine, model) = fixture(b"#!/bin/sh\nexit 0\n", b"model");
        assert_eq!(
            install_fixture(journal.path(), assets.path(), &engine, &model, false)
                .unwrap_err()
                .reason_code,
            "bundled_payload_missing"
        );
    }
    #[test]
    fn resolver_finds_development_and_installed_layouts() {
        let root = tempfile::tempdir().unwrap();
        let development = root.path().join("core/models/assets/rfdetr/fixture");
        fs::create_dir_all(development.parent().unwrap()).unwrap();
        fs::write(&development, b"fixture").unwrap();
        assert_eq!(
            resolve_rfdetr_asset_from(
                "fixture",
                &root.path().join("core/crates/solstone-core-local"),
                None,
            )
            .unwrap(),
            development
        );

        fs::remove_file(&development).unwrap();
        let installed = root
            .path()
            .join("lib/solstone_journal_models/assets/rfdetr/fixture");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(&installed, b"fixture").unwrap();
        assert_eq!(
            resolve_rfdetr_asset_from(
                "fixture",
                &root.path().join("unrelated"),
                Some(root.path().join("bin")),
            )
            .unwrap(),
            installed
        );
    }
    #[test]
    fn extracted_binary_digest_mismatch_cleans_install_outputs() {
        let journal = tempfile::tempdir().unwrap();
        let (assets, mut engine, model) = fixture(b"binary", b"model");
        engine.binary_sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(
            install_fixture(journal.path(), assets.path(), &engine, &model, true)
                .unwrap_err()
                .reason_code,
            "sha256_mismatch"
        );
        assert!(!binary_path(journal.path(), "linux-cpu-x64").exists());
        assert!(!model_path(journal.path()).exists());
    }
    #[test]
    fn blocked_fit_refuses_before_materializing() {
        let journal = tempfile::tempdir().unwrap();
        let (assets, engine, model) = fixture(b"binary", b"model");
        let report = fit_report::build_rfdetr_fit_report_with_free_bytes(
            journal.path(),
            "linux",
            "x86_64",
            Ok(1),
        );
        let error = install_rfdetr_with_sources(
            journal.path(),
            "linux",
            "x86_64",
            false,
            Some(report),
            Some((&engine, &model)),
            Some(assets.path()),
        )
        .unwrap_err();
        assert_eq!(error.reason_code, "host_unfit");
        assert!(!root(journal.path()).exists());
    }
    #[test]
    fn check_rejects_right_size_wrong_bytes() {
        let journal = tempfile::tempdir().unwrap();
        let (assets, engine, model) = fixture(b"#!/bin/sh\nexit 0\n", b"model");
        install_fixture(journal.path(), assets.path(), &engine, &model, false).unwrap();
        fs::write(model_path(journal.path()), b"wrong").unwrap();
        assert_eq!(
            check_rfdetr_model_with_rows(journal.path(), "linux-cpu-x64", &engine, &model)
                .unwrap_err()
                .reason_code,
            "sha256_mismatch"
        );
    }
    #[test]
    fn install_check_and_repair_use_bundled_inputs_without_network() {
        let journal = tempfile::tempdir().unwrap();
        let (assets, engine, model) = fixture(
            b"#!/bin/sh\n[ \"$1\" = \"--help\" ] && exit 0\nexit 1\n",
            b"model",
        );
        install_fixture(journal.path(), assets.path(), &engine, &model, false).unwrap();
        fs::write(binary_path(journal.path(), "linux-cpu-x64"), b"broken").unwrap();
        install_fixture(journal.path(), assets.path(), &engine, &model, false).unwrap();
        assert_eq!(
            check_rfdetr_model_with_rows(journal.path(), "linux-cpu-x64", &engine, &model).unwrap(),
            RfdetrInstallRecord::Installed
        );
    }
}
