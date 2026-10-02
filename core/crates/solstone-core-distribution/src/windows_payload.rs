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
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
#[cfg(not(windows))]
use sha2::{Digest, Sha256};

// CNG keeps member hashing fast on Windows CPUs without SHA extensions.
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_hash;

use crate::manifest_verify::verify_pinned_signature;

pub const WINDOWS_PAYLOAD_SCHEMA_V1: &str = "solstone.windows-installed-payload.v1";
pub const WINDOWS_PAYLOAD_TARGET: &str = "windows-x86_64";
pub const WINDOWS_PAYLOAD_MANIFEST: &str = "share/provenance/windows-payload.json";
pub const WINDOWS_PAYLOAD_SIGNATURE: &str = "share/provenance/windows-payload.json.minisig";
/// The CED engine is a signed application-directory payload, never mutable
/// journal state.
pub const WINDOWS_CED_WORKER: &str = "bin/solstone-core-ced-analyze.exe";
pub const WINDOWS_CED_LIBRARY: &str = "bin/ced.dll";
/// The PDFium engine is a signed private-library payload, never a system or
/// mutable-journal lookup.
pub const WINDOWS_PDFIUM_LIBRARY: &str = "lib/solstone-core-pdf/pdfium.dll";
/// The PDF worker is a signed package executable, never a sibling discovered
/// through an ambient search path.
pub const WINDOWS_PDFIUM_WORKER: &str = "bin/solstone-core-pdf.exe";
/// The speaker helper is a signed package executable, never an ambient sibling.
pub const WINDOWS_SPEAKERS_ANALYZE_WORKER: &str = "bin/solstone-core-speakers-analyze.exe";
/// The VAD helper is a signed package executable, never an ambient sibling.
pub const WINDOWS_VAD_ANALYZE_WORKER: &str = "bin/solstone-core-vad-analyze.exe";
/// The one shared ONNX Runtime DLL is a signed private package member.
pub const WINDOWS_ONNXRUNTIME_LIBRARY: &str = "lib/solstone-core-speakers-analyze/onnxruntime.dll";
/// The speaker embedding model is a signed package member.
pub const WINDOWS_WESPEAKER_MODEL: &str =
    "lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx";
/// The speaker segmentation model is a signed package member.
pub const WINDOWS_PYANNOTE_MODEL: &str =
    "lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx";
/// The VAD model is a signed package member.
pub const WINDOWS_SILERO_VAD_MODEL: &str = "lib/solstone_journal_models/assets/silero_vad_v6.onnx";
/// The Parakeet server is a signed package executable, never an owner-installed service.
pub const WINDOWS_PARAKEET_SERVER: &str = "bin/parakeet-server.exe";
/// The Parakeet model is a signed package member, never copied into journal state.
pub const WINDOWS_PARAKEET_MODEL: &str =
    "lib/solstone_journal_models/assets/parakeet/tdt-0.6b-v3-q8_0.gguf";
/// The backup tools are verified package executables, never runtime downloads.
pub const WINDOWS_RESTIC_WORKER: &str = "bin/restic.exe";
pub const WINDOWS_RCLONE_WORKER: &str = "bin/rclone.exe";
/// Required object detection uses the package engine and the bundled model.
pub const WINDOWS_RFDETR_WORKER: &str = "bin/rfdetr-cli.exe";
pub const WINDOWS_RFDETR_MODEL: &str =
    "lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf";
/// Vulkan observation uses the signed probe executable and loader DLL.
pub const WINDOWS_VULKAN_PROBE: &str = "bin/solstone-core-vulkan-probe.exe";
pub const WINDOWS_VULKAN_LOADER: &str = "bin/vulkan-1.dll";
/// Local thinking uses the signed package llama-server engine executable.
pub const WINDOWS_LLAMA_SERVER: &str = "bin/llama-server.exe";

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

/// One regular file found by the inventory walk, before any digest.
#[derive(Debug)]
struct InstalledMember {
    path: PathBuf,
    bytes: u64,
}

/// One installed payload tree whose signed manifest and complete inventory
/// have been admitted.
///
/// Admission checks the manifest signature, every member's path, size and
/// reparse state, the absence of unexpected or missing members, and the digest
/// of every DLL, since the loader can map any of them implicitly. Every other
/// member's digest is checked when a caller asks for its path, so a process
/// hashes what it is about to use rather than the whole package. Checked
/// digests are remembered only for the life of this value.
///
/// The capability intentionally carries no loader operation.  Consumers may
/// request a declared path, but must establish their own platform-safe loading
/// semantics after this package identity check.
#[derive(Debug, Clone)]
pub struct VerifiedWindowsPayload {
    root: PathBuf,
    manifest: WindowsPayloadManifest,
    checked: Arc<Mutex<BTreeSet<String>>>,
}

impl VerifiedWindowsPayload {
    #[must_use]
    pub fn manifest(&self) -> &WindowsPayloadManifest {
        &self.manifest
    }

    /// Return a member's path only when the signed manifest declares it and
    /// the installed file still has the declared size and digest.
    pub fn declared_path(&self, path: &str) -> Result<PathBuf, WindowsPayloadError> {
        let file = self
            .manifest
            .files
            .iter()
            .find(|file| file.path == path)
            .ok_or_else(|| WindowsPayloadError::new(WindowsPayloadRefusal::MissingMember, path))?;
        let mut checked = self
            .checked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !checked.contains(path) {
            check_member_digest(&self.root, file)?;
            checked.insert(path.to_owned());
        }
        Ok(self.root.join(path))
    }

    /// Return the CED engine only when the verified package declares it.
    pub fn ced_library_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_CED_LIBRARY)
    }

    /// Return the PDFium engine only when the verified package declares it.
    pub fn pdfium_library_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_PDFIUM_LIBRARY)
    }

    /// Return the PDF worker only when the verified package declares it.
    pub fn pdfium_worker_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_PDFIUM_WORKER)
    }

    /// Return the speaker helper only when the verified package declares it.
    pub fn speakers_analyze_worker_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_SPEAKERS_ANALYZE_WORKER)
    }

    /// Return the VAD helper only when the verified package declares it.
    pub fn vad_analyze_worker_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_VAD_ANALYZE_WORKER)
    }

    /// Return the shared ONNX Runtime DLL only when the verified package declares it.
    pub fn onnxruntime_library_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_ONNXRUNTIME_LIBRARY)
    }

    /// Return the speaker embedding model only when the verified package declares it.
    pub fn wespeaker_model_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_WESPEAKER_MODEL)
    }

    /// Return the speaker segmentation model only when the verified package declares it.
    pub fn pyannote_model_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_PYANNOTE_MODEL)
    }

    /// Return the VAD model only when the verified package declares it.
    pub fn silero_vad_model_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_SILERO_VAD_MODEL)
    }

    /// Return the Parakeet server only when the verified package declares it.
    pub fn parakeet_server_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_PARAKEET_SERVER)
    }

    /// Return the Parakeet model only when the verified package declares it.
    pub fn parakeet_model_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_PARAKEET_MODEL)
    }

    /// Return the Vulkan probe executable only when the verified package declares it.
    pub fn vulkan_probe_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_VULKAN_PROBE)
    }

    /// Return the Vulkan loader DLL only when the verified package declares it.
    pub fn vulkan_loader_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_VULKAN_LOADER)
    }

    /// Return the llama-server engine executable only when the verified package declares it.
    pub fn llama_server_path(&self) -> Result<PathBuf, WindowsPayloadError> {
        self.declared_path(WINDOWS_LLAMA_SERVER)
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
    let files = collect_payload_files(root)?
        .into_iter()
        .map(|(path, member)| {
            let identity = file_identity(&member.path, &path)?;
            Ok(WindowsPayloadFile {
                path,
                sha256: identity.sha256,
                bytes: identity.bytes,
            })
        })
        .collect::<Result<Vec<_>, WindowsPayloadError>>()?;
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

/// Results shared by every `verify_windows_payload` call made while a
/// [`SharedVerification`] guard is alive. `None` means no scope is open and
/// every call verifies the tree from scratch.
static SHARED_VERIFICATION: Mutex<Option<BTreeMap<PathBuf, VerifiedWindowsPayload>>> =
    Mutex::new(None);

/// Scope in which one bounded operation, such as a single diagnostics run,
/// verifies each package tree once instead of once per check.
///
/// A run that makes several readiness checks otherwise re-walks the package
/// and re-hashes the same members for each. The sharing ends when the guard
/// drops, so nothing is remembered across operations, and only successful
/// results are shared.
pub struct SharedVerification {
    owner: bool,
}

/// Open a [`SharedVerification`] scope. A nested call joins the open scope and
/// leaves it open when it drops.
#[must_use]
pub fn share_verification() -> SharedVerification {
    let mut state = SHARED_VERIFICATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let owner = state.is_none();
    if owner {
        *state = Some(BTreeMap::new());
    }
    SharedVerification { owner }
}

impl Drop for SharedVerification {
    fn drop(&mut self) {
        if self.owner {
            *SHARED_VERIFICATION
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }
}

fn verified_once(
    root: &Path,
    verify: impl FnOnce() -> Result<VerifiedWindowsPayload, WindowsPayloadError>,
) -> Result<VerifiedWindowsPayload, WindowsPayloadError> {
    {
        let state = SHARED_VERIFICATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(shared) = state.as_ref().and_then(|scope| scope.get(root)) {
            return Ok(shared.clone());
        }
    }
    let verified = verify()?;
    if let Some(scope) = SHARED_VERIFICATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        scope.insert(root.to_path_buf(), verified.clone());
    }
    Ok(verified)
}

/// Admit the package's signed manifest and its exact complete file tree.
///
/// Member digests other than DLLs are checked by
/// [`VerifiedWindowsPayload::declared_path`] when a caller asks for them.
pub fn verify_windows_payload(root: &Path) -> Result<VerifiedWindowsPayload, WindowsPayloadError> {
    verified_once(root, || verify_windows_payload_uncached(root))
}

fn verify_windows_payload_uncached(
    root: &Path,
) -> Result<VerifiedWindowsPayload, WindowsPayloadError> {
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
    for (path, member) in &actual {
        let Some(file) = declared.get(path.as_str()) else {
            return Err(WindowsPayloadError::new(
                WindowsPayloadRefusal::UnexpectedMember,
                path,
            ));
        };
        if file.bytes != member.bytes {
            return Err(WindowsPayloadError::new(WindowsPayloadRefusal::Bytes, path));
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
    let payload = VerifiedWindowsPayload {
        root: root.to_path_buf(),
        manifest,
        checked: Arc::default(),
    };
    for file in &payload.manifest.files {
        if is_library(&file.path) {
            payload.declared_path(&file.path)?;
        }
    }
    Ok(payload)
}

fn is_library(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
}

fn check_member_digest(root: &Path, file: &WindowsPayloadFile) -> Result<(), WindowsPayloadError> {
    let path = contained_path(root, &file.path)?;
    let identity = file_identity(&path, &file.path)?;
    if identity.bytes != file.bytes {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::Bytes,
            &file.path,
        ));
    }
    if identity.sha256 != file.sha256 {
        return Err(WindowsPayloadError::new(
            WindowsPayloadRefusal::Digest,
            &file.path,
        ));
    }
    Ok(())
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
) -> Result<BTreeMap<String, InstalledMember>, WindowsPayloadError> {
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
    files: &mut BTreeMap<String, InstalledMember>,
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
        let member = InstalledMember {
            path,
            bytes: metadata.len(),
        };
        if files.insert(relative.clone(), member).is_some() {
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
    #[cfg(not(windows))]
    let mut hasher = Sha256::new();
    #[cfg(windows)]
    let hash_error = |error: std::io::Error| {
        WindowsPayloadError::new(
            WindowsPayloadRefusal::Digest,
            format!("{relative}: {error}"),
        )
    };
    #[cfg(windows)]
    let mut hasher = native_hash::Sha256::new().map_err(hash_error)?;
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
        #[cfg(not(windows))]
        hasher.update(&buffer[..read]);
        #[cfg(windows)]
        hasher.update(&buffer[..read]).map_err(hash_error)?;
    }
    #[cfg(not(windows))]
    let sha256 = format!("{:x}", hasher.finalize());
    #[cfg(windows)]
    let sha256 = hasher
        .finish()
        .map_err(hash_error)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(FileIdentity { sha256, bytes })
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

#[cfg(test)]
mod shared_verification_tests {
    use super::*;
    use std::cell::Cell;

    fn payload(root: &Path) -> VerifiedWindowsPayload {
        VerifiedWindowsPayload {
            root: root.to_path_buf(),
            manifest: WindowsPayloadManifest {
                schema: WINDOWS_PAYLOAD_SCHEMA_V1.to_owned(),
                target: WINDOWS_PAYLOAD_TARGET.to_owned(),
                source_commit: "0".repeat(40),
                cargo_lock_sha256: "0".repeat(64),
                files: Vec::new(),
            },
            checked: Arc::default(),
        }
    }

    // One test, because the scope is process-wide and tests run in parallel.
    #[test]
    fn a_scope_verifies_each_tree_once_and_ends_with_its_guard() {
        let root = Path::new("/package/a");
        let other = Path::new("/package/b");
        let runs = Cell::new(0);
        let count = |path: &Path| {
            runs.set(runs.get() + 1);
            Ok(payload(path))
        };

        verified_once(root, || count(root)).unwrap();
        verified_once(root, || count(root)).unwrap();
        assert_eq!(runs.get(), 2, "no scope: every call verifies");

        {
            let _scope = share_verification();
            verified_once(root, || count(root)).unwrap();
            verified_once(root, || count(root)).unwrap();
            assert_eq!(runs.get(), 3, "scope: second call reuses the first");
            {
                let _joined = share_verification();
                verified_once(root, || count(root)).unwrap();
            }
            verified_once(root, || count(root)).unwrap();
            assert_eq!(runs.get(), 3, "a nested guard does not end the scope");
            verified_once(other, || count(other)).unwrap();
            assert_eq!(runs.get(), 4, "a different tree is verified separately");

            let failing = Path::new("/package/bad");
            let refuse = || {
                runs.set(runs.get() + 1);
                Err(WindowsPayloadError::new(WindowsPayloadRefusal::Digest, "x"))
            };
            assert!(verified_once(failing, refuse).is_err());
            assert!(verified_once(failing, refuse).is_err());
            assert_eq!(runs.get(), 6, "a refusal is never shared");
        }

        verified_once(root, || count(root)).unwrap();
        assert_eq!(runs.get(), 7, "the scope ended with its guard");
    }
}
