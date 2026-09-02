// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Runtime installation for the pinned ced.cpp sound-tagging assets.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_assets::{Artifact, Backend, Platform};
use thiserror::Error;

use super::{archive, download_artifact, select_artifact};

const ENGINE_UNIT: &str = "ced-engine";
const MODEL_UNIT: &str = "ced-model";
/// The engine pin the installer resolves against. Public because the
/// owner-facing download disclosure prints it, and a version typed into that
/// sentence goes silently false at the next bump while the owner reads a
/// version that is not what arrived.
pub const ENGINE_VERSION: &str = "v0.1.0";
const MODEL_REPO: &str = "mudler/ced-gguf";
const MODEL_REPOSITORY_DIRECTORY: &str = "mudler__ced-gguf";
const MODEL_REVISION: &str = "b5e9a4aad6438763c8da16079d77563fbed35c65";
const MODEL_FILE: &str = "ced-tiny-q8_0.gguf";
const SIDECAR: &str = ".ced-install.json";
const MODEL_SIDECAR: &str = ".ced-model.json";
const REQUIRED: [&str; 2] = ["LICENSE", "README.md"];

#[derive(Debug, Error)]
#[error("{message}")]
pub struct CedInstallError {
    pub reason_code: String,
    message: String,
    pub exit_code: u8,
}
impl CedInstallError {
    pub fn new(reason_code: impl Into<String>, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            reason_code: reason_code.into(),
            message: message.into(),
            exit_code,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CedRecord {
    pub artifact_key: String,
    pub engine_version: String,
    pub files: BTreeMap<String, String>,
    pub model_repo: String,
    pub model_revision: String,
}

/// Durable record for the CED model alone.
///
/// Windows keeps the signed CED engine in the immutable application package,
/// so its writable journal state must not describe or admit an engine. This
/// record gives the separately fetched model the same pinned identity and
/// crash-safe publication shape without recreating a mutable-engine layout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CedModelRecord {
    pub model_repo: String,
    pub model_revision: String,
    pub model_file: String,
    pub sha256: String,
    pub size_bytes: u64,
}

pub fn ced_artifact_key(os_name: &str, arch: &str) -> Option<&'static str> {
    match (os_name, arch.to_ascii_lowercase().as_str()) {
        ("linux", "amd64" | "x64" | "x86_64") => Some("linux-cpu-x64"),
        ("linux", "arm64" | "aarch64") => Some("linux-cpu-arm64"),
        ("darwin", "arm64") => Some("macos-metal-arm64"),
        _ => None,
    }
}

fn root(journal: &Path) -> PathBuf {
    journal.join("cache/providers/ced").join(ENGINE_VERSION)
}
fn sidecar(journal: &Path) -> PathBuf {
    root(journal).join(SIDECAR)
}
fn model_sidecar(journal: &Path) -> PathBuf {
    root(journal).join(MODEL_SIDECAR)
}
pub(crate) fn engine_dir(journal: &Path, key: &str) -> PathBuf {
    root(journal).join("engine").join(key)
}
pub fn ced_model_path(journal: &Path) -> PathBuf {
    root(journal)
        .join("models")
        .join(MODEL_REPOSITORY_DIRECTORY)
        .join(MODEL_REVISION)
        .join(MODEL_FILE)
}
pub fn ced_library_path(journal: &Path, key: &str) -> PathBuf {
    engine_dir(journal, key).join(library_name(key))
}
pub(crate) fn library_name(key: &str) -> &'static str {
    if key == "macos-metal-arm64" {
        "libced.dylib"
    } else {
        "libced.so"
    }
}
fn tarball(journal: &Path, row: &Artifact) -> PathBuf {
    root(journal).join("downloads").join(row.filename)
}
fn extract(journal: &Path, key: &str) -> PathBuf {
    root(journal).join("engine").join(format!(".{key}.extract"))
}
fn stage(journal: &Path, key: &str) -> PathBuf {
    root(journal).join("engine").join(format!(".{key}.stage"))
}

fn engine_artifact(key: &str) -> Result<&'static Artifact, CedInstallError> {
    let (platform, backend) = match key {
        "linux-cpu-x64" => (Platform::LinuxX64, Backend::Cpu),
        "linux-cpu-arm64" => (Platform::LinuxArm64, Backend::Cpu),
        "macos-metal-arm64" => (Platform::MacosArm64, Backend::Metal),
        _ => {
            return Err(CedInstallError::new(
                "unsupported_platform",
                format!("ced assets unsupported for {key}"),
                69,
            ));
        }
    };
    select_artifact(ENGINE_UNIT, Some(platform), Some(backend), Some(key), None).map_err(|error| {
        CedInstallError::new(
            "artifact_registry_mismatch",
            error
                .envelope
                .error
                .map(|value| value.message)
                .unwrap_or_else(|| "ced catalog lookup failed".to_owned()),
            65,
        )
    })
}
pub(crate) fn model_artifact() -> Result<&'static Artifact, CedInstallError> {
    select_artifact(MODEL_UNIT, None, None, None, Some(MODEL_FILE)).map_err(|error| {
        CedInstallError::new(
            "artifact_registry_mismatch",
            error
                .envelope
                .error
                .map(|value| value.message)
                .unwrap_or_else(|| "ced model catalog lookup failed".to_owned()),
            65,
        )
    })
}

fn expected_model_record() -> Result<CedModelRecord, CedInstallError> {
    let model = model_artifact()?;
    Ok(CedModelRecord {
        model_repo: MODEL_REPO.to_owned(),
        model_revision: MODEL_REVISION.to_owned(),
        model_file: MODEL_FILE.to_owned(),
        sha256: model.sha256.to_owned(),
        size_bytes: model.size_bytes,
    })
}

fn expected_files(key: &str) -> Result<BTreeMap<String, String>, CedInstallError> {
    let engine = engine_artifact(key)?;
    let model = model_artifact()?;
    // Deliberate: existing owner sidecars use the unescaped repository key, unlike the on-disk path.
    Ok(BTreeMap::from([
        (
            format!("engine/{key}/{}", engine.filename),
            engine.sha256.to_owned(),
        ),
        (
            format!("models/{MODEL_REPO}/{MODEL_REVISION}/{MODEL_FILE}"),
            model.sha256.to_owned(),
        ),
    ]))
}
pub(crate) fn record(key: &str) -> Result<CedRecord, CedInstallError> {
    Ok(CedRecord {
        artifact_key: key.to_owned(),
        engine_version: ENGINE_VERSION.to_owned(),
        files: expected_files(key)?,
        model_repo: MODEL_REPO.to_owned(),
        model_revision: MODEL_REVISION.to_owned(),
    })
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let direct = root.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let mut entries = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            if let Some(found) = find_file(&entry, name) {
                return Some(found);
            }
        } else if entry.file_name().is_some_and(|value| value == name) {
            return Some(entry);
        }
    }
    None
}
fn nonempty(path: &Path, label: &str) -> Result<(), CedInstallError> {
    let metadata = fs::metadata(path).map_err(|_| {
        CedInstallError::new(
            "file_missing",
            format!("ced asset missing: {label} at {}", path.display()),
            65,
        )
    })?;
    if !metadata.is_file() {
        return Err(CedInstallError::new(
            "file_missing",
            format!("ced asset missing: {label} at {}", path.display()),
            65,
        ));
    }
    if metadata.len() == 0 {
        return Err(CedInstallError::new(
            "size_mismatch",
            format!("ced asset is empty: {label} at {}", path.display()),
            65,
        ));
    }
    Ok(())
}
pub(crate) fn write_sidecar(journal: &Path, key: &str) -> Result<(), CedInstallError> {
    let path = sidecar(journal);
    fs::create_dir_all(path.parent().expect("sidecar parent"))
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    let mut text =
        serde_json::to_string_pretty(&record(key)?).expect("static CED record serializes");
    text.push('\n');
    let tmp = path
        .parent()
        .unwrap()
        .join(format!("tmp{}", uuid::Uuid::new_v4().simple()));
    let result = fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, &path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))
}
fn cleanup(journal: &Path, key: Option<&str>) {
    if let Some(key) = key {
        let _ = fs::remove_dir_all(engine_dir(journal, key));
        let _ = fs::remove_dir_all(extract(journal, key));
        let _ = fs::remove_dir_all(stage(journal, key));
        if let Ok(row) = engine_artifact(key) {
            let path = tarball(journal, row);
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(path.with_file_name(format!(
                "{}.tmp",
                path.file_name().unwrap().to_string_lossy()
            )));
        }
    }
    let model = ced_model_path(journal);
    let _ = fs::remove_file(&model);
    let _ = fs::remove_file(model.with_file_name(format!("{}.tmp", MODEL_FILE)));
    let _ = fs::remove_file(sidecar(journal));
}

fn cleanup_model(journal: &Path) {
    let model = ced_model_path(journal);
    let _ = fs::remove_file(&model);
    let _ = fs::remove_file(model.with_file_name(format!("{}.tmp", MODEL_FILE)));
    let _ = fs::remove_file(model_sidecar(journal));
}

/// Check the separately fetched CED model without admitting an engine from the
/// writable journal tree.
///
/// This is the Windows model-only half of the CED ownership boundary. The full
/// digest remains a readiness-time check immediately before CED sees the
/// model; this install-time check establishes the pinned metadata and expected
/// byte length before that probe runs.
pub fn check_ced_model(journal: &Path) -> Result<CedModelRecord, CedInstallError> {
    let path = model_sidecar(journal);
    let text = fs::read_to_string(&path).map_err(|_| {
        CedInstallError::new(
            "sidecar_missing",
            format!("ced model sidecar missing: {}", path.display()),
            65,
        )
    })?;
    let stored: CedModelRecord = serde_json::from_str(&text).map_err(|error| {
        CedInstallError::new(
            "sidecar_invalid",
            format!("ced model sidecar invalid: {}: {error}", path.display()),
            65,
        )
    })?;
    let expected = expected_model_record()?;
    if stored != expected {
        return Err(CedInstallError::new(
            "sidecar_mismatch",
            "ced model sidecar does not match the pinned model spec",
            65,
        ));
    }
    let model = ced_model_path(journal);
    let metadata = fs::metadata(&model).map_err(|_| {
        CedInstallError::new(
            "file_missing",
            format!("ced model missing: {}", model.display()),
            65,
        )
    })?;
    if !metadata.is_file() || metadata.len() != expected.size_bytes {
        return Err(CedInstallError::new(
            "size_mismatch",
            format!(
                "size mismatch for {MODEL_FILE}: expected {}, got {}",
                expected.size_bytes,
                metadata.len()
            ),
            65,
        ));
    }
    Ok(stored)
}

/// Fetch and publish only the pinned CED model.
///
/// Unlike [`install_ced_assets`], this function never downloads, extracts, or
/// writes executable engine code. It is intended for a Windows package whose
/// engine is verified separately from its immutable application directory.
pub fn install_ced_model(journal: &Path, force: bool) -> Result<CedModelRecord, CedInstallError> {
    install_ced_model_with_policy(journal, force, &archive::PRODUCTION_DOWNLOAD_POLICY)
}

pub(crate) fn install_ced_model_with_policy(
    journal: &Path,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<CedModelRecord, CedInstallError> {
    if !force && check_ced_model(journal).is_ok() {
        return check_ced_model(journal);
    }
    let result = (|| {
        let _ = fs::remove_file(model_sidecar(journal));
        let model = model_artifact()?;
        download_artifact(
            model,
            &ced_model_path(journal),
            policy,
            |_, _| {},
            "download_failed",
        )
        .map_err(dispatch_error)?;
        write_model_sidecar(journal)
    })();
    if result.is_err() {
        cleanup_model(journal);
    }
    result
}

pub(crate) fn write_model_sidecar(journal: &Path) -> Result<CedModelRecord, CedInstallError> {
    let record = expected_model_record()?;
    let path = model_sidecar(journal);
    fs::create_dir_all(path.parent().expect("model sidecar parent"))
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    let mut text =
        serde_json::to_string_pretty(&record).expect("static CED model record serializes");
    text.push('\n');
    let temporary = path
        .parent()
        .expect("model sidecar parent")
        .join(format!("tmp{}", uuid::Uuid::new_v4().simple()));
    let publication = fs::write(&temporary, text).and_then(|_| fs::rename(&temporary, &path));
    if publication.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    publication.map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    Ok(record)
}

pub fn check_ced_assets(
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> Result<Option<CedRecord>, CedInstallError> {
    let Some(key) = ced_artifact_key(os_name, arch) else {
        return Ok(None);
    };
    let path = sidecar(journal);
    let text = fs::read_to_string(&path).map_err(|_| {
        CedInstallError::new(
            "sidecar_missing",
            format!("ced sidecar missing: {}", path.display()),
            65,
        )
    })?;
    let stored: CedRecord = serde_json::from_str(&text).map_err(|error| {
        CedInstallError::new(
            "sidecar_invalid",
            format!("ced sidecar invalid: {}: {error}", path.display()),
            65,
        )
    })?;
    let expected = record(key)?;
    if stored.artifact_key != expected.artifact_key
        || stored.engine_version != expected.engine_version
        || stored.model_repo != expected.model_repo
        || stored.model_revision != expected.model_revision
        || stored.files != expected.files
    {
        return Err(CedInstallError::new(
            "sidecar_mismatch",
            "ced sidecar does not match pinned engine/model spec",
            65,
        ));
    }
    let model = model_artifact()?;
    let metadata = fs::metadata(ced_model_path(journal)).map_err(|_| {
        CedInstallError::new(
            "file_missing",
            format!("ced model missing: {}", ced_model_path(journal).display()),
            65,
        )
    })?;
    // Deliberately weak owner check: model size only; engine files merely exist and are nonzero.
    if !metadata.is_file() || metadata.len() != model.size_bytes {
        return Err(CedInstallError::new(
            "size_mismatch",
            format!(
                "size mismatch for {MODEL_FILE}: expected {}, got {}",
                model.size_bytes,
                metadata.len()
            ),
            65,
        ));
    }
    let directory = engine_dir(journal, key);
    nonempty(&directory.join(library_name(key)), library_name(key))?;
    nonempty(&directory.join("ced_capi.h"), "ced_capi.h")?;
    for name in REQUIRED {
        nonempty(&directory.join(name), name)?;
    }
    Ok(Some(stored))
}

pub fn install_ced_assets(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
) -> Result<Option<CedRecord>, CedInstallError> {
    install_ced_assets_with_policy(
        journal,
        os_name,
        arch,
        force,
        &archive::PRODUCTION_DOWNLOAD_POLICY,
    )
}
pub(crate) fn install_ced_assets_with_policy(
    journal: &Path,
    os_name: &str,
    arch: &str,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<Option<CedRecord>, CedInstallError> {
    let Some(key) = ced_artifact_key(os_name, arch) else {
        return Ok(None);
    };
    if !force && check_ced_assets(journal, os_name, arch).is_ok() {
        return check_ced_assets(journal, os_name, arch);
    }
    let result = (|| {
        let _ = fs::remove_file(sidecar(journal));
        let engine = engine_artifact(key)?;
        let archive_path = tarball(journal, engine);
        download_artifact(engine, &archive_path, policy, |_, _| {}, "download_failed")
            .map_err(dispatch_error)?;
        let extract_dir = extract(journal, key);
        let stage_dir = stage(journal, key);
        let _ = fs::remove_dir_all(&extract_dir);
        let _ = fs::remove_dir_all(&stage_dir);
        fs::create_dir_all(&extract_dir)
            .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
        archive::extract_tar_gz(&archive_path, &extract_dir).map_err(|error| {
            CedInstallError::new(
                if matches!(error, archive::ArchiveError::PathEscape(_)) {
                    "archive_path_traversal"
                } else {
                    "extract_failed"
                },
                error.to_string(),
                if matches!(error, archive::ArchiveError::PathEscape(_)) {
                    65
                } else {
                    74
                },
            )
        })?;
        fs::create_dir_all(&stage_dir)
            .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
        for name in [library_name(key), "ced_capi.h", "LICENSE", "README.md"] {
            let source = find_file(&extract_dir, name).ok_or_else(|| {
                CedInstallError::new(
                    "file_missing",
                    format!("extracted ced archive did not contain {name}"),
                    65,
                )
            })?;
            fs::copy(source, stage_dir.join(name))
                .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
        }
        let final_dir = engine_dir(journal, key);
        if final_dir.exists() {
            fs::remove_dir_all(&final_dir)
                .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
        }
        fs::create_dir_all(final_dir.parent().unwrap())
            .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
        fs::rename(&stage_dir, &final_dir)
            .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
        let _ = fs::remove_file(&archive_path);
        let _ = fs::remove_file(archive_path.with_file_name(format!("{}.tmp", engine.filename)));
        let _ = fs::remove_dir_all(&extract_dir);
        let _ = fs::remove_dir_all(&stage_dir);
        let model = model_artifact()?;
        download_artifact(
            model,
            &ced_model_path(journal),
            policy,
            |_, _| {},
            "download_failed",
        )
        .map_err(dispatch_error)?;
        write_sidecar(journal, key)?;
        Ok(Some(record(key)?))
    })();
    if result.is_err() {
        cleanup(journal, Some(key));
    }
    result
}
fn dispatch_error(error: super::DispatchError) -> CedInstallError {
    CedInstallError::new(
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
            .unwrap_or_else(|| "ced download failed".to_owned()),
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

    fn sound_fixture(journal: &Path) {
        let key = "linux-cpu-x64";
        write_sidecar(journal, key).unwrap();
        let model = model_artifact().unwrap();
        let model_path = ced_model_path(journal);
        fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&model_path)
            .unwrap()
            .set_len(model.size_bytes)
            .unwrap();
        let engine = engine_dir(journal, key);
        fs::create_dir_all(&engine).unwrap();
        for name in [library_name(key), "ced_capi.h", "LICENSE", "README.md"] {
            fs::write(engine.join(name), b"nonempty fixture").unwrap();
        }
    }

    fn model_only_fixture(journal: &Path) {
        let model = model_artifact().unwrap();
        let model_path = ced_model_path(journal);
        fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&model_path)
            .unwrap()
            .set_len(model.size_bytes)
            .unwrap();
        write_model_sidecar(journal).unwrap();
    }

    #[test]
    fn unsupported_platform_is_silent_for_check_and_install() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            check_ced_assets(temp.path(), "macos", "aarch64")
                .unwrap()
                .is_none()
        );
        assert!(
            install_ced_assets_with_policy(
                temp.path(),
                "macos",
                "aarch64",
                false,
                &archive::PRODUCTION_DOWNLOAD_POLICY,
            )
            .unwrap()
            .is_none()
        );
        assert!(!sidecar(temp.path()).exists());
    }

    #[test]
    fn check_preserves_the_owner_weak_file_checks() {
        let temp = tempfile::tempdir().unwrap();
        sound_fixture(temp.path());
        assert!(check_ced_assets(temp.path(), "linux", "x86_64").is_ok());

        let model = model_artifact().unwrap();
        OpenOptions::new()
            .write(true)
            .open(ced_model_path(temp.path()))
            .unwrap()
            .set_len(model.size_bytes - 1)
            .unwrap();
        assert_eq!(
            check_ced_assets(temp.path(), "linux", "x86_64")
                .unwrap_err()
                .reason_code,
            "size_mismatch"
        );

        OpenOptions::new()
            .write(true)
            .open(ced_model_path(temp.path()))
            .unwrap()
            .set_len(model.size_bytes)
            .unwrap();
        fs::write(
            engine_dir(temp.path(), "linux-cpu-x64").join("libced.so"),
            b"garbage",
        )
        .unwrap();
        assert!(check_ced_assets(temp.path(), "linux", "x86_64").is_ok());
        fs::write(
            engine_dir(temp.path(), "linux-cpu-x64").join("libced.so"),
            b"",
        )
        .unwrap();
        assert_eq!(
            check_ced_assets(temp.path(), "linux", "x86_64")
                .unwrap_err()
                .reason_code,
            "size_mismatch"
        );
    }

    #[cfg(unix)]
    #[test]
    fn model_only_install_keeps_journal_engine_free_and_never_fetches_when_ready() {
        let temp = tempfile::tempdir().unwrap();
        model_only_fixture(temp.path());
        let root = root(temp.path());
        let model = ced_model_path(temp.path());
        let before = fs::metadata(&model).unwrap();

        assert!(check_ced_model(temp.path()).is_ok());
        assert!(install_ced_model_with_policy(temp.path(), false, &DENY_ALL_POLICY).is_ok());
        let after = fs::metadata(&model).unwrap();
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.mtime(), before.mtime());
        assert!(!root.join("engine").exists());
        assert!(leaked_temps(&root).is_empty());
    }

    #[test]
    fn failed_model_only_repair_removes_only_model_state() {
        let temp = tempfile::tempdir().unwrap();
        model_only_fixture(temp.path());
        let sentinel = root(temp.path()).join("unrelated-sentinel");
        fs::write(&sentinel, b"keep").unwrap();

        let error = install_ced_model_with_policy(temp.path(), true, &DENY_ALL_POLICY)
            .expect_err("denied model download must fail");
        assert_eq!(error.reason_code, "download_host_refused");
        assert!(!ced_model_path(temp.path()).exists());
        assert!(!model_sidecar(temp.path()).exists());
        assert!(!root(temp.path()).join("engine").exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn sound_no_force_install_keeps_ced_files_and_sweeps_no_temps() {
        let temp = tempfile::tempdir().unwrap();
        sound_fixture(temp.path());
        let key = "linux-cpu-x64";
        let root = root(temp.path());
        prove_temp_sweep(&root, "ced.tar.gz", key);

        let model = ced_model_path(temp.path());
        let library = engine_dir(temp.path(), key).join(library_name(key));
        let sidecar = sidecar(temp.path());
        let before = [
            fs::metadata(&model).unwrap(),
            fs::metadata(&library).unwrap(),
            fs::metadata(&sidecar).unwrap(),
        ];
        assert!(install_ced_assets_with_policy(
            temp.path(),
            "linux",
            "x86_64",
            false,
            &DENY_ALL_POLICY,
        )
        .is_ok());
        for (path, metadata) in [model, library, sidecar].iter().zip(before) {
            let after = fs::metadata(path).unwrap();
            assert_eq!(after.ino(), metadata.ino());
            assert_eq!(after.mtime(), metadata.mtime());
        }
        assert!(leaked_temps(&root).is_empty());
    }

    #[test]
    fn failed_repair_removes_the_engine_and_model_but_not_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        sound_fixture(temp.path());
        let model = model_artifact().unwrap();
        OpenOptions::new()
            .write(true)
            .open(ced_model_path(temp.path()))
            .unwrap()
            .set_len(model.size_bytes - 1)
            .unwrap();
        let sentinel = root(temp.path()).join("unrelated-sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        assert!(check_ced_assets(temp.path(), "linux", "x86_64").is_err());

        let error =
            install_ced_assets_with_policy(temp.path(), "linux", "x86_64", false, &DENY_ALL_POLICY)
                .unwrap_err();
        assert_eq!(error.reason_code, "download_host_refused");
        assert!(!engine_dir(temp.path(), "linux-cpu-x64").exists());
        assert!(!ced_model_path(temp.path()).exists());
        assert!(!sidecar(temp.path()).exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }
}
