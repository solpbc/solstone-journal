// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::hexutil::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static BUNDLE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMember {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportLock {
    pub lock_version: u32,
    pub compiler: String,
    pub manifest_sha256: String,
    pub members: Vec<BundleMember>,
}

pub fn export_bundle(catalog_dir: &Path, out_dir: &Path) -> Result<(), Fault> {
    if out_dir.exists() {
        return Err(Fault::catalog("destination-exists"));
    }

    // Collect all export members
    let mut member_paths: Vec<String> = Vec::new();

    if catalog_dir.join("catalog.json").exists() {
        member_paths.push("catalog.json".to_string());
    }
    if catalog_dir.join("history.json").exists() {
        member_paths.push("history.json".to_string());
    }
    if catalog_dir.join("source-lock.json").exists() {
        member_paths.push("source-lock.json".to_string());
    }

    let source_dir = catalog_dir.join("source");
    if source_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&source_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".ftl") {
                member_paths.push(format!("source/{}", name));
            }
        }
    }

    let trans_dir = catalog_dir.join("translations");
    if trans_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&trans_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".ftl") {
                member_paths.push(format!("translations/{}", name));
            }
        }
    }

    let meta_dir = catalog_dir.join("meta");
    if meta_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&meta_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") {
                member_paths.push(format!("meta/{}", name));
            }
        }
    }

    member_paths.sort();

    let mut members = Vec::new();
    let mut member_data: Vec<(String, Vec<u8>)> = Vec::new();

    for rel_path in &member_paths {
        let full_path = catalog_dir.join(rel_path);
        let bytes = fs::read(&full_path).map_err(|_| Fault::catalog("io-error"))?;
        let hash = sha256_hex(&bytes);
        members.push(BundleMember {
            path: rel_path.clone(),
            sha256: hash,
        });
        member_data.push((rel_path.clone(), bytes));
    }

    let members_json = serde_json::to_string(&members).map_err(|_| Fault::catalog("io-error"))?;
    let manifest_sha256 = sha256_hex(members_json.as_bytes());

    let lock = ImportLock {
        lock_version: 1,
        compiler: "0.1.0".to_string(),
        manifest_sha256,
        members,
    };

    let mut lock_bytes = serde_json::to_vec(&lock).map_err(|_| Fault::catalog("io-error"))?;
    lock_bytes.push(b'\n');

    member_data.push(("import-lock.json".to_string(), lock_bytes));

    let parent = out_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|_| Fault::catalog("io-error"))?;

    let dir_name = out_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bundle");
    let pid = std::process::id();
    let count = BUNDLE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir_name = format!("{}.tmp.{}.{}", dir_name, pid, count);
    let temp_dir = parent.join(temp_dir_name);

    fs::create_dir_all(&temp_dir).map_err(|_| Fault::catalog("io-error"))?;

    let write_res = (|| -> Result<(), std::io::Error> {
        for (rel, bytes) in &member_data {
            let dest_file = temp_dir.join(rel);
            if let Some(p) = dest_file.parent() {
                fs::create_dir_all(p)?;
            }
            let mut f = fs::File::create(&dest_file)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("io-error"));
    }

    if out_dir.exists() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("destination-exists"));
    }

    if fs::rename(&temp_dir, out_dir).is_err() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("io-error"));
    }

    Ok(())
}

pub fn import_bundle(bundle_dir: &Path, dest_dir: &Path) -> Result<(), Fault> {
    // 1. Validate the bundle BEFORE touching the destination.
    let lock_file = bundle_dir.join("import-lock.json");
    let lock_bytes = fs::read(&lock_file).map_err(|_| Fault::catalog("import-refused"))?;

    let lock: ImportLock =
        serde_json::from_slice(&lock_bytes).map_err(|_| Fault::catalog("import-refused"))?;

    if lock.lock_version != 1 {
        return Err(Fault::catalog("unsupported-version"));
    }

    // Check canonical lock serialization
    let mut expected_lock_bytes =
        serde_json::to_vec(&lock).map_err(|_| Fault::catalog("import-refused"))?;
    expected_lock_bytes.push(b'\n');
    if lock_bytes != expected_lock_bytes {
        return Err(Fault::catalog("import-refused"));
    }

    // Check compiler version
    if lock.compiler != "0.1.0" {
        return Err(Fault::catalog("import-refused"));
    }

    // Check manifest_sha256
    let members_json =
        serde_json::to_string(&lock.members).map_err(|_| Fault::catalog("import-refused"))?;
    if sha256_hex(members_json.as_bytes()) != lock.manifest_sha256 {
        return Err(Fault::catalog("import-refused"));
    }

    // Collect all actual files in bundle_dir
    let mut actual_files = HashSet::new();
    collect_files_recursive(bundle_dir, "", &mut actual_files)?;

    let mut expected_files = HashSet::new();
    expected_files.insert("import-lock.json".to_string());

    let mut files_to_write = Vec::new();
    files_to_write.push(("import-lock.json".to_string(), lock_bytes));

    for member in &lock.members {
        let rel = &member.path;
        if rel.starts_with('/') || rel.contains("..") || rel.contains('\\') {
            return Err(Fault::catalog("import-refused"));
        }
        let full_path = bundle_dir.join(rel);
        let meta =
            fs::symlink_metadata(&full_path).map_err(|_| Fault::catalog("import-refused"))?;
        if meta.file_type().is_symlink() {
            return Err(Fault::catalog("import-refused"));
        }

        let bytes = fs::read(&full_path).map_err(|_| Fault::catalog("import-refused"))?;
        let hash = sha256_hex(&bytes);
        if hash != member.sha256 {
            return Err(Fault::catalog("import-refused"));
        }

        expected_files.insert(rel.clone());
        files_to_write.push((rel.clone(), bytes));
    }

    // Extra members or missing members reject
    if actual_files != expected_files {
        return Err(Fault::catalog("import-refused"));
    }

    // 2. Bundle is completely valid! Now check if dest_dir exists
    if dest_dir.exists() {
        return Err(Fault::catalog("destination-exists"));
    }

    // 3. Stage the whole tree in a unique temp directory and rename
    let parent = dest_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|_| Fault::catalog("io-error"))?;

    let dir_name = dest_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dest");
    let pid = std::process::id();
    let count = BUNDLE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir_name = format!("{}.tmp.{}.{}", dir_name, pid, count);
    let temp_dir = parent.join(temp_dir_name);

    fs::create_dir_all(&temp_dir).map_err(|_| Fault::catalog("io-error"))?;

    let write_res = (|| -> Result<(), std::io::Error> {
        for (rel, bytes) in &files_to_write {
            let dest_file = temp_dir.join(rel);
            if let Some(p) = dest_file.parent() {
                fs::create_dir_all(p)?;
            }
            let mut f = fs::File::create(&dest_file)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("io-error"));
    }

    if dest_dir.exists() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("destination-exists"));
    }

    if fs::rename(&temp_dir, dest_dir).is_err() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("io-error"));
    }

    Ok(())
}

fn collect_files_recursive(
    base: &Path,
    prefix: &str,
    out: &mut HashSet<String>,
) -> Result<(), Fault> {
    let dir = if prefix.is_empty() {
        base.to_path_buf()
    } else {
        base.join(prefix)
    };
    let entries = fs::read_dir(&dir).map_err(|_| Fault::catalog("import-refused"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|_| Fault::catalog("import-refused"))?;
        if meta.file_type().is_symlink() {
            return Err(Fault::catalog("import-refused"));
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{}/{}", prefix, name)
        };
        if meta.is_dir() {
            collect_files_recursive(base, &rel, out)?;
        } else {
            out.insert(rel);
        }
    }
    Ok(())
}
