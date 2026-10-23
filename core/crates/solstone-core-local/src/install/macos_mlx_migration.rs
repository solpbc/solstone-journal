// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-owned, one-way macOS runtime decision and predecessor retirement.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, atomic_replace, hold_lock, write_bytes_exclusive,
};

use super::{metal_candidate, mlx_candidate, pins};

const MODEL: &str = "local/qwen3.5-4b";
const RECORD: &str = "macos-mlx-migration.json";
// journal-io appends the stable .lock sidecar.
const LOCK: &str = "macos-mlx-migration";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationState {
    Absent,
    InProgress,
    Committed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RecordedState {
    InProgress,
    Committed,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    state: RecordedState,
}

fn parse(bytes: &[u8]) -> io::Result<MigrationState> {
    let value: Record = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    if value.schema != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported macOS runtime migration record schema",
        ));
    }
    Ok(match value.state {
        RecordedState::InProgress => MigrationState::InProgress,
        RecordedState::Committed => MigrationState::Committed,
    })
}

fn config_directory(journal: &Path) -> io::Result<PathBuf> {
    let config = journal.join("config");
    match fs::symlink_metadata(&config) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => Err(
            io::Error::other("macOS migration config is not a plain directory"),
        ),
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(config),
    }
}

fn lock_path(config: &Path) -> io::Result<PathBuf> {
    let path = config.join(LOCK);
    match fs::symlink_metadata(config.join(format!("{LOCK}.lock"))) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => Err(
            io::Error::other("macOS migration lock is not a regular file"),
        ),
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(path),
    }
}

/// Missing is the only state permitting a new decision; damaged authority is an error.
pub fn read(journal: &Path) -> io::Result<MigrationState> {
    let path = config_directory(journal)?.join(RECORD);
    match fs::symlink_metadata(&path) {
        Ok(metadata)
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > 4096 =>
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "macOS runtime migration record is not a regular file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(MigrationState::Absent);
        }
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    parse(&fs::read(path)?)
}

fn bytes(state: RecordedState) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&Record { schema: 1, state })
        .expect("closed migration record serializes");
    bytes.push(b'\n');
    bytes
}

/// Arm an installed predecessor before starting its model download.
pub fn start(journal: &Path) -> io::Result<MigrationState> {
    let config = config_directory(journal)?;
    let _lock = hold_lock(lock_path(&config)?, LockOptions::default()).map_err(io::Error::other)?;
    let state = read(journal)?;
    if state == MigrationState::Absent {
        write_bytes_exclusive(
            config.join(RECORD),
            &bytes(RecordedState::InProgress),
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .map_err(io::Error::other)?;
        return Ok(MigrationState::InProgress);
    }
    Ok(state)
}

/// The caller has admitted its owned MLX process after predecessor exit.
/// This decision is never reversed, including when later retirement fails.
pub fn commit(journal: &Path) -> io::Result<()> {
    let config = config_directory(journal)?;
    let _lock = hold_lock(lock_path(&config)?, LockOptions::default()).map_err(io::Error::other)?;
    if read(journal)? != MigrationState::Committed {
        atomic_replace(
            config.join(RECORD),
            &bytes(RecordedState::Committed),
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .map_err(io::Error::other)?;
    }
    Ok(())
}

/// Read-side projections follow the serving runtime during an uncommitted swap.
/// The health fingerprint selects an already-published MLX attempt; it never
/// establishes the durable one-way decision or authorizes predecessor deletion.
pub fn inspect_serving(journal: &Path) -> Value {
    let state = match read(journal) {
        Ok(state) => state,
        Err(error) => {
            return json!({"provider":"local","backend":"mlx","ready":false,"status":"proof-unavailable","reason_code":"local_probe_failed","detail":error.to_string(),"artifacts":{"binary_installed":false,"model_installed":false}});
        }
    };
    let mlx = mlx_candidate::inspect_present_default(journal);
    if state == MigrationState::Committed {
        return mlx;
    }
    let input = Map::from_iter([
        ("journal".into(), json!(journal)),
        ("model_id".into(), json!(MODEL)),
        ("backend".into(), json!("metal")),
    ]);
    let metal = match metal_candidate::inspect_present_with(&input, "aarch64-apple-darwin") {
        Ok(value) if value["ready"] == true => value,
        _ => return mlx,
    };
    let mlx_fingerprint = solstone_core_brain::bundled_runtime_desired_fingerprint(
        "mlx",
        MODEL,
        mlx.pointer("/target/target_fingerprint_sha256")
            .and_then(Value::as_str)
            .unwrap_or(""),
        mlx.pointer("/artifacts/binary_path")
            .and_then(Value::as_str),
        mlx.pointer("/artifacts/model_path")
            .and_then(Value::as_str)
            .unwrap_or(""),
        None,
    )
    .ok();
    let health = solstone_core_brain::inspect_runtime_health(journal);
    if mlx_fingerprint.as_ref().is_some_and(|fingerprint| {
        health
            .record
            .as_ref()
            .and_then(|value| value["desired_fingerprint_sha256"].as_str())
            == Some(fingerprint.sha256.as_str())
    }) {
        mlx
    } else {
        metal
    }
}

/// Refuse symlinks anywhere below the journal, including lock-directory ancestors.
fn plain_directory_below(journal: &Path, path: &Path) -> io::Result<bool> {
    let relative = path.strip_prefix(journal).map_err(io::Error::other)?;
    let mut parent = journal.to_path_buf();
    for component in relative.components() {
        parent.push(component);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "managed runtime cache is not a plain directory: {}",
                    parent.display()
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    Ok(true)
}

// These predecessor packages are flat. Enumerate pinned members and exact
// library-link targets before removing anything; an unknown entry is retained.
fn retire_flat(
    path: &Path,
    members: &[(String, Option<String>)],
    retained: &[String],
) -> io::Result<()> {
    let mut files = Vec::new();
    let mut has_retained = false;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let metadata = fs::symlink_metadata(entry.path())?;
        if retained.iter().any(|retained| name == retained.as_str()) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(io::Error::other(
                    "predecessor transfer lock is not a regular file",
                ));
            }
            has_retained = true;
            continue;
        }
        let Some((_, link)) = members.iter().find(|(member, _)| name == member.as_str()) else {
            return Err(io::Error::other("unknown predecessor cache entry"));
        };
        let valid = if let Some(target) = link {
            metadata.file_type().is_symlink() && fs::read_link(entry.path())? == Path::new(target)
        } else {
            metadata.is_file() && !metadata.file_type().is_symlink()
        };
        if !valid {
            return Err(io::Error::other(
                "predecessor cache entry has changed type or link target",
            ));
        }
        files.push(entry.path());
    }
    // The runtime manifest authorizes auxiliary package members. Keep it until
    // every other member is gone so an interrupted retirement can resume.
    files.sort_by_key(|file| {
        file.file_name()
            .is_some_and(|name| name == ".solstone-provider-manifest.json")
    });
    for file in files {
        fs::remove_file(file)?;
    }
    if has_retained {
        Ok(())
    } else {
        fs::remove_dir(path)
    }
}

/// Retire only named managed predecessor trees after the caller proves its exit.
/// In-use, damaged or linked trees remain retryable. No current model collection runs.
pub fn retire_predecessor(journal: &Path) -> io::Result<Vec<PathBuf>> {
    if read(journal)? != MigrationState::Committed {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "macOS runtime migration is not committed",
        ));
    }
    let journal = fs::canonicalize(journal)?;
    let _install = super::lease::acquire(&journal, "local")
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "local installer is in use"))?;
    let cache = pins::cache_root(&journal);
    let (release, filename, hash, _) = pins::vulkan_pin("aarch64-apple-darwin")
        .ok_or_else(|| io::Error::other("predecessor runtime pin unavailable"))?;
    let runtime = cache.join("bin/aarch64-apple-darwin").join(release);
    let runtime_fetch = cache.join(".fetch").join(hash);
    let mut paths = BTreeSet::from([
        runtime.clone(),
        runtime_fetch.clone(),
        pins::model_directory(&journal, MODEL),
        cache.join("models/local__qwen3.5-4b"),
    ]);
    if let Some(generation) = pins::model_generation(MODEL) {
        paths.insert(generation.directory(&cache.join("models")));
    }
    let mut held = Vec::new();
    for path in paths {
        if !plain_directory_below(&journal, &path)? {
            continue;
        }
        let parent = path.parent().expect("managed predecessor has parent");
        plain_directory_below(&journal, &parent.join(".use"))?;
        let mut lock_name = path.file_name().expect("managed name").to_os_string();
        lock_name.push(".lock");
        let lock_path = parent.join(".use").join(lock_name);
        if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(io::Error::other("managed generation use lock is a symlink"));
        }
        let guard = solstone_core_artifact_download::generation::hold_write(&path)?;
        held.push((path, guard));
    }
    let mut removed = Vec::new();
    for (path, _use) in held {
        if plain_directory_below(&journal, &path)? {
            let mut members: Vec<(String, Option<String>)> = if path == runtime {
                pins::required_members_for(release, "aarch64-apple-darwin")
                    .ok_or_else(|| io::Error::other("predecessor runtime members unavailable"))?
                    .iter()
                    .map(|member| match member {
                        pins::RequiredMember::Regular(name) => (name.to_string(), None),
                        pins::RequiredMember::Link { name, target } => {
                            (name.to_string(), Some(target.to_string()))
                        }
                    })
                    .collect()
            } else if path == runtime_fetch {
                vec![
                    (filename.to_owned(), None),
                    (format!(".{filename}.{hash}.part"), None),
                    (format!(".{filename}.{hash}.part.json"), None),
                ]
            } else {
                let generation = pins::model_generation(MODEL)
                    .ok_or_else(|| io::Error::other("predecessor model members unavailable"))?;
                let mut members: Vec<_> = generation
                    .files
                    .into_iter()
                    .map(|member| (member.file, None))
                    .collect();
                members.extend([
                    (".generation.json".into(), None),
                    (".retiring.json".into(), None),
                ]);
                members
            };
            if path == runtime {
                let manifest = super::manifest::artifact_manifest_path(&path);
                match fs::symlink_metadata(&manifest) {
                    Ok(metadata)
                        if metadata.is_file()
                            && !metadata.file_type().is_symlink()
                            && metadata.len() <= 65536 =>
                    {
                        let record: Value = serde_json::from_slice(&fs::read(manifest)?)
                            .map_err(io::Error::other)?;
                        if record["schema_version"] != 1
                            || record["provider"] != "local"
                            || record["source"]["pin_identity"]
                                != pins::vulkan_identity("aarch64-apple-darwin")
                                    .unwrap_or(Value::Null)
                            || !record["external_root"].is_null()
                        {
                            return Err(io::Error::other(
                                "predecessor runtime manifest is unavailable",
                            ));
                        }
                        let inventory = record["inventory"].as_array().ok_or_else(|| {
                            io::Error::other("predecessor runtime inventory is unavailable")
                        })?;
                        for entry in inventory {
                            let name = entry["relative_path"].as_str().ok_or_else(|| {
                                io::Error::other("invalid predecessor runtime member")
                            })?;
                            if Path::new(name).components().count() != 1
                                || name.starts_with('.')
                                || !matches!(
                                    Path::new(name).components().next(),
                                    Some(std::path::Component::Normal(_))
                                )
                                || entry["size"].as_u64().is_none()
                                || !entry["sha256"].as_str().is_some_and(|hash| {
                                    hash.len() == 64
                                        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                                })
                            {
                                return Err(io::Error::other("invalid predecessor runtime member"));
                            }
                            if !members.iter().any(|(member, _)| member == name) {
                                members.push((name.into(), None));
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                    Ok(_) => {
                        return Err(io::Error::other(
                            "predecessor runtime manifest is not a regular file",
                        ));
                    }
                }
            }
            members.push((".solstone-provider-manifest.json".into(), None));
            let retained = if path == runtime_fetch {
                vec![format!("{filename}.lock")]
            } else {
                Vec::new()
            };
            retire_flat(&path, &members, &retained)?;
            if !path.exists() {
                removed.push(path);
            }
        }
    }
    // The old model downloader shared this transfer directory with other
    // models. Remove only this pin's own files and resumable sidecars.
    if let Some(fetch) = pins::model_fetch_directory(&journal, MODEL)
        && plain_directory_below(&journal, &fetch)?
        && let Some(generation) = pins::model_generation(MODEL)
    {
        for member in generation.files {
            let partial = format!(".{}.{}.part", member.file, member.sha256);
            for name in [member.file, format!("{partial}.json"), partial] {
                let path = fetch.join(name);
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                        fs::remove_file(&path)?;
                        removed.push(path);
                    }
                    Ok(_) => {
                        return Err(io::Error::other(
                            "predecessor transfer is not a regular file",
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_record_is_closed_and_never_confuses_damage_with_absence() {
        assert_eq!(
            parse(br#"{"schema":1,"state":"in-progress"}"#).unwrap(),
            MigrationState::InProgress
        );
        assert_eq!(
            parse(br#"{"schema":1,"state":"committed"}"#).unwrap(),
            MigrationState::Committed
        );
        for bytes in [
            b"".as_slice(),
            br#"{"schema":2,"state":"committed"}"#,
            br#"{"schema":1,"state":"old"}"#,
            br#"{"schema":1,"state":"committed","extra":true}"#,
        ] {
            assert!(parse(bytes).is_err());
        }
    }

    #[cfg(feature = "full-tests")]
    fn materialize(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("fixture"), b"managed predecessor").unwrap();
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn decision_survives_restart_and_damage_never_reopens_it() {
        let journal = tempfile::tempdir().unwrap();
        assert_eq!(read(journal.path()).unwrap(), MigrationState::Absent);
        assert_eq!(start(journal.path()).unwrap(), MigrationState::InProgress);
        assert_eq!(start(journal.path()).unwrap(), MigrationState::InProgress);
        commit(journal.path()).unwrap();
        assert_eq!(start(journal.path()).unwrap(), MigrationState::Committed);
        commit(journal.path()).unwrap();
        assert_eq!(read(journal.path()).unwrap(), MigrationState::Committed);
        let path = journal.path().join("config").join(RECORD);
        for damaged in [b"{".as_slice(), br#"{"schema":2,"state":"committed"}"#] {
            fs::write(&path, damaged).unwrap();
            assert!(read(journal.path()).is_err());
            assert!(start(journal.path()).is_err());
            assert!(commit(journal.path()).is_err());
            assert!(retire_predecessor(journal.path()).is_err());
            assert_eq!(fs::read(&path).unwrap(), damaged);
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn retirement_is_committed_leased_narrow_and_resumes_after_partial_deletion() {
        use solstone_core_artifact_download::generation::hold_use;
        let journal = tempfile::tempdir().unwrap();
        let cache = pins::cache_root(journal.path());
        let (release, _, hash, _) = pins::vulkan_pin("aarch64-apple-darwin").unwrap();
        let runtime = cache.join("bin/aarch64-apple-darwin").join(release);
        let model = pins::model_directory(journal.path(), MODEL);
        let other = cache.join("models/owner-model");
        let mlx = pins::mlx_model_directory(journal.path());
        let unrelated_runtime = runtime.parent().unwrap().join("owner-runtime");
        let runtime_fetch = cache.join(".fetch").join(hash);
        for path in [&other, &mlx, &unrelated_runtime] {
            materialize(path);
        }
        for (path, name) in [
            (&runtime, "llama-server"),
            (&model, "Qwen3.5-4B-Q4_K_M.gguf"),
            (
                &runtime_fetch,
                pins::vulkan_pin("aarch64-apple-darwin").unwrap().1,
            ),
        ] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join(name), b"managed predecessor").unwrap();
        }
        let download_lock = runtime_fetch.join(format!(
            "{}.lock",
            pins::vulkan_pin("aarch64-apple-darwin").unwrap().1
        ));
        fs::write(&download_lock, b"stable download lock").unwrap();
        let fetch = pins::model_fetch_directory(journal.path(), MODEL).unwrap();
        materialize(&fetch);
        let member = pins::model_generation(MODEL).unwrap().files.remove(0);
        fs::write(fetch.join(&member.file), b"old transfer").unwrap();
        let partial = fetch.join(format!(".{}.{}.part", member.file, member.sha256));
        fs::write(&partial, b"partial").unwrap();
        fs::write(partial.with_extension("part.json"), b"binding").unwrap();
        start(journal.path()).unwrap();
        assert!(retire_predecessor(journal.path()).is_err());
        commit(journal.path()).unwrap();
        let in_use = hold_use(&model).unwrap();
        assert_eq!(
            retire_predecessor(journal.path()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(runtime.exists(), "all leases precede every deletion");
        drop(in_use);
        // Model the interruption between two completed managed-tree removals.
        fs::remove_dir_all(&runtime).unwrap();
        retire_predecessor(journal.path()).unwrap();
        assert!(!model.exists());
        assert_eq!(fs::read(&download_lock).unwrap(), b"stable download lock");
        assert!(
            !runtime_fetch
                .join(pins::vulkan_pin("aarch64-apple-darwin").unwrap().1)
                .exists()
        );
        assert!(!fetch.join(&member.file).exists() && !partial.exists());
        for path in [&other, &mlx, &unrelated_runtime, &fetch] {
            assert_eq!(
                fs::read(path.join("fixture")).unwrap(),
                b"managed predecessor"
            );
        }
        assert!(retire_predecessor(journal.path()).unwrap().is_empty());
        assert_eq!(read(journal.path()).unwrap(), MigrationState::Committed);
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn retirement_preserves_unknown_files_and_refuses_changed_library_links() {
        use std::os::unix::fs::symlink;
        let journal = tempfile::tempdir().unwrap();
        commit(journal.path()).unwrap();
        let runtime = pins::cache_root(journal.path()).join("bin/aarch64-apple-darwin/b11669");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join("llama-server"), b"managed").unwrap();
        fs::write(runtime.join("owner-file"), b"preserve").unwrap();
        assert!(retire_predecessor(journal.path()).is_err());
        assert_eq!(fs::read(runtime.join("owner-file")).unwrap(), b"preserve");
        assert_eq!(fs::read(runtime.join("llama-server")).unwrap(), b"managed");
        fs::remove_file(runtime.join("owner-file")).unwrap();
        // The signed archive also contains auxiliary CLI binaries beyond the
        // launch-required subset. Its verified provider inventory names them.
        fs::write(runtime.join("llama-auxiliary"), b"auxiliary").unwrap();
        let inventory =
            super::super::manifest::inventory_for_tree(&runtime, "runtime_support").unwrap();
        let manifest = super::super::manifest::build_manifest(
            "local",
            "llama-server-vulkan",
            "fixture",
            json!({"pin_identity":pins::vulkan_identity("aarch64-apple-darwin").unwrap()}),
            inventory,
            None,
            None,
        )
        .unwrap();
        super::super::manifest::write_manifest(
            &super::super::manifest::artifact_manifest_path(&runtime),
            &manifest,
        )
        .unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("library"), b"preserve").unwrap();
        symlink(
            outside.path().join("library"),
            runtime.join("libllama.dylib"),
        )
        .unwrap();
        assert!(retire_predecessor(journal.path()).is_err());
        assert!(runtime.join("llama-server").exists());
        assert_eq!(
            fs::read(outside.path().join("library")).unwrap(),
            b"preserve"
        );
        fs::remove_file(runtime.join("libllama.dylib")).unwrap();
        symlink("libllama.0.dylib", runtime.join("libllama.dylib")).unwrap();
        retire_predecessor(journal.path()).unwrap();
        assert!(!runtime.exists());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn linked_authority_and_cache_roots_never_authorize_writes_or_deletion() {
        use std::os::unix::fs::symlink;
        let journal = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        materialize(external.path());
        symlink(external.path(), journal.path().join("config")).unwrap();
        assert!(read(journal.path()).is_err());
        assert!(start(journal.path()).is_err());
        assert!(commit(journal.path()).is_err());
        fs::remove_file(journal.path().join("config")).unwrap();
        start(journal.path()).unwrap();
        let record = journal.path().join("config").join(RECORD);
        fs::remove_file(&record).unwrap();
        symlink(external.path().join("fixture"), &record).unwrap();
        assert!(read(journal.path()).is_err());
        assert!(commit(journal.path()).is_err());
        fs::remove_file(&record).unwrap();
        commit(journal.path()).unwrap();
        let cache = pins::cache_root(journal.path());
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        symlink(external.path(), &cache).unwrap();
        assert!(retire_predecessor(journal.path()).is_err());
        assert_eq!(
            fs::read(external.path().join("fixture")).unwrap(),
            b"managed predecessor"
        );
        assert_eq!(read(journal.path()).unwrap(), MigrationState::Committed);
    }
}
