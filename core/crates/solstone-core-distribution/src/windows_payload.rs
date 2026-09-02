// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed, complete Windows package-payload inventory.
//!
//! A controlled-build receipt identifies pre-signing dependency output.  This
//! module is the later package boundary: it verifies the installed tree against
//! one pinned-key manifest after package signing has produced its final bytes.
//! It deliberately does not install, sign, load, or otherwise execute a file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest_verify::verify_pinned_signature;

pub const WINDOWS_PAYLOAD_SCHEMA_V1: &str = "solstone.windows-installed-payload.v1";
pub const WINDOWS_PAYLOAD_TARGET: &str = "windows-x86_64";
pub const WINDOWS_PAYLOAD_MANIFEST: &str = "share/provenance/windows-payload.json";
pub const WINDOWS_PAYLOAD_SIGNATURE: &str = "share/provenance/windows-payload.json.minisig";
/// The CED engine is a signed application-directory payload, never mutable
/// journal state.
pub const WINDOWS_CED_LIBRARY: &str = "bin/ced.dll";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsPayloadFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsPayloadManifest {
    pub schema: String,
    pub target: String,
    pub source_commit: String,
    pub cargo_lock_sha256: String,
    pub files: Vec<WindowsPayloadFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    sha256: String,
    bytes: u64,
}

/// A complete verified snapshot of one installed payload tree.
///
/// The capability intentionally carries no loader operation.  Consumers may
/// request a declared path, but must establish their own platform-safe loading
/// semantics after this package identity check.
#[derive(Debug, Clone)]
pub struct VerifiedWindowsPayload {
    root: PathBuf,
    manifest: WindowsPayloadManifest,
}

impl VerifiedWindowsPayload {
    #[must_use]
    pub fn manifest(&self) -> &WindowsPayloadManifest {
        &self.manifest
    }

    /// Return a path only when it was part of the pinned, verified snapshot.
    #[must_use]
    pub fn declared_path(&self, path: &str) -> Option<PathBuf> {
        self.manifest
            .files
            .iter()
            .any(|file| file.path == path)
            .then(|| self.root.join(path))
    }

    /// Return the CED engine only when the verified package declared it.
    ///
    /// The caller still owns the platform-safe dynamic-load operation. This
    /// capability only binds that operation to the exact signed tree checked
    /// by [`verify_windows_payload`].
    pub fn ced_library_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_CED_LIBRARY).ok_or_else(|| {
            WindowsPayloadError::new(WindowsPayloadRefusal::MissingMember, WINDOWS_CED_LIBRARY)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPayloadRefusal {
    Missing,
    NotRegular,
    ReparseOrSymlink,
    UnsafePath,
    Manifest,
    Signature,
    Schema,
    Target,
    SourceCommit,
    LockDigest,
    Empty,
    UnsortedOrDuplicate,
    CaseCollision,
    Digest,
    Bytes,
    MissingMember,
    UnexpectedMember,
}

impl WindowsPayloadRefusal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::NotRegular => "not-regular",
            Self::ReparseOrSymlink => "reparse-or-symlink",
            Self::UnsafePath => "unsafe-path",
            Self::Manifest => "manifest",
            Self::Signature => "signature",
            Self::Schema => "schema",
            Self::Target => "target",
            Self::SourceCommit => "source-commit",
            Self::LockDigest => "cargo-lock-sha256",
            Self::Empty => "empty",
            Self::UnsortedOrDuplicate => "unsorted-or-duplicate",
            Self::CaseCollision => "case-collision",
            Self::Digest => "digest",
            Self::Bytes => "bytes",
            Self::MissingMember => "missing-member",
            Self::UnexpectedMember => "unexpected-member",
        }
    }
}

#[derive(Debug)]
pub struct WindowsPayloadError {
    pub kind: WindowsPayloadRefusal,
    pub detail: String,
}

impl WindowsPayloadError {
    fn new(kind: WindowsPayloadRefusal, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for WindowsPayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}\n  {}", self.kind.as_str(), self.detail)
    }
}

impl std::error::Error for WindowsPayloadError {}

/// Render the unsigned manifest which the package finalizer signs after every
/// package byte, including Authenticode-mutated PEs, is final.
pub fn render_windows_payload_manifest(
    root: &Path,
    source_commit: &str,
    cargo_lock_sha256: &str,
) -> Result<Vec<u8>, WindowsPayloadError> {
    require_commit(source_commit)?;
    require_digest(WindowsPayloadRefusal::LockDigest, cargo_lock_sha256)?;
    let actual = collect_payload_files(root)?;
    let files = actual
        .into_iter()
        .map(|(path, identity)| WindowsPayloadFile {
            path,
            sha256: identity.sha256,
            bytes: identity.bytes,
        })
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::Empty,
            "payload has no regular files",
        ));
    }
    let manifest = WindowsPayloadManifest {
        schema: WINDOWS_PAYLOAD_SCHEMA_V1.to_owned(),
        target: WINDOWS_PAYLOAD_TARGET.to_owned(),
        source_commit: source_commit.to_owned(),
        cargo_lock_sha256: cargo_lock_sha256.to_owned(),
        files,
    };
    validate_manifest(&manifest)?;
    serde_json::to_vec(&manifest).map_err(|error| {
        WindowsPayloadError::new(WindowsPayloadRefusal::Manifest, error.to_string())
    })
}

/// Verify the package's signed manifest and its exact complete file tree.
pub fn verify_windows_payload(root: &Path) -> Result<VerifiedWindowsPayload, WindowsPayloadError> {
    let signature_path = root.join(WINDOWS_PAYLOAD_SIGNATURE);
    let manifest_bytes = read_regular_at(root, WINDOWS_PAYLOAD_MANIFEST)?;
    let signature_bytes = read_regular_at(root, WINDOWS_PAYLOAD_SIGNATURE)?;
    verify_pinned_signature(&manifest_bytes, &signature_path, &signature_bytes).map_err(
        |error| WindowsPayloadError::new(WindowsPayloadRefusal::Signature, error.to_string()),
    )?;
    let manifest =
        serde_json::from_slice::<WindowsPayloadManifest>(&manifest_bytes).map_err(|error| {
            WindowsPayloadError::new(WindowsPayloadRefusal::Manifest, error.to_string())
        })?;
    validate_manifest(&manifest)?;
    let actual = collect_payload_files(root)?;
    let declared = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    for (path, identity) in &actual {
        let Some(file) = declared.get(path.as_str()) else {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::UnexpectedMember,
                path,
            ));
        };
        if file.bytes != identity.bytes {
            return Err(WindowsPayloadError::new(WindowsPayloadRefusal::Bytes, path));
        }
        if file.sha256 != identity.sha256 {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::Digest,
                path,
            ));
        }
    }
    for file in &manifest.files {
        if !actual.contains_key(&file.path) {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::MissingMember,
                &file.path,
            ));
        }
    }
    Ok(VerifiedWindowsPayload {
        root: root.to_path_buf(),
        manifest,
    })
}

fn validate_manifest(manifest: &WindowsPayloadManifest) -> Result<(), WindowsPayloadError> {
    if manifest.schema != WINDOWS_PAYLOAD_SCHEMA_V1 {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::Schema,
            &manifest.schema,
        ));
    }
    if manifest.target != WINDOWS_PAYLOAD_TARGET {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::Target,
            &manifest.target,
        ));
    }
    require_commit(&manifest.source_commit)?;
    require_digest(
        WindowsPayloadRefusal::LockDigest,
        &manifest.cargo_lock_sha256,
    )?;
    if manifest.files.is_empty() {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::Empty,
            "manifest files",
        ));
    }
    let mut previous = None;
    let mut folded = BTreeSet::new();
    for file in &manifest.files {
        validate_file_path(&file.path)?;
        if previous.is_some_and(|value: &str| value >= file.path.as_str()) {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::UnsortedOrDuplicate,
                &file.path,
            ));
        }
        previous = Some(file.path.as_str());
        if !folded.insert(file.path.to_ascii_lowercase()) {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::CaseCollision,
                &file.path,
            ));
        }
        require_digest(WindowsPayloadRefusal::Digest, &file.sha256)?;
        if file.bytes == 0 {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::Bytes,
                &file.path,
            ));
        }
    }
    Ok(())
}

fn require_commit(value: &str) -> Result<(), WindowsPayloadError> {
    if [40, 64].contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::SourceCommit,
            value,
        ))
    }
}

fn require_digest(kind: WindowsPayloadRefusal, value: &str) -> Result<(), WindowsPayloadError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(WindowsPayloadError::new(kind, value))
    }
}

fn validate_file_path(path: &str) -> Result<(), WindowsPayloadError> {
    if path == WINDOWS_PAYLOAD_MANIFEST || path == WINDOWS_PAYLOAD_SIGNATURE {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::UnsafePath,
            path,
        ));
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.to_string_lossy().contains('\\')
    {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::UnsafePath,
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn read_regular_at(root: &Path, relative: &str) -> Result<Vec<u8>, WindowsPayloadError> {
    validate_reserved_path(relative)?;
    let path = contained_path(root, relative)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        WindowsPayloadError::new(
            WindowsPayloadRefusal::Missing,
            format!("{relative}: {error}"),
        )
    })?;
    if is_link_or_reparse(&metadata) {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::ReparseOrSymlink,
            relative,
        ));
    }
    if !metadata.is_file() {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::NotRegular,
            relative,
        ));
    }
    fs::read(&path).map_err(|error| {
        WindowsPayloadError::new(
            WindowsPayloadRefusal::Missing,
            format!("{relative}: {error}"),
        )
    })
}

fn validate_reserved_path(path: &str) -> Result<(), WindowsPayloadError> {
    if path == WINDOWS_PAYLOAD_MANIFEST || path == WINDOWS_PAYLOAD_SIGNATURE {
        Ok(())
    } else {
        Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::UnsafePath,
            path,
        ))
    }
}

fn contained_path(root: &Path, relative: &str) -> Result<PathBuf, WindowsPayloadError> {
    let root_meta = fs::symlink_metadata(root).map_err(|error| {
        WindowsPayloadError::new(
            WindowsPayloadRefusal::Missing,
            format!("{}: {error}", root.display()),
        )
    })?;
    if is_link_or_reparse(&root_meta) {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::ReparseOrSymlink,
            root.display().to_string(),
        ));
    }
    if !root_meta.is_dir() {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::NotRegular,
            root.display().to_string(),
        ));
    }
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::UnsafePath,
                relative,
            ));
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            WindowsPayloadError::new(
                WindowsPayloadRefusal::Missing,
                format!("{relative}: {error}"),
            )
        })?;
        if is_link_or_reparse(&metadata) {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::ReparseOrSymlink,
                relative,
            ));
        }
    }
    Ok(path)
}

fn collect_payload_files(
    root: &Path,
) -> Result<BTreeMap<String, FileIdentity>, WindowsPayloadError> {
    let root_meta = fs::symlink_metadata(root).map_err(|error| {
        WindowsPayloadError::new(
            WindowsPayloadRefusal::Missing,
            format!("{}: {error}", root.display()),
        )
    })?;
    if is_link_or_reparse(&root_meta) {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::ReparseOrSymlink,
            root.display().to_string(),
        ));
    }
    if !root_meta.is_dir() {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::NotRegular,
            root.display().to_string(),
        ));
    }
    let mut files = BTreeMap::new();
    collect_directory(root, root, &mut files)?;
    Ok(files)
}

fn collect_directory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, FileIdentity>,
) -> Result<(), WindowsPayloadError> {
    for entry in fs::read_dir(directory).map_err(|error| {
        WindowsPayloadError::new(
            WindowsPayloadRefusal::Missing,
            format!("{}: {error}", directory.display()),
        )
    })? {
        let entry = entry.map_err(|error| {
            WindowsPayloadError::new(WindowsPayloadRefusal::Missing, error.to_string())
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            WindowsPayloadError::new(
                WindowsPayloadRefusal::Missing,
                format!("{}: {error}", path.display()),
            )
        })?;
        let relative = path
            .strip_prefix(root)
            .expect("recursive path is rooted")
            .to_str()
            .ok_or_else(|| {
                WindowsPayloadError::new(
                    WindowsPayloadRefusal::UnsafePath,
                    path.display().to_string(),
                )
            })?;
        #[cfg(not(windows))]
        if relative.contains('\\') {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::UnsafePath,
                relative,
            ));
        }
        let relative = relative.replace('\\', "/");
        if is_link_or_reparse(&metadata) {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::ReparseOrSymlink,
                relative,
            ));
        }
        if metadata.is_dir() {
            collect_directory(root, &path, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::NotRegular,
                relative,
            ));
        }
        if relative == WINDOWS_PAYLOAD_MANIFEST || relative == WINDOWS_PAYLOAD_SIGNATURE {
            continue;
        }
        validate_file_path(&relative)?;
        let identity = file_identity(&path, &relative)?;
        if files.insert(relative.clone(), identity).is_some() {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::UnsortedOrDuplicate,
                relative,
            ));
        }
    }
    Ok(())
}

fn file_identity(path: &Path, relative: &str) -> Result<FileIdentity, WindowsPayloadError> {
    let mut file = fs::File::open(path).map_err(|error| {
        WindowsPayloadError::new(
            WindowsPayloadRefusal::Missing,
            format!("{relative}: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            WindowsPayloadError::new(
                WindowsPayloadRefusal::Missing,
                format!("{relative}: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| WindowsPayloadError::new(WindowsPayloadRefusal::Bytes, relative))?;
        hasher.update(&buffer[..read]);
    }
    Ok(FileIdentity {
        sha256: format!("{:x}", hasher.finalize()),
        bytes,
    })
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
