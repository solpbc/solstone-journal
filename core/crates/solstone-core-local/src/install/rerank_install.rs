// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Runtime installation for the pinned rerank ONNX model.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_assets::Artifact;
use thiserror::Error;

use super::{archive, download_artifact, select_artifact};

const UNIT: &str = "rerank-model";
const REVISION: &str = "a09144355adeed5f58c8ed011d209bf8ee5a1fec";
const REPO: &str = "Xenova/ms-marco-MiniLM-L-6-v2";
const SIDECAR: &str = ".rerank-install.json";
const MODEL: &str = "onnx/model.onnx";
const TOKENIZER: &str = "tokenizer.json";

#[derive(Debug, Error)]
#[error("{message}")]
pub struct RerankInstallError {
    pub reason_code: String,
    message: String,
    pub exit_code: u8,
}

impl RerankInstallError {
    pub fn new(reason_code: impl Into<String>, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            reason_code: reason_code.into(),
            message: message.into(),
            exit_code,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    files: BTreeMap<String, String>,
    repo: String,
    revision: String,
}

fn root(journal: &Path) -> PathBuf {
    journal.join("cache/providers/rerank").join(REVISION)
}

fn sidecar(journal: &Path) -> PathBuf {
    root(journal).join(SIDECAR)
}

fn artifact(filename: &str) -> Result<&'static Artifact, RerankInstallError> {
    select_artifact(UNIT, None, None, None, Some(filename)).map_err(|error| {
        RerankInstallError::new(
            "artifact_registry_mismatch",
            error
                .envelope
                .error
                .map(|value| value.message)
                .unwrap_or_else(|| "rerank catalog lookup failed".to_owned()),
            65,
        )
    })
}

fn expected() -> Result<Vec<&'static Artifact>, RerankInstallError> {
    Ok(vec![artifact(MODEL)?, artifact(TOKENIZER)?])
}

fn expected_files(rows: &[&Artifact]) -> BTreeMap<String, String> {
    rows.iter()
        .map(|row| (row.filename.to_owned(), row.sha256.to_owned()))
        .collect()
}

fn verify(path: &Path, row: &Artifact) -> Result<(), RerankInstallError> {
    let metadata = fs::metadata(path).map_err(|_| {
        RerankInstallError::new(
            "file_missing",
            format!("rerank asset missing: {}", row.filename),
            65,
        )
    })?;
    if !metadata.is_file() {
        return Err(RerankInstallError::new(
            "file_missing",
            format!("rerank asset missing: {}", row.filename),
            65,
        ));
    }
    if metadata.len() != row.size_bytes {
        return Err(RerankInstallError::new(
            "size_mismatch",
            format!(
                "size mismatch for {}: expected {}, got {}",
                row.filename,
                row.size_bytes,
                metadata.len()
            ),
            65,
        ));
    }
    archive::verify_sha256(path, row.sha256).map_err(|_| {
        RerankInstallError::new(
            "sha256_mismatch",
            format!("sha256 mismatch for {}", row.filename),
            65,
        )
    })?;
    Ok(())
}

fn write_sidecar(journal: &Path, rows: &[&Artifact]) -> Result<(), RerankInstallError> {
    let path = sidecar(journal);
    fs::create_dir_all(path.parent().expect("sidecar parent"))
        .map_err(|error| RerankInstallError::new("install_failed", error.to_string(), 74))?;
    let record = Record {
        files: expected_files(rows),
        repo: REPO.to_owned(),
        revision: REVISION.to_owned(),
    };
    let mut bytes = serde_json::to_string_pretty(&record).expect("static rerank record serializes");
    bytes.push('\n');
    let temporary = path
        .parent()
        .expect("sidecar parent")
        .join(format!("tmp{}", uuid::Uuid::new_v4().simple()));
    let result = fs::write(&temporary, bytes).and_then(|_| fs::rename(&temporary, &path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| RerankInstallError::new("install_failed", error.to_string(), 74))
}

fn cleanup(journal: &Path, rows: &[&Artifact]) {
    for row in rows {
        let path = root(journal).join(row.filename);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_file_name(format!(
            "{}.tmp",
            path.file_name().unwrap().to_string_lossy()
        )));
        let _ = fs::remove_file(path.with_file_name(format!(
            ".{}.part",
            path.file_name().unwrap().to_string_lossy()
        )));
    }
    let _ = fs::remove_file(sidecar(journal));
}

pub fn check_rerank_model(journal: &Path) -> Result<(), RerankInstallError> {
    let rows = expected()?;
    check_rerank_model_with_rows(journal, &rows)
}

fn check_rerank_model_with_rows(
    journal: &Path,
    rows: &[&Artifact],
) -> Result<(), RerankInstallError> {
    let text = fs::read_to_string(sidecar(journal)).map_err(|_| {
        RerankInstallError::new(
            "sidecar_missing",
            format!("rerank sidecar missing: {}", sidecar(journal).display()),
            65,
        )
    })?;
    let record: Record = serde_json::from_str(&text).map_err(|error| {
        RerankInstallError::new(
            "sidecar_invalid",
            format!(
                "rerank sidecar invalid: {}: {error}",
                sidecar(journal).display()
            ),
            65,
        )
    })?;
    if record.repo != REPO || record.revision != REVISION || record.files != expected_files(rows) {
        return Err(RerankInstallError::new(
            "sidecar_mismatch",
            "rerank sidecar does not match pinned artifacts",
            65,
        ));
    }
    for row in rows {
        verify(&root(journal).join(row.filename), row)?;
    }
    Ok(())
}

pub fn install_rerank_model(journal: &Path, force: bool) -> Result<(), RerankInstallError> {
    install_rerank_model_with_policy(journal, force, &archive::PRODUCTION_DOWNLOAD_POLICY)
}

pub(crate) fn install_rerank_model_with_policy(
    journal: &Path,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<(), RerankInstallError> {
    let rows = expected()?;
    install_rerank_model_with_rows(journal, force, policy, &rows)
}

fn install_rerank_model_with_rows(
    journal: &Path,
    force: bool,
    policy: &archive::DownloadHostPolicy<'_>,
    rows: &[&Artifact],
) -> Result<(), RerankInstallError> {
    if !force && check_rerank_model_with_rows(journal, rows).is_ok() {
        return Ok(());
    }
    let result = (|| {
        let _ = fs::remove_file(sidecar(journal));
        for row in rows {
            let destination = root(journal).join(row.filename);
            download_artifact(row, &destination, policy, |_, _| {}, "download_failed").map_err(
                |error| {
                    RerankInstallError::new(
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
                            .unwrap_or_else(|| "rerank download failed".to_owned()),
                        error.exit_code,
                    )
                },
            )?;
        }
        write_sidecar(journal, rows)
    })();
    if result.is_err() {
        cleanup(journal, rows);
    }
    result
}

#[cfg(feature = "differential")]
pub fn differential_snapshot(journal: &Path) -> serde_json::Value {
    let rows = expected().expect("static rerank catalog rows");
    serde_json::json!({
        "repo": REPO,
        "revision": REVISION,
        "files": rows.iter().map(|row| serde_json::json!({
            "filename": row.filename,
            "sha256": row.sha256,
            "size_bytes": row.size_bytes,
            "destination": root(journal).join(row.filename),
        })).collect::<Vec<_>>(),
        "cache_root": root(journal).parent().expect("rerank asset root has cache parent"),
        "sidecar": sidecar(journal),
        "sidecar_file_keys": expected_files(&rows).into_keys().collect::<Vec<_>>(),
    })
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

    #[test]
    fn check_rejects_right_size_wrong_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let rows = expected().unwrap();
        write_sidecar(temp.path(), &rows).unwrap();
        for row in rows {
            let path = root(temp.path()).join(row.filename);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)
                .unwrap()
                .set_len(row.size_bytes)
                .unwrap();
        }
        assert_eq!(
            check_rerank_model(temp.path()).unwrap_err().reason_code,
            "sha256_mismatch"
        );
    }

    #[test]
    fn check_has_no_platform_skip() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            check_rerank_model(temp.path()).unwrap_err().reason_code,
            "sidecar_missing"
        );
    }

    #[test]
    fn failed_repair_removes_the_full_rerank_set_but_not_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let rows = expected().unwrap();
        write_sidecar(temp.path(), &rows).unwrap();
        for row in rows {
            let path = root(temp.path()).join(row.filename);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)
                .unwrap()
                .set_len(row.size_bytes)
                .unwrap();
        }
        let sentinel = root(temp.path()).join("unrelated-sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        assert!(check_rerank_model(temp.path()).is_err());

        let error =
            install_rerank_model_with_policy(temp.path(), false, &DENY_ALL_POLICY).unwrap_err();
        assert_eq!(error.reason_code, "download_host_refused");
        assert!(!root(temp.path()).join(MODEL).exists());
        assert!(!root(temp.path()).join(TOKENIZER).exists());
        assert!(!sidecar(temp.path()).exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn sound_no_force_install_keeps_rerank_files_and_sweeps_no_temps() {
        let temp = tempfile::tempdir().unwrap();
        let model_bytes = b"sound rerank model";
        let tokenizer_bytes = b"sound rerank tokenizer";
        let rows = [
            Artifact {
                unit: UNIT,
                version: REVISION,
                filename: MODEL,
                sha256: "d8e745035f836a6c8c122129f930adba3b000a2e8600987f3d5517b1d73cc64a",
                size_bytes: model_bytes.len() as u64,
                upstream_url: "https://example.invalid/model",
                origin_key: "test",
                artifact_key: None,
                platform: None,
                backend: None,
                extracted_binary_sha256: None,
            },
            Artifact {
                unit: UNIT,
                version: REVISION,
                filename: TOKENIZER,
                sha256: "3127e6111fa15cecb9acfbebdd020a80b08d2ef2f2db7e5c38ec638fb2d51244",
                size_bytes: tokenizer_bytes.len() as u64,
                upstream_url: "https://example.invalid/tokenizer",
                origin_key: "test",
                artifact_key: None,
                platform: None,
                backend: None,
                extracted_binary_sha256: None,
            },
        ];
        for (row, bytes) in rows
            .iter()
            .zip([model_bytes.as_slice(), tokenizer_bytes.as_slice()])
        {
            let path = root(temp.path()).join(row.filename);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let references = rows.iter().collect::<Vec<_>>();
        write_sidecar(temp.path(), &references).unwrap();
        let root = root(temp.path());
        prove_temp_sweep(&root, "model.onnx", "linux-cpu-x64");
        let paths = [root.join(MODEL), root.join(TOKENIZER), sidecar(temp.path())];
        let before = paths.clone().map(|path| fs::metadata(path).unwrap());
        install_rerank_model_with_rows(temp.path(), false, &DENY_ALL_POLICY, &references).unwrap();
        for (path, metadata) in paths.iter().zip(before) {
            let after = fs::metadata(path).unwrap();
            assert_eq!(after.ino(), metadata.ino());
            assert_eq!(after.mtime(), metadata.mtime());
        }
        assert!(leaked_temps(&root).is_empty());

        fs::write(root.join(TOKENIZER), vec![b'x'; tokenizer_bytes.len()]).unwrap();
        assert_eq!(
            check_rerank_model_with_rows(temp.path(), &references)
                .unwrap_err()
                .reason_code,
            "sha256_mismatch"
        );
        fs::write(root.join(TOKENIZER), tokenizer_bytes).unwrap();
        fs::write(root.join(MODEL), b"corrupt model").unwrap();
        assert_eq!(
            check_rerank_model_with_rows(temp.path(), &references)
                .unwrap_err()
                .reason_code,
            "size_mismatch"
        );
        fs::write(root.join(MODEL), model_bytes).unwrap();
        fs::write(root.join(TOKENIZER), b"corrupt tokenizer").unwrap();
        assert_eq!(
            check_rerank_model_with_rows(temp.path(), &references)
                .unwrap_err()
                .reason_code,
            "size_mismatch"
        );
    }
}
