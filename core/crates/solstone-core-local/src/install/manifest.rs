// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::fingerprint;

pub const MANIFEST_NAME: &str = ".solstone-provider-manifest.json";
const PRIVATE_MODE: u32 = 0o600;

pub fn artifact_manifest_path(root: &Path) -> PathBuf {
    root.join(MANIFEST_NAME)
}

pub(crate) fn manifest_temp_name(file_name: &OsStr) -> OsString {
    let mut temporary = OsString::from(".");
    temporary.push(file_name);
    temporary.push(".tmp");
    temporary
}

pub fn runtime_inventory(root: &Path, exclude_names: &[String]) -> Result<Vec<Value>, String> {
    let mut output = Vec::new();
    walk_files(root, &mut |path| {
        if is_provider_manifest(path)
            || path.file_name().is_some_and(|name| {
                exclude_names
                    .iter()
                    .any(|excluded| name == std::ffi::OsStr::new(excluded))
            })
        {
            return Ok(());
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let role = if path.file_name().is_some_and(|name| name == "llama-server") {
            "runtime_binary"
        } else {
            "runtime_support"
        };
        output.push(inventory_entry(path, relative, role)?);
        Ok(())
    })?;
    Ok(output)
}

pub fn inventory_for_tree(root: &Path, role: &str) -> Result<Vec<Value>, String> {
    let mut output = Vec::new();
    walk_files(root, &mut |path| {
        if is_provider_manifest(path) {
            return Ok(());
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        output.push(inventory_entry(path, relative, role)?);
        Ok(())
    })?;
    Ok(output)
}

pub fn build_manifest(
    provider: &str,
    unit: &str,
    target_fingerprint_sha256: &str,
    source: Value,
    mut inventory: Vec<Value>,
    external_root: Option<&Path>,
    attempt_id: Option<&str>,
) -> Result<Value, String> {
    let source = normalize_source(source)?;
    inventory.sort_by_key(|entry| {
        (
            entry["role"].as_str().unwrap_or_default().to_owned(),
            entry["relative_path"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    });
    Ok(
        json!({"schema_version":1,"provider":provider,"unit":unit,"target_fingerprint_sha256":target_fingerprint_sha256,"created_by_attempt_id":attempt_id,"external_root":external_root.map(|root|root.display().to_string()),"source":source,"inventory":inventory}),
    )
}

pub fn write_manifest(path: &Path, manifest: &Value) -> Result<Value, String> {
    if !manifest.is_object() {
        return Err("provider artifact manifest must be an object".to_owned());
    }
    let parent = path.parent().ok_or("manifest path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(manifest_temp_name(
        path.file_name().ok_or("manifest file name")?,
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    set_private(&file).map_err(|error| error.to_string())?;
    file.write_all(
        serde_json::to_string_pretty(manifest)
            .map_err(|error| error.to_string())?
            .as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())?;
    Ok(manifest.clone())
}

pub fn prove_manifest(path: &Path, pin_identity: &Value) -> Value {
    prove_manifest_inner(path, pin_identity, None)
}

/// Prove one named inventory member while preserving the manifest proof's
/// status and reason-code vocabulary.
pub fn prove_manifest_member(path: &Path, pin_identity: &Value, member: &str) -> Value {
    prove_manifest_inner(path, pin_identity, Some(member))
}

fn prove_manifest_inner(path: &Path, pin_identity: &Value, wanted_member: Option<&str>) -> Value {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return proof("missing-or-mismatched", "manifest_missing");
        }
        Err(_) => return proof("proof-unavailable", "manifest_io_error"),
    };
    let manifest: Value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.is_object() => value,
        Ok(_) | Err(_) => return proof("missing-or-mismatched", "manifest_malformed"),
    };
    let expected = normalize_pin_identity(pin_identity).unwrap_or(Value::Null);
    if manifest.pointer("/source/pin_identity") != Some(&expected) {
        return proof("missing-or-mismatched", "manifest_pin_mismatch");
    }
    let root = manifest
        .get("external_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_path_buf());
    let Some(inventory) = manifest.get("inventory").and_then(Value::as_array) else {
        return proof("missing-or-mismatched", "manifest_malformed");
    };
    let mut found_member = wanted_member.is_none();
    for entry in inventory {
        let Some(relative) = entry.get("relative_path").and_then(Value::as_str) else {
            return proof("missing-or-mismatched", "inventory_malformed");
        };
        if wanted_member.is_some_and(|member| member != relative) {
            continue;
        }
        found_member = true;
        let relative = Path::new(relative);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return proof("missing-or-mismatched", "inventory_malformed");
        }
        let member = root.join(relative);
        if !member.is_file() {
            return proof("missing-or-mismatched", "inventory_member_missing");
        }
        let expected_hash = entry.get("sha256").and_then(Value::as_str).unwrap_or("");
        if expected_hash.is_empty() {
            return proof("missing-or-mismatched", "expected_hash_unavailable");
        }
        if sha256_file(&member).ok().as_deref() != Some(expected_hash) {
            return proof("missing-or-mismatched", "sha256_mismatch");
        }
    }
    if !found_member {
        return proof("missing-or-mismatched", "inventory_member_missing");
    }
    proof("ready", "ready")
}

pub fn cuda_trust(artifact: &Path, declared: &[String]) -> Value {
    match fs::read(artifact) {
        Err(_) => json!({"trust":"unavailable","reason_code":"cuda_runtime_artifact_unreadable"}),
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let found: std::collections::BTreeSet<_> = ["sm_86", "sm_89", "sm_120a", "sm_121a"]
                .into_iter()
                .filter(|arch| text.contains(arch))
                .map(ToOwned::to_owned)
                .collect();
            let declared: std::collections::BTreeSet<_> = declared.iter().cloned().collect();
            if declared.is_subset(&found) {
                json!({"trust":"trusted","declared_arch_set":declared,"embedded_arch_set":found})
            } else {
                json!({"trust":"absent","reason_code":"cuda_runtime_arch_mismatch","declared_arch_set":declared,"embedded_arch_set":found})
            }
        }
    }
}

fn inventory_entry(path: &Path, relative_path: String, role: &str) -> Result<Value, String> {
    Ok(
        json!({"relative_path":relative_path,"role":role,"size":fs::metadata(path).map_err(|error|error.to_string())?.len(),"sha256":sha256_file(path)?}),
    )
}
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest).map_err(|error| error.to_string())?;
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub(crate) fn verify_file_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let actual = sha256_file(path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "sha256 mismatch for {}: expected {expected}, got {actual}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    }
}
fn normalize_source(source: Value) -> Result<Value, String> {
    let canonical = fingerprint::canonical(source)?;
    let mut source: Value = serde_json::from_str(&canonical).map_err(|error| error.to_string())?;
    if source.get("pin_identity").is_none() {
        source = json!({"pin_identity":source});
    }
    Ok(source)
}
fn normalize_pin_identity(identity: &Value) -> Result<Value, String> {
    serde_json::from_str(&fingerprint::canonical(identity.clone())?)
        .map_err(|error| error.to_string())
}
fn proof(status: &str, reason_code: &str) -> Value {
    json!({"status":status,"reason_code":reason_code,"cache_hit":false})
}
fn is_provider_manifest(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        name == OsStr::new(MANIFEST_NAME) || name == manifest_temp_name(OsStr::new(MANIFEST_NAME))
    })
}
fn walk_files(
    root: &Path,
    callback: &mut impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            walk_files(&path, callback)?;
        } else if path.is_file() {
            callback(&path)?;
        }
    }
    Ok(())
}
fn set_private(_file: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        _file.set_permissions(fs::Permissions::from_mode(PRIVATE_MODE))?;
    }
    Ok(())
}
