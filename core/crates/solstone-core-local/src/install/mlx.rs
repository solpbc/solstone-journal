// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! MLX snapshot acquisition deliberately has a local-source seam.  Production
//! fetching can supply a populated snapshot directory; tests never need Hugging Face.

use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;

use super::archive::verify_sha256;

pub trait SnapshotSource {
    fn populate(&self, destination: &Path) -> Result<(), String>;
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
