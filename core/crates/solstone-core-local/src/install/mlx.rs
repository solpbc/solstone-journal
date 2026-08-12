// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! MLX snapshot acquisition deliberately has a local-source seam.  Production
//! fetching can supply a populated snapshot directory; tests never need Hugging Face.

use serde_json::{Map, Value, json};
use solstone_core_assets::Artifact;
use std::fs;
use std::path::Path;

use super::archive::{self, DownloadHostPolicy, verify_sha256};

pub trait SnapshotSource {
    fn populate(&self, destination: &Path) -> Result<(), String>;
}

/// The registry rows that make up one MLX snapshot.
///
/// Snapshot filenames are unique only within a repository, so the join is on
/// `artifact_key` (the source repo) AND `version` (the revision), never on
/// filename alone.
pub fn snapshot_objects(repo: &str, revision: &str) -> Vec<&'static Artifact> {
    solstone_core_assets::catalog()
        .iter()
        .filter(|artifact| {
            artifact.unit == "mlx-snapshot"
                && artifact.artifact_key == Some(repo)
                && artifact.version == revision
        })
        .collect()
}

/// The production `SnapshotSource`: every object comes from sol pbc's own
/// origin, one registry row at a time.
///
/// This is the whole point of the type. The Python path calls
/// `huggingface_hub.snapshot_download`, which tells a model hub that an owner's
/// journal exists; the covenant in Article 8 forbids that. Routing through
/// `download_verified` means the single-element host allowlist and the pinned
/// digest both apply per object, and there is no upstream host to fall back to.
pub struct OriginSnapshotSource<'a> {
    pub repo: &'a str,
    pub revision: &'a str,
    pub policy: &'a DownloadHostPolicy<'a>,
}

impl SnapshotSource for OriginSnapshotSource<'_> {
    fn populate(&self, destination: &Path) -> Result<(), String> {
        let objects = snapshot_objects(self.repo, self.revision);
        // ⛔ Fail closed. An unknown repo/revision must not yield an empty
        // directory that later reads as a successfully-populated snapshot --
        // that is indistinguishable from a model with no files.
        if objects.is_empty() {
            return Err(format!(
                "no registry objects for mlx snapshot {}@{}",
                self.repo, self.revision
            ));
        }
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        for artifact in objects {
            archive::download_verified(
                artifact,
                &destination.join(artifact.filename),
                self.policy,
                |_, _| {},
            )
            .map_err(|error| format!("fetch {}@{}: {error}", self.repo, artifact.filename))?;
        }
        Ok(())
    }
}

pub fn validate_snapshot_sha256(root: &Path, hashes: &Map<String, Value>) -> Result<(), String> {
    for (relative, expected) in hashes {
        verify_sha256(
            &root.join(relative),
            expected.as_str().ok_or("hash must be a string")?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn create_gemma4_variant(source: &Path, destination: &Path) -> Result<Value, String> {
    if destination.exists() {
        fs::remove_dir_all(destination).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(entry.path(), &target).map_err(|error| error.to_string())?;
        #[cfg(not(unix))]
        fs::copy(entry.path(), &target).map_err(|error| error.to_string())?;
    }
    for name in ["config.json", "processor_config.json"] {
        let target = destination.join(name);
        if target.exists() {
            let source_file =
                fs::read_to_string(source.join(name)).map_err(|error| error.to_string())?;
            let mut value: Value =
                serde_json::from_str(&source_file).map_err(|error| error.to_string())?;
            if let Some(object) = value.as_object_mut() {
                object.insert("max_position_embeddings".to_owned(), Value::from(10240));
            }
            let _ = fs::remove_file(&target);
            fs::write(
                target,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
                ),
            )
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(
        json!({"variant_path":destination,"rewritten_files":["config.json","processor_config.json"]}),
    )
}
