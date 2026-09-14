// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable, journal-local MCP permission storage.

use std::collections::{BTreeSet, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, PathError,
    create_directory_with_mode, hold_lock, write_json,
};

const ENDPOINT_DIRECTORY: &str = "mcp-endpoint";
const PERMISSIONS_FILE: &str = "permissions.json";
const SCHEMA_VERSION: u32 = 1;

/// The top-level durable permissions file envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionsFile {
    pub schema: u32,
    #[serde(default)]
    pub permissions: Vec<ConnectionPermissionRecord>,
}

impl Default for PermissionsFile {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            permissions: Vec::new(),
        }
    }
}

/// A single connection's permission grant record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionPermissionRecord {
    pub connection: String,
    pub schema: u32,
    pub generation: u64,
    pub evaluation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<ReadPermission>,
}

/// The read permission block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadPermission {
    pub categories: Vec<String>,
    pub scope: ReadScope,
}

impl ReadPermission {
    /// Construct the default whole-journal read permission.
    #[must_use]
    pub fn default_whole_journal() -> Self {
        Self {
            categories: vec![
                "transcripts".to_string(),
                "entities".to_string(),
                "facets".to_string(),
            ],
            scope: ReadScope::WholeJournal,
        }
    }
}

/// The read scope block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadScope {
    WholeJournal,
    Facets { ids: Vec<String> },
}

/// A typed decision for a connection read evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Snapshot(ConnectionReadSnapshot),
    Denied { reason: &'static str },
}

/// The current enforceable read boundary for one stored connection permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionReadSnapshot {
    pub categories: BTreeSet<AdmittedCategory>,
    pub scope: ConnectionScope,
    pub generation: u64,
}

/// Resolve owner-facing declared facet names to the stable identifiers stored
/// in a connection permission. This is read-only: callers decide whether to
/// persist the resulting ids.
pub fn resolve_permission_facet_names(
    journal_path: &Path,
    names: &[String],
) -> Result<Vec<String>, String> {
    let declared = solstone_core_facets::list_declared_facet_names(journal_path)
        .map_err(|error| format!("could not list declared facets: {error}"))?;
    let mut ids = BTreeSet::new();
    for name in names {
        if !declared.iter().any(|declared_name| declared_name == name) {
            return Err(format!("unknown facet {name:?}"));
        }
        let declaration = solstone_core_facets::read_facet_declaration(journal_path, name)
            .map_err(|error| format!("could not read facet {name:?}: {error}"))?
            .ok_or_else(|| format!("unknown facet {name:?}"))?;
        let Some(id) = declaration
            .value()
            .get("id")
            .and_then(serde_json::Value::as_str)
        else {
            return Err(format!(
                "facet {name:?} has no stable id; run journal backfill-facet-ids"
            ));
        };
        if !solstone_core_facets::is_well_formed_facet_id(id) {
            return Err(format!(
                "facet {name:?} has no stable id; run journal backfill-facet-ids"
            ));
        }
        ids.insert(id.to_owned());
    }
    Ok(ids.into_iter().collect())
}

/// Errors occurring while operating the permissions store.
#[derive(Debug)]
pub enum PermissionStoreError {
    Directory(PathError),
    Lock(LockError),
    Read { path: PathBuf, source: io::Error },
    Malformed { path: PathBuf, reason: String },
    UnsupportedSchema { path: PathBuf, found: u32 },
    Write(AtomicWriteError),
    ConnectionNotFound { key: String },
    InvalidConnectionKey { key: String },
}

fn format_message(formatter: &mut fmt::Formatter<'_>, message: fmt::Arguments<'_>) -> fmt::Result {
    fmt::Display::fmt(&message, formatter)
}

impl fmt::Display for PermissionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(error) => format_message(
                formatter,
                format_args!("could not prepare permissions directory: {error}"),
            ),
            Self::Lock(error) => format_message(
                formatter,
                format_args!("could not lock permissions store: {error}"),
            ),
            Self::Read { path, source } => format_message(
                formatter,
                format_args!("could not read permissions at {}: {source}", path.display()),
            ),
            Self::Malformed { path, reason } => format_message(
                formatter,
                format_args!("malformed permissions at {}: {reason}", path.display()),
            ),
            Self::UnsupportedSchema { path, found } => format_message(
                formatter,
                format_args!("unsupported schema version {found} at {}", path.display()),
            ),
            Self::Write(error) => format_message(
                formatter,
                format_args!("could not write permissions: {error}"),
            ),
            Self::ConnectionNotFound { key } => {
                format_message(formatter, format_args!("connection not found: {key}"))
            }
            Self::InvalidConnectionKey { key } => format_message(
                formatter,
                format_args!("invalid connection key format: {key}"),
            ),
        }
    }
}

impl Error for PermissionStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Lock(error) => Some(error),
            Self::Directory(error) => Some(error),
            Self::Write(error) => Some(error),
            _ => None,
        }
    }
}

/// Pure evaluation of a connection's read permissions without writing or modifying files.
#[must_use]
pub fn evaluate_connection_read(journal_root: &Path, connection_key: &str) -> PermissionDecision {
    let path = journal_root.join(ENDPOINT_DIRECTORY).join(PERMISSIONS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return PermissionDecision::Denied {
                reason: "no_permission",
            };
        }
        Err(_) => {
            return PermissionDecision::Denied {
                reason: "unenforceable",
            };
        }
    };

    let file: PermissionsFile = match serde_json::from_slice(&bytes) {
        Ok(f) => f,
        Err(_) => {
            return PermissionDecision::Denied {
                reason: "unenforceable",
            };
        }
    };

    if file.schema != SCHEMA_VERSION {
        return PermissionDecision::Denied {
            reason: "unenforceable",
        };
    }

    let Some(record) = file
        .permissions
        .iter()
        .find(|p| p.connection == connection_key)
    else {
        return PermissionDecision::Denied {
            reason: "no_permission",
        };
    };

    if record.schema != SCHEMA_VERSION || record.evaluation != "enforce" {
        return PermissionDecision::Denied {
            reason: "unenforceable",
        };
    }

    let Some(ref read) = record.read else {
        return PermissionDecision::Denied {
            reason: "no_permission",
        };
    };

    let categories = read
        .categories
        .iter()
        .filter_map(|category| match category.as_str() {
            "transcripts" => Some(AdmittedCategory::Transcripts),
            "entities" => Some(AdmittedCategory::Entities),
            "facets" => Some(AdmittedCategory::Facets),
            _ => None,
        })
        .collect();
    let scope = match &read.scope {
        ReadScope::WholeJournal => ConnectionScope::WholeJournal,
        ReadScope::Facets { ids } => ConnectionScope::ChosenFacets {
            ids: ids.iter().cloned().collect(),
        },
    };
    PermissionDecision::Snapshot(ConnectionReadSnapshot {
        categories,
        scope,
        generation: record.generation,
    })
}

/// A journal-root-bound permission store.
pub struct PermissionStore {
    root: PathBuf,
}

impl PermissionStore {
    /// Bind a permission store to one journal root.
    #[must_use]
    pub fn open(journal_root: &Path) -> Self {
        Self {
            root: journal_root.to_path_buf(),
        }
    }

    /// Return the path to the permissions file.
    #[must_use]
    pub fn permissions_path(&self) -> PathBuf {
        self.root.join(ENDPOINT_DIRECTORY).join(PERMISSIONS_FILE)
    }

    fn endpoint_directory(&self) -> PathBuf {
        self.root.join(ENDPOINT_DIRECTORY)
    }

    fn ensure_directory(&self) -> Result<(), PermissionStoreError> {
        create_directory_with_mode(&self.endpoint_directory(), 0o700)
            .map_err(PermissionStoreError::Directory)
    }

    /// Read the permissions file without locking.
    pub fn read(&self) -> Result<PermissionsFile, PermissionStoreError> {
        let path = self.permissions_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PermissionsFile::default());
            }
            Err(source) => return Err(PermissionStoreError::Read { path, source }),
        };
        let file: PermissionsFile =
            serde_json::from_slice(&bytes).map_err(|e| PermissionStoreError::Malformed {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        if file.schema != SCHEMA_VERSION {
            return Err(PermissionStoreError::UnsupportedSchema {
                path,
                found: file.schema,
            });
        }
        Ok(file)
    }

    /// Write the permissions file under lock.
    fn write_locked(&self, file: &PermissionsFile) -> Result<(), PermissionStoreError> {
        self.ensure_directory()?;
        let path = self.permissions_path();
        write_json(
            &path,
            file,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(PermissionStoreError::Write)
    }

    /// Get permission record for one connection key.
    pub fn get_permission(
        &self,
        connection_key: &str,
    ) -> Result<Option<ConnectionPermissionRecord>, PermissionStoreError> {
        let file = self.read()?;
        Ok(file
            .permissions
            .into_iter()
            .find(|p| p.connection == connection_key))
    }

    /// Set permission for one connection key under lock.
    pub fn set_permission(
        &self,
        connection_key: &str,
        read: ReadPermission,
    ) -> Result<ConnectionPermissionRecord, PermissionStoreError> {
        self.ensure_directory()?;
        let path = self.permissions_path();
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(PermissionStoreError::Lock)?;
        let mut file = self.read()?;
        let next_generation = match file
            .permissions
            .iter()
            .find(|p| p.connection == connection_key)
        {
            Some(existing) => existing.generation.saturating_add(1),
            None => 1,
        };
        let record = ConnectionPermissionRecord {
            connection: connection_key.to_string(),
            schema: SCHEMA_VERSION,
            generation: next_generation,
            evaluation: "enforce".to_string(),
            read: Some(read),
        };
        if let Some(pos) = file
            .permissions
            .iter()
            .position(|p| p.connection == connection_key)
        {
            file.permissions[pos] = record.clone();
        } else {
            file.permissions.push(record.clone());
        }
        self.write_locked(&file)?;
        Ok(record)
    }

    /// Clear permission for one connection key under lock.
    /// Returns true if an existing permission was cleared (incrementing generation and leaving read: None).
    /// Clear of missing does not invent a document and returns false.
    pub fn clear_permission(&self, connection_key: &str) -> Result<bool, PermissionStoreError> {
        self.ensure_directory()?;
        let path = self.permissions_path();
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(PermissionStoreError::Lock)?;
        let mut file = self.read()?;
        if let Some(pos) = file
            .permissions
            .iter()
            .position(|p| p.connection == connection_key)
        {
            let next_generation = file.permissions[pos].generation.saturating_add(1);
            file.permissions[pos] = ConnectionPermissionRecord {
                connection: connection_key.to_string(),
                schema: SCHEMA_VERSION,
                generation: next_generation,
                evaluation: "enforce".to_string(),
                read: None,
            };
            self.write_locked(&file)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Remove a connection record entirely (used on revocation/prune).
    pub fn remove_connection(&self, connection_key: &str) -> Result<bool, PermissionStoreError> {
        self.ensure_directory()?;
        let path = self.permissions_path();
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(PermissionStoreError::Lock)?;
        let mut file = self.read()?;
        let initial_len = file.permissions.len();
        file.permissions.retain(|p| p.connection != connection_key);
        let removed = file.permissions.len() != initial_len;
        if removed {
            self.write_locked(&file)?;
        }
        Ok(removed)
    }

    /// Sweep permission keys that are not present in `active_keys`. Returns number of swept keys.
    pub fn sweep_orphans(
        &self,
        active_keys: &HashSet<String>,
    ) -> Result<usize, PermissionStoreError> {
        self.ensure_directory()?;
        let path = self.permissions_path();
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(PermissionStoreError::Lock)?;
        let mut file = self.read()?;
        let initial_count = file.permissions.len();
        file.permissions
            .retain(|p| active_keys.contains(&p.connection));
        let removed_count = initial_count - file.permissions.len();
        if removed_count > 0 {
            self.write_locked(&file)?;
        }
        Ok(removed_count)
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn default_whole_journal_read_has_expected_shape() {
        let read = ReadPermission::default_whole_journal();
        assert_eq!(read.scope, ReadScope::WholeJournal);
        assert_eq!(read.categories, vec!["transcripts", "entities", "facets"]);
    }

    #[test]
    fn resolve_owner_facet_names_returns_ids_and_refuses_without_writing_permissions() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let facet = temp.path().join("facets/alpha");
        fs::create_dir_all(&facet).unwrap();
        fs::write(
            facet.join("facet.json"),
            r#"{"id":"123e4567-e89b-42d3-a456-426614174000","title":"Alpha"}"#,
        )
        .unwrap();
        let ids = resolve_permission_facet_names(temp.path(), &["alpha".to_owned()]).unwrap();
        let expected_ids = ids.clone();
        let store = PermissionStore::open(temp.path());
        store
            .set_permission(
                "bearer:owner",
                ReadPermission {
                    categories: vec!["facets".to_owned()],
                    scope: ReadScope::Facets { ids },
                },
            )
            .unwrap();
        let stored = store
            .get_permission("bearer:owner")
            .unwrap()
            .unwrap()
            .read
            .unwrap();
        assert_eq!(stored.scope, ReadScope::Facets { ids: expected_ids });
        let before = fs::read(temp.path().join(ENDPOINT_DIRECTORY).join(PERMISSIONS_FILE)).unwrap();
        assert!(resolve_permission_facet_names(temp.path(), &["missing".to_owned()]).is_err());
        fs::write(facet.join("facet.json"), r#"{"title":"Alpha"}"#).unwrap();
        assert!(
            resolve_permission_facet_names(temp.path(), &["alpha".to_owned()])
                .unwrap_err()
                .contains("journal backfill-facet-ids")
        );
        assert_eq!(
            before,
            fs::read(temp.path().join(ENDPOINT_DIRECTORY).join(PERMISSIONS_FILE)).unwrap()
        );
    }

    #[test]
    fn unknown_categories_authorize_nothing_but_remain_an_enforceable_snapshot() {
        let temp = TempDir::new_in("/var/tmp").expect("temp dir");
        let store = PermissionStore::open(temp.path());
        store
            .set_permission(
                "bearer:opaque",
                ReadPermission {
                    categories: vec!["unrecognized".to_owned()],
                    scope: ReadScope::WholeJournal,
                },
            )
            .expect("stores closed token fixture");
        assert_eq!(
            evaluate_connection_read(temp.path(), "bearer:opaque"),
            PermissionDecision::Snapshot(ConnectionReadSnapshot {
                categories: std::collections::BTreeSet::new(),
                scope: ConnectionScope::WholeJournal,
                generation: 1,
            })
        );
    }

    #[test]
    fn chosen_facet_scope_is_an_enforceable_snapshot() {
        let temp = TempDir::new_in("/var/tmp").expect("temp dir");
        let store = PermissionStore::open(temp.path());
        store
            .set_permission(
                "bearer:scoped",
                ReadPermission {
                    categories: vec!["transcripts".to_owned()],
                    scope: ReadScope::Facets {
                        ids: vec!["facet-stable-a".to_owned()],
                    },
                },
            )
            .expect("stores scoped permission");

        assert_eq!(
            evaluate_connection_read(temp.path(), "bearer:scoped"),
            PermissionDecision::Snapshot(ConnectionReadSnapshot {
                categories: [AdmittedCategory::Transcripts].into_iter().collect(),
                scope: ConnectionScope::ChosenFacets {
                    ids: ["facet-stable-a".to_owned()].into_iter().collect(),
                },
                generation: 1,
            })
        );
    }

    #[test]
    fn serde_roundtrip_locked_schema() {
        let raw_json = r#"{
            "schema": 1,
            "permissions": [
                {
                    "connection": "bearer:abc123def456",
                    "schema": 1,
                    "generation": 1,
                    "evaluation": "enforce",
                    "read": {
                        "categories": ["transcripts", "entities", "facets"],
                        "scope": {
                            "kind": "whole_journal"
                        }
                    }
                }
            ]
        }"#;

        let parsed: PermissionsFile = serde_json::from_str(raw_json).expect("parses schema 1");
        assert_eq!(parsed.schema, 1);
        assert_eq!(parsed.permissions.len(), 1);
        let record = &parsed.permissions[0];
        assert_eq!(record.connection, "bearer:abc123def456");
        assert_eq!(record.generation, 1);
        assert_eq!(record.evaluation, "enforce");

        let serialized = serde_json::to_string(&parsed).expect("serializes");
        let reparsed: PermissionsFile = serde_json::from_str(&serialized).expect("reparses");
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn store_rejects_unsupported_schema() {
        let temp = TempDir::new_in("/var/tmp").expect("temp dir");
        let store = PermissionStore::open(temp.path());
        let mcp_dir = temp.path().join(ENDPOINT_DIRECTORY);
        fs::create_dir_all(&mcp_dir).expect("creates dir");
        let raw = r#"{"schema": 2, "permissions": []}"#;
        fs::write(mcp_dir.join(PERMISSIONS_FILE), raw).expect("writes v2");

        let err = store.read().unwrap_err();
        match err {
            PermissionStoreError::UnsupportedSchema { found, .. } => assert_eq!(found, 2),
            other => panic!("unexpected error: {other:?}"),
        }

        // Pure evaluation returns unenforceable and does NOT rewrite the file
        let decision = evaluate_connection_read(temp.path(), "bearer:token1");
        assert_eq!(
            decision,
            PermissionDecision::Denied {
                reason: "unenforceable"
            }
        );
        assert_eq!(
            fs::read_to_string(mcp_dir.join(PERMISSIONS_FILE)).unwrap(),
            raw
        );
    }

    #[test]
    fn store_get_set_clear_sweep_lifecycle() {
        let temp = TempDir::new_in("/var/tmp").expect("temp dir");
        let store = PermissionStore::open(temp.path());

        // Missing file returns empty permissions and evaluates to no_permission
        let initial = store.read().expect("reads empty store");
        assert_eq!(initial.schema, 1);
        assert!(initial.permissions.is_empty());
        assert_eq!(store.get_permission("bearer:token1").unwrap(), None);
        assert_eq!(
            evaluate_connection_read(temp.path(), "bearer:token1"),
            PermissionDecision::Denied {
                reason: "no_permission"
            }
        );

        // Clear of missing does not invent a document
        assert!(!store.clear_permission("bearer:token1").unwrap());
        assert_eq!(store.get_permission("bearer:token1").unwrap(), None);

        // Set permission (generation starts at 1)
        let read = ReadPermission::default_whole_journal();
        let record = store
            .set_permission("bearer:token1", read.clone())
            .expect("sets perm");
        assert_eq!(record.generation, 1);
        assert_eq!(record.read, Some(read.clone()));
        assert_eq!(
            evaluate_connection_read(temp.path(), "bearer:token1"),
            PermissionDecision::Snapshot(ConnectionReadSnapshot {
                categories: [
                    AdmittedCategory::Transcripts,
                    AdmittedCategory::Entities,
                    AdmittedCategory::Facets,
                ]
                .into_iter()
                .collect(),
                scope: ConnectionScope::WholeJournal,
                generation: 1,
            })
        );

        // Set second permission
        let record2 = store
            .set_permission("oauth:grant1", read.clone())
            .expect("sets perm2");
        assert_eq!(record2.generation, 1);
        assert_eq!(store.read().unwrap().permissions.len(), 2);

        // Updating existing set increments generation
        let record1_updated = store
            .set_permission("bearer:token1", read)
            .expect("updates perm");
        assert_eq!(record1_updated.generation, 2);

        // Clear permission increments generation and leaves read: None
        let cleared = store.clear_permission("bearer:token1").expect("clears");
        assert!(cleared);
        let cleared_record = store
            .get_permission("bearer:token1")
            .unwrap()
            .expect("record exists");
        assert_eq!(cleared_record.generation, 3);
        assert_eq!(cleared_record.read, None);
        assert_eq!(
            evaluate_connection_read(temp.path(), "bearer:token1"),
            PermissionDecision::Denied {
                reason: "no_permission"
            }
        );

        // Sweep orphans: only oauth:grant1 is active
        let mut active = HashSet::new();
        active.insert("oauth:grant1".to_string());
        let swept = store.sweep_orphans(&active).expect("sweeps");
        assert_eq!(swept, 1);
        assert_eq!(store.get_permission("bearer:token1").unwrap(), None);
        assert_eq!(
            store
                .get_permission("oauth:grant1")
                .unwrap()
                .unwrap()
                .generation,
            1
        );
    }
}
