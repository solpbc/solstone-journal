// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Selector-gated installation of the pinned MLX model generation.

use super::{DispatchError, failure, fetch_install_member, mlx_candidate, pins, status};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn install(
    journal: &Path,
    state: &mut status::InstallStatus,
) -> Result<Value, DispatchError> {
    let helper = mlx_candidate::resolve_mlx_helper(None)
        .map_err(|e| failure("package", "package_unavailable", e, 65))?;
    if std::env::var_os(mlx_candidate::MODEL_DIR_ENV_KEY).is_some() {
        let proof = mlx_candidate::inspect_verified_default(journal);
        if proof["ready"] != true {
            return Err(failure(
                "verification",
                "artifact_stale",
                proof["detail"]
                    .as_str()
                    .unwrap_or("MLX override verification failed"),
                65,
            ));
        }
        return Ok(
            json!({"backend":"mlx", "binary_path":helper, "model_path":mlx_candidate::model_directory(journal)}),
        );
    }
    let rows: Vec<_> = solstone_core_assets::catalog()
        .iter()
        .filter(|a| a.unit == "local-model-mlx")
        .collect();
    let generation = pins::mlx_model_generation();
    let root = install_generation(
        journal,
        state,
        &generation,
        &rows,
        |state, row, destination, completed, total| {
            fetch_install_member(
                journal,
                state,
                row,
                destination,
                completed,
                total,
                "model_download_failed",
            )
        },
    )?;
    Ok(json!({"backend":"mlx", "binary_path":helper, "model_path":root}))
}

fn install_generation(
    journal: &Path,
    state: &mut status::InstallStatus,
    generation: &solstone_core_artifact_download::generation::Generation,
    rows: &[&solstone_core_assets::Artifact],
    mut fetch: impl FnMut(
        &mut status::InstallStatus,
        &solstone_core_assets::Artifact,
        &Path,
        u64,
        u64,
    ) -> Result<(), DispatchError>,
) -> Result<std::path::PathBuf, DispatchError> {
    let root = generation.directory(&pins::cache_root(journal).join("models"));
    let fetch_root = pins::cache_root(journal)
        .join("downloads/local-model-mlx")
        .join(generation.key());
    let _write = solstone_core_artifact_download::generation::hold_write(&root)
        .map_err(|e| failure("state", "model_in_use", e, 74))?;
    fs::create_dir_all(&fetch_root)
        .and_then(|_| fs::create_dir_all(&root))
        .map_err(|e| failure("io", "model_dir_create_failed", e, 74))?;
    let total = rows.iter().map(|a| a.size_bytes).sum();
    *state = status::write_status(
        journal,
        status::transition(state.clone(), "downloading", None, None)
            .map_err(|e| failure("state", "transition_failed", e, 74))?,
    )
    .map_err(|e| failure("state", "status_write_failed", e, 74))?;
    let mut completed = 0;
    for row in rows {
        status::assert_current(journal, state)
            .map_err(|e| failure("state", "attempt_superseded", e, 74))?;
        let dest = root.join(row.filename);
        if solstone_core_artifact_download::verify_sha256(&dest, row.sha256).is_err() {
            let fetched = fetch_root.join(row.filename);
            super::retry_transient_fetch(
                state,
                |state| fetch(state, row, &fetched, completed, total),
                |state| {
                    status::assert_current(journal, state)
                        .map(|_| ())
                        .map_err(|e| failure("state", "attempt_superseded", e, 74))
                },
                &super::MODEL_FETCH_RETRY_WAITS,
                std::thread::sleep,
            )?;
            solstone_core_journal_io::install_retained_file(
                &fetched,
                &dest,
                solstone_core_journal_io::AtomicWriteOptions::default(),
            )
            .map_err(|e| failure("io", "model_publish_failed", e, 74))?;
        }
        completed += row.size_bytes;
    }
    state.progress_bytes_received = Some(completed);
    state.progress_bytes_total = Some(total);
    *state = status::write_status(
        journal,
        status::transition(state.clone(), "verifying", None, None)
            .map_err(|e| failure("state", "transition_failed", e, 74))?,
    )
    .map_err(|e| failure("state", "status_write_failed", e, 74))?;
    generation
        .seal(&root)
        .map_err(|e| failure("verification", "sha256_mismatch", e, 74))?;
    Ok(root)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use solstone_core_artifact_download::generation::{Generation, Member};
    use solstone_core_assets::{Artifact, Platform};

    #[test]
    fn partial_or_corrupt_model_never_seals_and_retry_preserves_gguf() {
        let journal = tempfile::tempdir().unwrap();
        let gguf = pins::cache_root(journal.path()).join("models/existing-gguf/weights.gguf");
        fs::create_dir_all(gguf.parent().unwrap()).unwrap();
        fs::write(&gguf, b"keep existing model").unwrap();
        let rows: Vec<Artifact> = [".gitattributes", "config.json"]
            .into_iter()
            .map(|name| Artifact {
                unit: "local-model-mlx",
                version: "fixture",
                filename: name,
                sha256: Box::leak(super::super::fingerprint::sha256(name).into_boxed_str()),
                size_bytes: name.len() as u64,
                upstream_url: "https://example.invalid/pinned-fixture",
                origin_key: "assets/local-model-mlx/fixture/input",
                artifact_key: None,
                platform: Some(Platform::MacosArm64),
                backend: None,
                extracted_binary_sha256: None,
            })
            .collect();
        let generation = Generation::new(
            "local-model-mlx",
            rows.iter()
                .map(|row| Member {
                    file: row.filename.into(),
                    sha256: row.sha256.into(),
                    size_bytes: row.size_bytes,
                })
                .collect(),
        )
        .unwrap();
        let root = generation.directory(&pins::cache_root(journal.path()).join("models"));
        let row_refs: Vec<_> = rows.iter().collect();
        let begin = || {
            status::begin_or_replace(
                journal.path(),
                "local",
                "{}".into(),
                "fixture".into(),
                None,
                "resolving",
                None,
            )
            .unwrap()
        };
        let mut state = begin();
        let mut count = 0;
        assert!(
            install_generation(
                journal.path(),
                &mut state,
                &generation,
                &row_refs,
                |_, row, dest, _, _| {
                    count += 1;
                    if count == 2 {
                        return Err(failure("fixture", "fixture_stop", "interrupted", 74));
                    }
                    fs::write(dest, row.filename.as_bytes()).unwrap();
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!generation.is_sealed(&root));
        let mut state = begin();
        assert!(
            install_generation(
                journal.path(),
                &mut state,
                &generation,
                &row_refs,
                |_, row, dest, _, _| {
                    fs::write(dest, vec![b'x'; row.size_bytes as usize]).unwrap();
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!generation.is_sealed(&root));
        let mut state = begin();
        install_generation(
            journal.path(),
            &mut state,
            &generation,
            &row_refs,
            |_, row, dest, _, _| {
                fs::write(dest, row.filename.as_bytes()).unwrap();
                Ok(())
            },
        )
        .unwrap();
        assert!(generation.is_sealed(&root));
        assert_eq!(fs::read(&gguf).unwrap(), b"keep existing model");
        fs::write(root.join(".unexpected"), b"extra").unwrap();
        assert!(!generation.is_sealed(&root));
    }
}
