// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Import contract types; real contract operation signatures are defined by a later wave.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::sync_state::BackendName;

/// A preview request is distinct from a save request at the type boundary.
#[derive(Debug, Default)]
pub struct PreviewRequest;

/// A save request is distinct from a preview request at the type boundary.
#[derive(Debug, Default)]
pub struct SaveRequest;

/// Preview marker for sync operations.
#[derive(Debug, Default)]
pub struct SyncPreviewRequest;

/// Save marker for sync operations.
#[derive(Debug, Default)]
pub struct SyncSaveRequest;

/// The reference audio auto mode.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AudioAuto {
    Enabled,
    Disabled,
    Value(String),
}

/// A selected sync backend request. Each variant owns only its accepted parameters.
///
/// Python-side variants cannot receive the native window parameter:
///
/// ```compile_fail,E0559
/// use solstone_core_import::SyncBackendRequest;
/// use std::path::PathBuf;
///
/// let _ = SyncBackendRequest::Plaud {
///     journal_root: PathBuf::from("journal"),
///     save: false,
///     window_days: 7,
/// };
/// ```
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SyncBackendRequest {
    Plaud {
        journal_root: PathBuf,
        save: bool,
    },
    Obsidian {
        journal_root: PathBuf,
        save: bool,
        source_path: Option<PathBuf>,
        force: bool,
    },
    Audio {
        journal_root: PathBuf,
        save: bool,
        source_path: Option<PathBuf>,
        force: bool,
        auto: AudioAuto,
    },
    Oura {
        journal_root: PathBuf,
        save: bool,
        window_days: u32,
        confirmed: bool,
        scheduled: bool,
    },
}

impl SyncBackendRequest {
    #[must_use]
    pub const fn backend(&self) -> BackendName {
        match self {
            Self::Plaud { .. } => BackendName::Plaud,
            Self::Obsidian { .. } => BackendName::Obsidian,
            Self::Audio { .. } => BackendName::Audio,
            Self::Oura { .. } => BackendName::Oura,
        }
    }
}

/// Optional human-readable scheduling guidance returned by a backend.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SyncGuidance {
    text: String,
}

impl SyncGuidance {
    #[must_use]
    pub fn new(text: String) -> Self {
        Self { text }
    }

    #[must_use]
    pub fn format_text(&self) -> &str {
        &self.text
    }
}

/// Preview data returned without saving an import.
#[derive(Debug, Eq, PartialEq)]
pub struct ImportPreview {
    pub date_range: (String, String),
    pub item_count: u64,
    pub entity_count: u64,
    pub summary: String,
}

impl ImportPreview {
    pub const FIELD_NAMES: [&str; 4] = ["date_range", "item_count", "entity_count", "summary"];
}

/// Saved import outcome fields retained for the native contract.
#[derive(Debug, Eq, PartialEq)]
pub struct ImportResult {
    pub entries_written: u64,
    pub entities_seeded: u64,
    pub files_created: Vec<String>,
    pub errors: Vec<String>,
    pub summary: String,
    pub hard_failures: Vec<String>,
    pub segments: Option<Vec<(String, String)>>,
    pub date_range: Option<(String, String)>,
    pub merge_summary: Option<String>,
    pub principal_collision: Option<String>,
    pub merge_log_path: Option<String>,
    pub merge_staging_path: Option<String>,
    pub raw_retention: Option<String>,
}

impl ImportResult {
    pub const FIELD_NAMES: [&str; 13] = [
        "entries_written",
        "entities_seeded",
        "files_created",
        "errors",
        "summary",
        "hard_failures",
        "segments",
        "date_range",
        "merge_summary",
        "principal_collision",
        "merge_log_path",
        "merge_staging_path",
        "raw_retention",
    ];
}

/// True when the import manifest should be written rather than suppressed.
#[must_use]
pub fn should_write_manifest(result: &ImportResult) -> bool {
    result.entries_written > 0 || result.hard_failures.is_empty()
}

/// Opaque source identity. It is not required to be a plain digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceHash(String);

impl SourceHash {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// A read-only capability for an owner-controlled source tree.
///
/// Its private field prevents direct construction outside this crate:
///
/// ```compile_fail,E0451
/// use solstone_core_import::OwnerSource;
/// use std::path::Path;
///
/// let _ = OwnerSource { root: Path::new(".") };
/// ```
///
/// It does not implement `AsRef<Path>`:
///
/// ```compile_fail,E0277
/// use solstone_core_import::OwnerSource;
/// use std::path::Path;
///
/// fn needs_path<T: AsRef<Path>>(_: T) {}
/// needs_path(OwnerSource::new(Path::new(".")));
/// ```
///
/// It has no `Default` implementation:
///
/// ```compile_fail,E0277
/// use solstone_core_import::OwnerSource;
///
/// fn needs_default<T: Default>() {}
/// needs_default::<OwnerSource<'static>>();
/// ```
///
/// It has no method that removes source material:
///
/// ```compile_fail,E0599
/// use solstone_core_import::OwnerSource;
/// use std::path::Path;
///
/// OwnerSource::new(Path::new(".")).remove_file();
/// ```
///
/// A valid caller can obtain aggregate metadata without receiving filesystem authority:
///
/// ```
/// use solstone_core_import::OwnerSource;
/// use std::path::Path;
///
/// let source = OwnerSource::new(Path::new("."));
/// let metadata = source.metadata().expect("current directory is readable");
/// assert!(metadata.entry_count() > 0);
/// ```
pub struct OwnerSource<'a> {
    root: &'a Path,
}

impl<'a> OwnerSource<'a> {
    #[must_use]
    pub fn new(root: &'a Path) -> Self {
        Self { root }
    }

    pub fn metadata(&self) -> io::Result<OwnerSourceMetadata> {
        let fingerprint = fingerprint_tree(self.root)?;
        let entry_count = u64::try_from(fingerprint.entries.len()).expect("entry count fits u64");
        let total_bytes = fingerprint
            .entries
            .iter()
            .map(|entry| u64::try_from(entry.bytes.len()).expect("byte count fits u64"))
            .sum();
        Ok(OwnerSourceMetadata {
            entry_count,
            total_bytes,
        })
    }
}

/// Aggregate metadata exposed by [`OwnerSource`].
#[derive(Debug, Eq, PartialEq)]
pub struct OwnerSourceMetadata {
    entry_count: u64,
    total_bytes: u64,
}

impl OwnerSourceMetadata {
    #[must_use]
    pub const fn entry_count(&self) -> u64 {
        self.entry_count
    }

    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// Before/after source observations produced by [`observe_source_immutability`].
#[derive(Debug, Eq, PartialEq)]
pub struct SourceImmutabilityReport {
    before: SourceTreeFingerprint,
    after: SourceTreeFingerprint,
}

impl SourceImmutabilityReport {
    #[must_use]
    pub fn violated(&self) -> bool {
        self.before != self.after
    }
}

/// Observe whether an action changes an owner-controlled source tree.
pub fn observe_source_immutability(
    root: &Path,
    action: impl FnOnce(&OwnerSource<'_>),
) -> io::Result<SourceImmutabilityReport> {
    let before = fingerprint_tree(root)?;
    let source = OwnerSource::new(root);
    action(&source);
    let after = fingerprint_tree(root)?;
    Ok(SourceImmutabilityReport { before, after })
}

#[derive(Debug, Eq, PartialEq)]
struct SourceTreeFingerprint {
    entries: Vec<SourceEntryFingerprint>,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceEntryFingerprint {
    relative_path: String,
    kind: SourceEntryKind,
    bytes: Vec<u8>,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SourceEntryKind {
    Directory,
    File,
    Symlink,
}

fn fingerprint_tree(root: &Path) -> io::Result<SourceTreeFingerprint> {
    let mut entries = Vec::new();
    collect_entries(root, root, &mut entries)?;
    entries.sort();
    Ok(SourceTreeFingerprint { entries })
}

fn collect_entries(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<SourceEntryFingerprint>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative_path = path
            .strip_prefix(root)
            .expect("walked path remains under root")
            .to_string_lossy()
            .into_owned();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            entries.push(SourceEntryFingerprint {
                relative_path,
                kind: SourceEntryKind::Directory,
                bytes: Vec::new(),
            });
            collect_entries(root, &path, entries)?;
        } else if file_type.is_file() {
            entries.push(SourceEntryFingerprint {
                relative_path,
                kind: SourceEntryKind::File,
                bytes: fs::read(path)?,
            });
        } else if file_type.is_symlink() {
            entries.push(SourceEntryFingerprint {
                relative_path,
                kind: SourceEntryKind::Symlink,
                bytes: fs::read_link(path)?.to_string_lossy().as_bytes().to_vec(),
            });
        }
    }
    Ok(())
}
