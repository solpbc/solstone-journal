// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The known disk inventory is deliberately wider than native `BodySourceFamily`.
const KNOWN_SOURCE_FAMILIES: [&str; 4] = ["apple_health", "oura_api", "oura", "dexcom_clarity"];

/// The complete body-import inventory and the manifests skipped while reading it.
#[derive(Debug)]
pub struct BodyImportInventory {
    /// Readable, known-family imports sorted by manifest path.
    pub entries: Vec<BodyImportInventoryEntry>,
    /// Manifests skipped by the Python-compatible inventory scan.
    pub skipped: Vec<BodyImportSkip>,
}

/// One readable import manifest, retaining both its typed fields and full JSON object.
#[derive(Debug, Clone)]
pub struct BodyImportInventoryEntry {
    /// Bundle directory name.
    pub import_id: String,
    /// Manifest `source_type`.
    pub source_type: String,
    /// Tri-state manifest row-count claim.
    pub entry_count: ManifestEntryCount,
    /// Sorted normalized shard months.
    pub normalized_months: Vec<String>,
    /// Human-readable month inventory label.
    pub normalized_months_label: String,
    /// Full manifest contents, including the derived fields above.
    pub manifest: Map<String, Value>,
}

/// The distinct row-count states a manifest can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestEntryCount {
    /// No count field exists.
    Absent,
    /// The field was present and parsed as a non-negative integer.
    Present(u64),
    /// The field was present but was not a non-negative integer.
    Unparseable,
}

/// A skipped manifest and the reason it could not be inventoried.
#[derive(Debug)]
pub enum BodyImportSkip {
    /// The manifest could not be read as a JSON object.
    UnreadableManifest {
        /// Manifest path.
        path: PathBuf,
        /// Structured read failure.
        source: ManifestReadError,
    },
    /// A readable manifest has a source family outside the disk inventory set.
    UnknownSourceFamily {
        /// Manifest path.
        path: PathBuf,
        /// Source family declared by the manifest.
        source_type: String,
    },
}

/// Why a manifest was unreadable.
#[derive(Debug)]
pub enum ManifestReadError {
    /// Filesystem read failure.
    Read(io::Error),
    /// JSON decoding failure.
    Parse(serde_json::Error),
    /// Decoded JSON was not an object.
    NotAnObject,
}

/// A top-level inventory-directory read failure.
#[derive(Debug)]
pub struct BodyImportInventoryError {
    /// Directory which could not be read.
    pub path: PathBuf,
    /// Filesystem failure.
    pub source: io::Error,
}

impl fmt::Display for BodyImportInventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not read body import inventory {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for BodyImportInventoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Reads the Python-compatible body import manifest inventory under `journal_root`.
pub fn read_body_import_inventory(
    journal_root: impl AsRef<Path>,
) -> Result<BodyImportInventory, BodyImportInventoryError> {
    let imports = journal_root.as_ref().join("imports");
    let entries = match fs::read_dir(&imports) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(BodyImportInventory {
                entries: Vec::new(),
                skipped: Vec::new(),
            });
        }
        Err(source) => {
            return Err(BodyImportInventoryError {
                path: imports,
                source,
            });
        }
    };
    let mut manifests = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("manifest.json"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    manifests.sort();

    let mut inventory = BodyImportInventory {
        entries: Vec::new(),
        skipped: Vec::new(),
    };
    for path in manifests {
        let mut manifest = match read_manifest(&path) {
            Ok(manifest) => manifest,
            Err(source) => {
                log::warn!("Skipping unreadable import manifest {}", path.display());
                inventory
                    .skipped
                    .push(BodyImportSkip::UnreadableManifest { path, source });
                continue;
            }
        };
        let source_type = manifest
            .get("source_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if !KNOWN_SOURCE_FAMILIES.contains(&source_type.as_str()) {
            inventory
                .skipped
                .push(BodyImportSkip::UnknownSourceFamily { path, source_type });
            continue;
        }
        let import_id = manifest
            .get("import_id")
            .and_then(Value::as_str)
            .filter(|import_id| !import_id.is_empty())
            .or_else(|| {
                path.parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
            })
            .unwrap_or_default()
            .to_owned();
        let normalized_months = normalized_months(path.parent().unwrap_or(&imports));
        let normalized_months_label = normalized_months_label(&normalized_months);
        let entry_count = match manifest.get("entry_count") {
            None => ManifestEntryCount::Absent,
            Some(value) => value
                .as_u64()
                .map(ManifestEntryCount::Present)
                .unwrap_or(ManifestEntryCount::Unparseable),
        };
        manifest.insert("import_id".to_owned(), Value::String(import_id.clone()));
        manifest.insert(
            "normalized_months".to_owned(),
            Value::Array(
                normalized_months
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        manifest.insert(
            "normalized_months_label".to_owned(),
            Value::String(normalized_months_label.clone()),
        );
        inventory.entries.push(BodyImportInventoryEntry {
            import_id,
            source_type,
            entry_count,
            normalized_months,
            normalized_months_label,
            manifest,
        });
    }
    Ok(inventory)
}

fn read_manifest(path: &Path) -> Result<Map<String, Value>, ManifestReadError> {
    let contents = fs::read_to_string(path).map_err(ManifestReadError::Read)?;
    match serde_json::from_str(&contents).map_err(ManifestReadError::Parse)? {
        Value::Object(manifest) => Ok(manifest),
        _ => Err(ManifestReadError::NotAnObject),
    }
}

fn normalized_months(bundle: &Path) -> Vec<String> {
    let normalized = bundle.join("normalized");
    let Ok(entries) = fs::read_dir(normalized) else {
        return Vec::new();
    };
    let mut months = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("jsonl"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .collect::<Vec<_>>();
    months.sort();
    months
}

fn normalized_months_label(months: &[String]) -> String {
    match months {
        [] => "—".to_owned(),
        [month] => month.clone(),
        [first, .., last] => format!("{first} – {last} · {} months", months.len()),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-inventory-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn records_unreadable_and_unknown_manifests_without_losing_remaining_entries() {
        let temporary = TempDir::new();
        let imports = temporary.path().join("imports");
        for (name, manifest) in [
            ("valid", r#"{"source_type":"apple_health","entry_count":2}"#),
            ("broken", "not json"),
            ("unknown", r#"{"source_type":"garmin_connect"}"#),
        ] {
            let bundle = imports.join(name);
            fs::create_dir_all(bundle.join("normalized")).unwrap();
            fs::write(bundle.join("manifest.json"), manifest).unwrap();
        }
        fs::write(imports.join("valid/normalized/2024-01.jsonl"), "").unwrap();

        let inventory = read_body_import_inventory(temporary.path()).unwrap();
        assert_eq!(inventory.entries.len(), 1);
        assert_eq!(inventory.entries[0].import_id, "valid");
        assert!(
            inventory.entries[0]
                .manifest
                .contains_key("normalized_months_label")
        );
        assert!(matches!(
            inventory.skipped.as_slice(),
            [
                BodyImportSkip::UnreadableManifest { .. },
                BodyImportSkip::UnknownSourceFamily { source_type, .. }
            ] if source_type == "garmin_connect"
        ));
        assert!(matches!(
            &inventory.skipped[0],
            BodyImportSkip::UnreadableManifest {
                source: ManifestReadError::Parse(_),
                ..
            }
        ));
    }
}
