// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Conservative retirement of the one legacy optional-model lookup directory.

use serde_json::Value;
use solstone_core_artifact_download::generation::{
    Generation, Member, hold_write, pending_retirement,
};
use std::{fs, io, path::Path};

fn valid(record: &Generation) -> bool {
    let identity = &record.identity;
    record.unit == "local-model"
        && identity["unit"] == "local-model"
        && identity["model_id"] == "local/qwen3.5-4b"
        && ["repo", "revision"]
            .iter()
            .all(|key| identity[*key].as_str().is_some_and(|s| !s.is_empty()))
        && record.files.len() == 2
        && identity["filename"] != identity["mmproj_filename"]
        && [("filename", "sha256"), ("mmproj_filename", "mmproj_sha256")]
            .iter()
            .all(|(name, sha)| {
                record
                    .files
                    .iter()
                    .filter(|m| {
                        Some(m.file.as_str()) == identity[*name].as_str()
                            && Some(m.sha256.as_str()) == identity[*sha].as_str()
                            && m.size_bytes > 0
                    })
                    .count()
                    == 1
            })
}

fn plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return false;
            }
        }
        m.is_dir() && !m.file_type().is_symlink()
    })
}

fn read(directory: &Path) -> Option<Generation> {
    if !plain_directory(directory) || !directory.parent().is_some_and(plain_directory) {
        return None;
    }
    // A flat install never wrote the sibling-generation control record.
    if fs::symlink_metadata(directory.join(".generation.json")).is_ok() {
        return None;
    }
    if let Some(record) = pending_retirement(directory) {
        return valid(&record).then_some(record);
    }
    let path = super::manifest::artifact_manifest_path(directory);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65536 {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    let manifest: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if manifest["schema_version"] != 1
        || manifest["provider"] != "local"
        || manifest["unit"] != "local-model"
        || !manifest["external_root"].is_null()
    {
        return None;
    }
    let files = manifest["inventory"]
        .as_array()?
        .iter()
        .map(|entry| {
            (entry["role"] == "model").then_some(())?;
            Some(Member {
                file: entry["relative_path"].as_str()?.into(),
                sha256: entry["sha256"].as_str()?.into(),
                size_bytes: entry["size"].as_u64()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let mut record = Generation::new("local-model", files).ok()?;
    record.identity = manifest["source"]["pin_identity"].clone();
    valid(&record).then_some(record)
}

pub(super) fn collect_legacy(journal: &Path, current: &Generation) -> io::Result<()> {
    let predecessor = match solstone_core_assets::windows_predecessor::retention_predecessor() {
        Some(Ok(p)) => Some(p),
        Some(Err(reason)) => {
            log::warn!("{reason}");
            return Ok(());
        }
        None => None,
    };
    let flat = super::pins::cache_root(journal).join("models/local__qwen3.5-4b");
    let Some(record) = read(&flat) else {
        return Ok(());
    };
    if record.files == current.files
        || predecessor.is_some_and(|p| {
            p.protects(
                "local-model",
                &record
                    .files
                    .iter()
                    .map(|f| (f.sha256.as_str(), f.size_bytes))
                    .collect::<Vec<_>>(),
            )
        })
    {
        return Ok(());
    }
    let _hold = match hold_write(&flat) {
        Ok(h) => h,
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(e) => return Err(e),
    };
    if read(&flat).as_ref() != Some(&record) {
        return Ok(());
    }
    record.retire(&flat)
}

#[cfg(all(test, feature = "runtime-fetch-test"))]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use solstone_core_artifact_download::generation::{collect, hold_use};
    use solstone_core_assets::windows_predecessor::{
        windows_predecessor, with_windows_predecessor_fixture,
    };

    pub(super) fn record(model: u8, projector: u8) -> Generation {
        let files = [
            ("Qwen3.5-4B-Q4_K_M.gguf", model),
            ("mmproj-F16.gguf", projector),
        ]
        .into_iter()
        .map(|(file, b)| Member {
            file: file.into(),
            sha256: format!("{:x}", Sha256::digest([b])),
            size_bytes: 1,
        })
        .collect();
        let mut g = Generation::new("local-model", files).unwrap();
        g.identity = json!({"unit":"local-model","model_id":"local/qwen3.5-4b","repo":"fixture","revision":"fixture","filename":g.files[0].file,"sha256":g.files[0].sha256,"mmproj_filename":g.files[1].file,"mmproj_sha256":g.files[1].sha256});
        g
    }
    pub(super) fn materialize(path: &Path, g: &Generation, model: u8, projector: u8) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join(&g.files[0].file), [model]).unwrap();
        fs::write(path.join(&g.files[1].file), [projector]).unwrap();
        let manifest = super::super::manifest::build_manifest(
            "local",
            "local-model",
            "fixture",
            json!({"pin_identity":g.identity}),
            super::super::manifest::inventory_for_members(
                path,
                &[&g.files[0].file, &g.files[1].file],
                "model",
            )
            .unwrap(),
            None,
            None,
        )
        .unwrap();
        super::super::manifest::write_manifest(
            &super::super::manifest::artifact_manifest_path(path),
            &manifest,
        )
        .unwrap();
    }
    fn predecessor(
        g: &Generation,
    ) -> solstone_core_assets::windows_predecessor::WindowsPredecessor {
        let mut p = windows_predecessor().unwrap().clone();
        for a in &mut p
            .predecessor
            .groups
            .iter_mut()
            .find(|g| g.unit == "local-model")
            .unwrap()
            .assets
        {
            let m = g
                .files
                .iter()
                .find(|m| a.origin_key.ends_with(&format!("/{}", m.file)))
                .unwrap();
            a.sha256 = m.sha256.clone();
            a.size_bytes = m.size_bytes;
        }
        p
    }
    #[test]
    fn duplicate_identity_roles_do_not_admit_an_unbound_member() {
        let mut malformed = record(b'a', b'b');
        malformed.identity["mmproj_filename"] = malformed.identity["filename"].clone();
        malformed.identity["mmproj_sha256"] = malformed.identity["sha256"].clone();
        assert!(!valid(&malformed));
    }

    #[test]
    fn flat_cleanup_frees_the_obsolete_last_link_and_keeps_current_previous_and_active() {
        let journal = tempfile::tempdir().unwrap();
        let root = super::super::pins::cache_root(journal.path()).join("models");
        let flat = root.join("local__qwen3.5-4b");
        let a = record(b'a', b'a');
        let b = record(b'b', b'b');
        let c = record(b'c', b'c');
        materialize(&flat, &a, b'a', b'a');
        let a_back = a.directory(&root);
        a.back_from(&flat, &a_back).unwrap();
        for (g, byte) in [(&b, b'b'), (&c, b'c')] {
            let path = g.directory(&root);
            materialize(&path, g, byte, byte);
            g.seal(&path).unwrap();
        }
        with_windows_predecessor_fixture(predecessor(&a), || {
            collect_legacy(journal.path(), &c).unwrap();
            assert!(flat.exists());
        });
        with_windows_predecessor_fixture(predecessor(&b), || {
            collect_legacy(journal.path(), &a).unwrap();
            assert!(flat.exists());
            let hold = hold_use(&flat).unwrap();
            collect_legacy(journal.path(), &c).unwrap();
            assert!(flat.exists());
            drop(hold);
            fs::write(flat.join("owner-file"), b"preserve").unwrap();
            assert!(collect_legacy(journal.path(), &c).is_err());
            assert!(flat.exists());
            fs::remove_file(flat.join("owner-file")).unwrap();
            fs::write(flat.join(".generation.json"), b"owner control").unwrap();
            collect_legacy(journal.path(), &c).unwrap();
            assert_eq!(
                fs::read(flat.join(".generation.json")).unwrap(),
                b"owner control"
            );
            assert!(a.verify(&flat));
            fs::remove_file(flat.join(".generation.json")).unwrap();
            collect_legacy(journal.path(), &c).unwrap();
            collect(&root, "local-model", std::slice::from_ref(&c), true).unwrap();
            assert!(!flat.exists() && !a_back.exists());
            assert!(b.directory(&root).exists() && c.directory(&root).exists());
        });
    }
    #[test]
    fn partial_retirement_recovers_without_provider_manifest_and_reuse_reduces_admission() {
        let journal = tempfile::tempdir().unwrap();
        let root = super::super::pins::cache_root(journal.path()).join("models");
        let flat = root.join("local__qwen3.5-4b");
        let a = record(b'a', b'a');
        let c = record(b'a', b'c');
        materialize(&flat, &a, b'a', b'a');
        let needed = super::super::pins::remaining_model_member_bytes(
            journal.path(),
            "local/qwen3.5-4b",
            &c.files[0],
            &c.directory(&root),
            &root.join("fetch"),
        );
        assert_eq!(needed, 0);
        fs::write(flat.join(".retiring.json"), serde_json::to_vec(&a).unwrap()).unwrap();
        fs::remove_file(flat.join(&a.files[0].file)).unwrap();
        fs::remove_file(super::super::manifest::artifact_manifest_path(&flat)).unwrap();
        with_windows_predecessor_fixture(predecessor(&c), || {
            collect_legacy(journal.path(), &c).unwrap()
        });
        assert!(!flat.exists());
        assert_eq!(
            super::super::pins::remaining_model_member_bytes(
                journal.path(),
                "local/qwen3.5-4b",
                &c.files[0],
                &c.directory(&root),
                &root.join("fetch")
            ),
            1
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(feature = "runtime-fetch-test")]
#[test]
fn linked_legacy_root_and_directory_never_authorize_retirement() {
    let journal = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let a = tests::record(b'a', b'a');
    let c = tests::record(b'c', b'c');
    tests::materialize(outside.path(), &a, b'a', b'a');
    let models = super::pins::cache_root(journal.path()).join("models");
    fs::create_dir_all(&models).unwrap();
    let flat = models.join("local__qwen3.5-4b");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &flat).unwrap();
    #[cfg(windows)]
    {
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&flat)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "junction fixture: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    collect_legacy(journal.path(), &c).unwrap();
    assert!(a.verify(outside.path()));
    #[cfg(unix)]
    fs::remove_file(&flat).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&flat).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let external_models = parent.path().join("models");
    fs::create_dir(&external_models).unwrap();
    tests::materialize(&external_models.join("local__qwen3.5-4b"), &a, b'a', b'a');
    fs::remove_dir(&models).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external_models, &models).unwrap();
    #[cfg(windows)]
    {
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&models)
            .arg(&external_models)
            .output()
            .unwrap();
        assert!(result.status.success());
    }
    collect_legacy(journal.path(), &c).unwrap();
    assert!(a.verify(&external_models.join("local__qwen3.5-4b")));
    #[cfg(unix)]
    fs::remove_file(&models).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&models).unwrap();
}
