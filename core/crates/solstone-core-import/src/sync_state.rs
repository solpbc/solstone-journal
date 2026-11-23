// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable sync-state inventory and private atomic state publication.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};

/// One backend recognised by the sync library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendName {
    Plaud,
    Oura,
}

impl BackendName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plaud => "plaud",
            Self::Oura => "oura",
        }
    }
}

/// The fixture-defined backend presentation order.
pub const SYNC_BACKEND_INVENTORY: [BackendName; 2] = [BackendName::Plaud, BackendName::Oura];

/// A loss-preserving sync-state JSON document.
///
/// Keeping the root as an ordered JSON object preserves every known and unknown
/// root and per-file member while backend orchestration changes only its own keys.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncState {
    backend: BackendName,
    root: Map<String, Value>,
}

impl SyncState {
    #[must_use]
    pub fn empty(backend: BackendName) -> Self {
        let mut root = Map::new();
        root.insert(
            "backend".to_owned(),
            Value::String(backend.as_str().to_owned()),
        );
        root.insert("files".to_owned(), Value::Object(Map::new()));
        Self { backend, root }
    }

    #[must_use]
    pub const fn backend(&self) -> BackendName {
        self.backend
    }

    #[must_use]
    pub fn root(&self) -> &Map<String, Value> {
        &self.root
    }

    #[must_use]
    pub fn root_mut(&mut self) -> &mut Map<String, Value> {
        &mut self.root
    }

    #[must_use]
    pub fn files_mut(&mut self) -> &mut Map<String, Value> {
        let files = self
            .root
            .entry("files".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !files.is_object() {
            *files = Value::Object(Map::new());
        }
        files
            .as_object_mut()
            .expect("files was normalized to object")
    }

    fn from_value(backend: BackendName, value: Value) -> Result<Self, SyncStateReadClass> {
        let root = value
            .as_object()
            .cloned()
            .ok_or(SyncStateReadClass::WrongShape)?;
        match root.get("files") {
            None | Some(Value::Object(_)) => {
                validate_known_numbers(backend, &root)?;
                Ok(Self { backend, root })
            }
            Some(_) => Err(SyncStateReadClass::WrongShape),
        }
    }
}

/// Why a sync-state document starts a benign re-catalogue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncStateReadClass {
    Io,
    MalformedJson,
    WrongShape,
    Overflow,
}

/// A sync-state read is never a blocking error.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncStateRead {
    Absent,
    Unreadable { class: SyncStateReadClass },
    Loaded(SyncState),
}

/// A named failure to publish sync state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncStateWriteError {
    message: String,
}

impl fmt::Display for SyncStateWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SyncStateWriteError {}

/// Load one backend state document.
///
/// Unlike consent artifacts, unreadable *sync* state is benign and re-catalogs.
#[must_use]
pub fn read_sync_state(root: &Path, backend: BackendName) -> SyncStateRead {
    let path = state_path(root, backend);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return SyncStateRead::Absent,
        Err(_) => {
            return SyncStateRead::Unreadable {
                class: SyncStateReadClass::Io,
            };
        }
    };
    let value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return SyncStateRead::Unreadable {
                class: SyncStateReadClass::MalformedJson,
            };
        }
    };
    match SyncState::from_value(backend, value) {
        Ok(state) => SyncStateRead::Loaded(state),
        Err(class) => SyncStateRead::Unreadable { class },
    }
}

/// Atomically publish state with the reference private modes.
pub fn write_sync_state(root: &Path, state: &SyncState) -> Result<(), SyncStateWriteError> {
    let imports = root.join("imports");
    create_directory_with_mode(&imports, 0o700).map_err(path_error)?;
    let bytes = serialize_state(&state.root).map_err(|message| SyncStateWriteError { message })?;
    atomic_replace(
        state_path(root, state.backend),
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(path_error)
}

#[must_use]
pub fn state_path(root: &Path, backend: BackendName) -> PathBuf {
    root.join("imports")
        .join(format!("{}.json", backend.as_str()))
}

fn path_error(error: impl fmt::Display) -> SyncStateWriteError {
    SyncStateWriteError {
        message: error.to_string(),
    }
}

fn serialize_state(value: &Map<String, Value>) -> Result<Vec<u8>, String> {
    let rendered = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    Ok(escape_non_ascii(&rendered).into_bytes())
}

fn escape_non_ascii(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii() {
            rendered.push(character);
        } else if u32::from(character) <= 0xffff {
            use std::fmt::Write;
            let _ = write!(rendered, "\\u{:04x}", u32::from(character));
        } else {
            use std::fmt::Write;
            let scalar = u32::from(character) - 0x1_0000;
            let high = 0xd800 + (scalar >> 10);
            let low = 0xdc00 + (scalar & 0x3ff);
            let _ = write!(rendered, "\\u{high:04x}\\u{low:04x}");
        }
    }
    rendered
}

fn validate_known_numbers(
    backend: BackendName,
    root: &Map<String, Value>,
) -> Result<(), SyncStateReadClass> {
    let fields: &[&str] = match backend {
        BackendName::Plaud => &["filesize"],
        BackendName::Oura => &[],
    };
    let Some(files) = root.get("files").and_then(Value::as_object) else {
        return Ok(());
    };
    for entry in files.values().filter_map(Value::as_object) {
        for field in fields {
            if entry.get(*field).is_some_and(outside_i64) {
                return Err(SyncStateReadClass::Overflow);
            }
        }
    }
    Ok(())
}

fn outside_i64(value: &Value) -> bool {
    value
        .as_u64()
        .is_some_and(|number| number > i64::MAX as u64)
}

/// The import records a sync made: each Plaud recording and each file the retired
/// audio-folder sync previously brought in. A source that syncs a backlog is not
/// an import, so the Import page and its list leave these out; their windows and
/// owner-data deletion stay where they are.
#[derive(Debug, Default)]
pub struct SyncMadeImports {
    plaud_imports: std::collections::BTreeSet<String>,
    audio_hashes: std::collections::BTreeSet<String>,
}

impl SyncMadeImports {
    /// Read from the syncs' own records of what they brought in. An unreadable
    /// record names nothing, and the rows it would have named stay listed.
    #[must_use]
    pub fn read(root: &Path) -> Self {
        let mut made = Self::default();
        if let SyncStateRead::Loaded(state) = read_sync_state(root, BackendName::Plaud) {
            made.plaud_imports = file_entries(state.root())
                .filter_map(|entry| entry.get("import_timestamp").and_then(Value::as_str))
                .map(str::to_owned)
                .collect();
        }
        if let Some(audio_root) = read_retired_audio_catalog(root) {
            let retained = audio_root
                .get("retained_catalogs")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|catalogs| catalogs.values())
                .filter_map(Value::as_object)
                .flat_map(|files| files.values())
                .filter_map(Value::as_object);
            made.audio_hashes = file_entries(&audio_root)
                .chain(retained)
                .filter(|entry| {
                    entry.get("status").and_then(Value::as_str) == Some("imported")
                        || entry.contains_key("imported_at")
                })
                .filter_map(|entry| entry.get("hash").and_then(Value::as_str))
                .map(str::to_owned)
                .collect();
        }
        made
    }

    /// Whether the import `id` is one a sync made.
    #[must_use]
    pub fn contains(&self, root: &Path, id: &str) -> bool {
        if self.plaud_imports.contains(id) {
            return true;
        }
        if import_record(root, id, "import.json")
            .as_ref()
            .and_then(|record| record.get("plaud_file_id"))
            .is_some_and(Value::is_string)
        {
            return true;
        }
        self.made_by_audio_folder(root, id)
    }

    /// Whether the import `id` is a file the retired audio-folder sync brought in.
    #[must_use]
    pub fn made_by_audio_folder(&self, root: &Path, id: &str) -> bool {
        if self.audio_hashes.is_empty() {
            return false;
        }
        ["import.json", "manifest.json"]
            .into_iter()
            .filter_map(|name| import_record(root, id, name))
            .filter_map(|record| record.get("source_hash")?.as_str().map(str::to_owned))
            .any(|hash| self.audio_hashes.contains(&hash))
    }
}

fn import_record(root: &Path, id: &str, name: &str) -> Option<Map<String, Value>> {
    serde_json::from_slice::<Value>(&fs::read(root.join("imports").join(id).join(name)).ok()?)
        .ok()?
        .as_object()
        .cloned()
}

fn read_retired_audio_catalog(root: &Path) -> Option<Map<String, Value>> {
    let bytes = fs::read(root.join("imports").join("audio.json")).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

fn file_entries(root: &Map<String, Value>) -> impl Iterator<Item = &Map<String, Value>> {
    root.get("files")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|files| files.values())
        .filter_map(Value::as_object)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn write_import_manifest(root: &Path, id: &str, hash: &str) {
        let dir = root.join("imports").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&json!({
                "source_type": "audio",
                "source_hash": hash,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn sync_made_imports_hides_prior_audio_folder_files_without_mutating_state() {
        let tree = tempfile::tempdir().unwrap();
        let imports = tree.path().join("imports");
        fs::create_dir_all(&imports).unwrap();
        let audio_state = serde_json::to_vec_pretty(&json!({
            "backend": "audio",
            "files": {
                "active.m4a": {"status": "imported", "hash": "hash-active"},
                "pending.m4a": {"status": "available", "hash": "hash-pending"},
            },
            "retained_catalogs": {
                "prior-folder": {
                    "parked.m4a": {"imported_at": "2026-10-01T12:00:00Z", "hash": "hash-parked"}
                }
            }
        }))
        .unwrap();
        let state_file = imports.join("audio.json");
        fs::write(&state_file, &audio_state).unwrap();

        write_import_manifest(tree.path(), "20261001_090000", "hash-active");
        write_import_manifest(tree.path(), "20261002_090000", "hash-parked");
        write_import_manifest(tree.path(), "20261003_090000", "hash-pending");
        write_import_manifest(tree.path(), "20261004_090000", "hash-owner-picked");

        let made = SyncMadeImports::read(tree.path());
        assert!(made.contains(tree.path(), "20261001_090000"));
        assert!(made.contains(tree.path(), "20261002_090000"));
        assert!(!made.contains(tree.path(), "20261003_090000"));
        assert!(!made.contains(tree.path(), "20261004_090000"));
        assert_eq!(fs::read(&state_file).unwrap(), audio_state);
    }

    #[test]
    fn corrupt_retired_audio_catalog_does_not_fail_sync_made_imports() {
        let tree = tempfile::tempdir().unwrap();
        let imports = tree.path().join("imports");
        fs::create_dir_all(&imports).unwrap();
        fs::write(imports.join("audio.json"), b"{not valid json").unwrap();
        write_import_manifest(tree.path(), "20261004_090000", "hash-owner-picked");

        let made = SyncMadeImports::read(tree.path());
        assert!(!made.contains(tree.path(), "20261004_090000"));
    }
}
