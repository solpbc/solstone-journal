// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable schema-v1 setup state.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_installation_identity::LegacyManifestEvidence;

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

pub const SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupManifest {
    pub schema_version: u8,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub mode: String,
    pub args_resolved: Map<String, Value>,
    pub steps: Vec<Value>,
}

impl SetupManifest {
    #[must_use]
    pub fn initial(started_at: String, mode: String, args_resolved: Map<String, Value>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            started_at,
            completed_at: None,
            mode,
            args_resolved,
            steps: Vec::new(),
        }
    }
}

#[must_use]
pub fn manifest_path(journal_path: &Path) -> PathBuf {
    journal_path.join("health").join("setup-state.json")
}

/// Read any schema-v1-compatible JSON manifest, including one written by Python.
#[must_use]
pub fn read_manifest(path: &Path) -> Option<SetupManifest> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Classifies legacy schema-v1 manifest evidence without changing the tolerant
/// reader used by existing setup state consumers.
#[must_use]
pub fn legacy_manifest_evidence(path: &Path) -> LegacyManifestEvidence {
    match fs::read(path) {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            LegacyManifestEvidence::Absent
        }
        Err(_) => LegacyManifestEvidence::Unreadable,
        Ok(_) if read_manifest(path).is_some() => LegacyManifestEvidence::ValidProviderlessSchemaV1,
        Ok(_) => LegacyManifestEvidence::Malformed,
    }
}

/// Index the last record for each externally supplied step name.
#[must_use]
pub fn prior_steps(manifest: &SetupManifest) -> BTreeMap<String, &Map<String, Value>> {
    manifest
        .steps
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|step| {
            step.get("name")
                .and_then(Value::as_str)
                .map(|name| (name.to_owned(), step))
        })
        .collect()
}

/// Match Python: only a prior `ok` whose every recorded path exists may skip.
#[must_use]
pub fn can_skip(step: Option<&Map<String, Value>>) -> bool {
    let Some(step) = step else {
        return false;
    };
    if step.get("status").and_then(Value::as_str) != Some("ok") {
        return false;
    }
    step.get("paths")
        .and_then(Value::as_array)
        .is_none_or(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .all(|path| Path::new(path).exists())
        })
}

/// Publish the manifest atomically. Errors are warnings, never setup failures.
pub fn write_manifest(path: &Path, manifest: &SetupManifest) {
    if let Err(error) = write_manifest_inner(path, manifest) {
        eprintln!("warning: could not write setup manifest: {error}");
    }
}

fn write_manifest_inner(path: &Path, manifest: &SetupManifest) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "manifest path must have a parent",
        )
    })?;
    fs::create_dir_all(parent)?;
    let content = serde_json::to_string_pretty(manifest).map_err(io::Error::other)?;
    let (temp_path, mut file) = create_temp_file(parent)?;
    let result = (|| {
        file.write_all(content.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        drop(file);
        fs::rename(&temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

fn create_temp_file(parent: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..128 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".tmp_setup_state{}_{sequence}.json",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary setup manifest",
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        SCHEMA_VERSION, SetupManifest, can_skip, legacy_manifest_evidence, manifest_path,
        prior_steps, read_manifest, write_manifest,
    };
    use serde_json::{Map, json};
    use std::fs;
    use std::path::PathBuf;

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "solstone-core-setup-manifest-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn manifest() -> SetupManifest {
        SetupManifest::initial(
            "2026-01-01T00:00:00Z".into(),
            "interactive".into(),
            Map::new(),
        )
    }

    #[test]
    fn manifest_path_and_schema_shape_are_pinned() {
        let journal = root("shape").join("journal");
        let path = manifest_path(&journal);
        assert_eq!(path, journal.join("health/setup-state.json"));
        write_manifest(&path, &manifest());
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "schema_version",
                "started_at",
                "completed_at",
                "mode",
                "args_resolved",
                "steps"
            ]
        );
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn write_is_atomic_and_read_old_accepts_python_shape() {
        let path = manifest_path(&root("old").join("journal"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
  "schema_version": 1,
  "started_at": "2026-01-01T00:00:00Z",
  "completed_at": null,
  "mode": "non_interactive",
  "args_resolved": {},
  "steps": [{"name":"doctor","status":"ok","paths":[]}]
}
"#,
        )
        .unwrap();
        let old = read_manifest(&path).unwrap();
        assert_eq!(prior_steps(&old).len(), 1);
        write_manifest(&path, &manifest());
        assert!(read_manifest(&path).is_some());
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp_setup_state")
        }));
    }

    #[test]
    fn write_failure_is_warning_only() {
        let root = root("warning");
        let blocker = root.join("not-a-directory");
        fs::write(&blocker, "blocker").unwrap();
        write_manifest(&blocker.join("setup-state.json"), &manifest());
        assert_eq!(fs::read_to_string(blocker).unwrap(), "blocker");
    }

    #[test]
    fn legacy_manifest_evidence_preserves_read_tolerance_and_error_classes() {
        let root = root("legacy-evidence");
        let missing = root.join("missing.json");
        assert_eq!(
            legacy_manifest_evidence(&missing),
            solstone_core_installation_identity::LegacyManifestEvidence::Absent
        );
        let valid = root.join("valid.json");
        write_manifest(&valid, &manifest());
        assert_eq!(
            legacy_manifest_evidence(&valid),
            solstone_core_installation_identity::LegacyManifestEvidence::ValidProviderlessSchemaV1
        );
        let malformed = root.join("malformed.json");
        fs::write(&malformed, "{").unwrap();
        assert_eq!(
            legacy_manifest_evidence(&malformed),
            solstone_core_installation_identity::LegacyManifestEvidence::Malformed
        );
    }

    #[test]
    fn can_skip_requires_ok_and_existing_paths() {
        let root = root("skip");
        let present = root.join("present");
        fs::write(&present, "x").unwrap();
        let ok = Map::from_iter([
            ("status".into(), json!("ok")),
            ("paths".into(), json!([present])),
        ]);
        assert!(can_skip(Some(&ok)));
        let missing = Map::from_iter([
            ("status".into(), json!("ok")),
            ("paths".into(), json!([root.join("missing")])),
        ]);
        assert!(!can_skip(Some(&missing)));
        let skipped = Map::from_iter([("status".into(), json!("skipped"))]);
        assert!(!can_skip(Some(&skipped)));
    }
}
