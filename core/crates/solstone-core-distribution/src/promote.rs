// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::deb::{write_deb, DebMeta};
use crate::inspect::{write_sidecars, ReleaseInfo};
use crate::provenance::{require_clean, require_commit, require_lock, Provenance};
use crate::rpm::{write_rpm, RpmMeta};
use crate::stage::write_staged_file_mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteStep {
    Compile,
    Stage,
    Tar,
    Deb,
    Rpm,
    Checksums,
    Manifest,
    Revalidate,
    Rename,
}

impl PromoteStep {
    pub const ALL: [Self; 9] = [
        Self::Compile,
        Self::Stage,
        Self::Tar,
        Self::Deb,
        Self::Rpm,
        Self::Checksums,
        Self::Manifest,
        Self::Revalidate,
        Self::Rename,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compile => "compile",
            Self::Stage => "stage",
            Self::Tar => "tar",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Checksums => "checksums",
            Self::Manifest => "manifest",
            Self::Revalidate => "revalidate",
            Self::Rename => "rename",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromoteRequest {
    pub dest: PathBuf,
    pub work: PathBuf,
    pub tree: Vec<(String, Vec<u8>, u32)>,
    pub version: String,
    pub arch: String,
    pub deb_arch: String,
    pub rpm_arch: String,
    pub dirty: bool,
    pub observed: Provenance,
    pub expected: Provenance,
    pub fail_after: Option<String>,
}

#[derive(Debug)]
pub struct PromoteError {
    pub message: String,
}

impl PromoteError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PromoteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PromoteError {}

fn fail_after(request: &PromoteRequest) -> Option<String> {
    request
        .fail_after
        .clone()
        .or_else(|| env::var("SOLSTONE_DISTRIBUTION_FAIL_AFTER").ok())
}

fn checkpoint(request: &PromoteRequest, step: PromoteStep) -> Result<(), PromoteError> {
    if fail_after(request).as_deref() == Some(step.as_str()) {
        return Err(PromoteError::new(format!("injected-failure {}", step.as_str())));
    }
    Ok(())
}

#[must_use]
pub fn isolated_target_dir(work: &Path) -> PathBuf {
    work.join("distribution-target")
}

pub fn promote(request: &PromoteRequest) -> Result<PathBuf, PromoteError> {
    require_clean(request.dirty).map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Compile)?;

    let stage = request.work.join("stage");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).map_err(|error| PromoteError::new(error.to_string()))?;
    for (dest, bytes, mode) in &request.tree {
        write_staged_file_mode(&stage, dest, bytes, *mode)
            .map_err(|error| PromoteError::new(error.to_string()))?;
    }
    checkpoint(request, PromoteStep::Stage)?;

    let partial = request.work.join("out.partial");
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(&partial).map_err(|error| PromoteError::new(error.to_string()))?;
    crate::tar::write_tar_gz(&stage, &partial.join("tree.tar.gz"))
        .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Tar)?;
    write_deb(
        &stage,
        &partial.join("tree.deb"),
        DebMeta {
            version: &request.version,
            arch: &request.deb_arch,
        },
    )
    .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Deb)?;
    write_rpm(
        &stage,
        &partial.join("tree.rpm"),
        RpmMeta {
            version: &request.version,
            arch: &request.rpm_arch,
        },
    )
    .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Rpm)?;

    let release = ReleaseInfo {
        product: "solstone-journal",
        version: &request.version,
        target: &request.arch,
        commit: &request.expected.commit,
        lock_sha256: &request.expected.lock_sha256,
    };
    write_sidecars(&partial, &release).map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Checksums)?;
    checkpoint(request, PromoteStep::Manifest)?;

    require_commit(&request.expected.commit, &request.observed.commit)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    require_lock(
        &request.expected.lock_sha256,
        &request.observed.lock_sha256,
    )
    .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Revalidate)?;

    checkpoint(request, PromoteStep::Rename)?;
    if let Some(parent) = request.dest.parent() {
        fs::create_dir_all(parent).map_err(|error| PromoteError::new(error.to_string()))?;
    }
    if request.dest.exists() {
        let displaced = request.work.join("dest.displaced");
        let _ = fs::remove_dir_all(&displaced);
        fs::rename(&request.dest, &displaced)
            .map_err(|error| PromoteError::new(error.to_string()))?;
    }
    fs::rename(&partial, &request.dest).map_err(|error| PromoteError::new(error.to_string()))?;
    Ok(request.dest.clone())
}

pub fn snapshot_dir(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, PromoteError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let mut files = BTreeMap::new();
    collect(path, path, &mut files)?;
    Ok(files)
}

fn collect(
    root: &Path,
    dir: &Path,
    files: &mut std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), PromoteError> {
    for entry in fs::read_dir(dir).map_err(|error| PromoteError::new(error.to_string()))? {
        let entry = entry.map_err(|error| PromoteError::new(error.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| PromoteError::new(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        files.insert(
            relative,
            fs::read(&path).map_err(|error| PromoteError::new(error.to_string()))?,
        );
    }
    Ok(())
}
