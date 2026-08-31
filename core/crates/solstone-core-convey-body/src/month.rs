// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{NormalizedRow, ShardReadError, read_normalized_shard};

/// Per-request normalized-month reader with identity-preserving month cache.
#[derive(Debug)]
pub struct MonthReader {
    journal_root: PathBuf,
    cache: BTreeMap<String, Arc<Vec<NormalizedRow>>>,
}

impl MonthReader {
    /// Creates a reader rooted at one journal directory.
    pub fn new(journal_root: impl Into<PathBuf>) -> Self {
        Self {
            journal_root: journal_root.into(),
            cache: BTreeMap::new(),
        }
    }

    /// Reads one month, returning a shared cached result for repeated requests.
    pub fn read_month(&mut self, month: &str) -> Result<Arc<Vec<NormalizedRow>>, ShardReadError> {
        if let Some(rows) = self.cache.get(month) {
            return Ok(Arc::clone(rows));
        }
        let rows = Arc::new(read_normalized_rows(&self.journal_root, Some(month))?);
        self.cache.insert(month.to_owned(), Arc::clone(&rows));
        Ok(rows)
    }
}

/// Reads normalized rows in reverse-lexicographic shard-path order with within-month dedupe.
pub fn read_normalized_rows(
    journal_root: impl AsRef<Path>,
    month: Option<&str>,
) -> Result<Vec<NormalizedRow>, ShardReadError> {
    let paths = normalized_shard_paths(journal_root.as_ref(), month)?;
    let mut rows = Vec::new();
    let mut seen = BTreeMap::<String, usize>::new();
    for path in paths {
        for mut row in read_normalized_shard(path)? {
            let Some(key) = row.dedupe_key_text().map(str::to_owned) else {
                rows.push(row);
                continue;
            };
            let own_import_id = row.import_id_text().map(str::to_owned);
            if let Some(index) = seen.get(&key).copied() {
                if let Some(import_id) = own_import_id {
                    let import_ids = &mut rows[index].import_ids;
                    if !import_ids.contains(&import_id) {
                        import_ids.insert(0, import_id);
                    }
                }
                continue;
            }
            if let Some(import_id) = own_import_id {
                row.import_ids.push(import_id);
            }
            seen.insert(key, rows.len());
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Every month with a normalized shard in any import bundle, unique and ascending.
pub fn coverage_month_keys(journal_root: impl AsRef<Path>) -> Result<Vec<String>, ShardReadError> {
    let imports = journal_root.as_ref().join("imports");
    let entries = match fs::read_dir(&imports) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ShardReadError::Read {
                path: imports,
                source,
            });
        }
    };
    let mut months = std::collections::BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|source| ShardReadError::Read {
            path: imports.clone(),
            source,
        })?;
        let normalized = entry.path().join("normalized");
        let shards = match fs::read_dir(&normalized) {
            Ok(shards) => shards,
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(source) => {
                return Err(ShardReadError::Read {
                    path: normalized,
                    source,
                });
            }
        };
        for shard in shards {
            let shard = shard.map_err(|source| ShardReadError::Read {
                path: normalized.clone(),
                source,
            })?;
            let path = shard.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                && is_month(stem)
            {
                months.insert(stem.to_owned());
            }
        }
    }
    Ok(months.into_iter().collect())
}

fn normalized_shard_paths(
    journal_root: &Path,
    month: Option<&str>,
) -> Result<Vec<PathBuf>, ShardReadError> {
    let imports = journal_root.join("imports");
    let entries = match fs::read_dir(&imports) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ShardReadError::Read {
                path: imports,
                source,
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ShardReadError::Read {
            path: imports.clone(),
            source,
        })?;
        if !entry
            .file_type()
            .map_err(|source| ShardReadError::Read {
                path: entry.path(),
                source,
            })?
            .is_dir()
        {
            continue;
        }
        let normalized = entry.path().join("normalized");
        let shards = match fs::read_dir(&normalized) {
            Ok(shards) => shards,
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(source) => {
                return Err(ShardReadError::Read {
                    path: normalized,
                    source,
                });
            }
        };
        for shard in shards {
            let shard = shard.map_err(|source| ShardReadError::Read {
                path: normalized.clone(),
                source,
            })?;
            let path = shard.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(month) = month
                && path.file_stem().and_then(|stem| stem.to_str()) != Some(month)
            {
                continue;
            }
            paths.push(path);
        }
    }
    paths.sort_by(|left, right| right.cmp(left));
    Ok(paths)
}

fn is_month(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(|byte| byte.is_ascii_digit())
        && bytes[5..].iter().all(|byte| byte.is_ascii_digit())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-month-{}-{}",
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

    fn write_shard(root: &Path, bundle: &str, rows: &[serde_json::Value]) {
        let path = root.join("imports").join(bundle).join("normalized");
        fs::create_dir_all(&path).unwrap();
        let contents = rows
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        fs::write(path.join("2024-01.jsonl"), format!("{contents}\n")).unwrap();
    }

    #[test]
    fn later_bundle_payload_wins_and_import_ids_are_oldest_first() {
        let temporary = TempDir::new();
        write_shard(
            temporary.path(),
            "body-a",
            &[json!({"dedupe_key":"synthetic-key","import_id":"body-a","value":1})],
        );
        write_shard(
            temporary.path(),
            "body-b",
            &[json!({"dedupe_key":"synthetic-key","import_id":"body-b","value":2})],
        );
        let rows = read_normalized_rows(temporary.path(), Some("2024-01")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].import_ids, ["body-a", "body-b"]);
        assert_eq!(rows[0].extra.len(), 0);
        assert!(matches!(
            rows[0].value,
            solstone_core_body_source::ValueState::Present(
                solstone_core_body_source::BodyValue::Integer(ref value)
            ) if value.digits() == "2"
        ));
    }

    #[test]
    fn reads_bundles_in_reverse_lexicographic_path_order() {
        let temporary = TempDir::new();
        for bundle in ["body-a", "body-b", "body-c"] {
            write_shard(
                temporary.path(),
                bundle,
                &[json!({"dedupe_key":bundle,"import_id":bundle,"value":bundle})],
            );
        }
        let rows = read_normalized_rows(temporary.path(), Some("2024-01")).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.dedupe_key_text())
                .collect::<Vec<_>>(),
            [Some("body-c"), Some("body-b"), Some("body-a")]
        );
        let mut reader = MonthReader::new(temporary.path());
        let first = reader.read_month("2024-01").unwrap();
        let second = reader.read_month("2024-01").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }
}
