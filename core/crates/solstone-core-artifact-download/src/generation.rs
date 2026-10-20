// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Immutable downloaded data sets. Control records and transfer checkpoints stay
//! outside these directories so older releases cannot sweep newer generations.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockOptions, atomic_replace, hold_lock, hold_shared_lock,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const MANIFEST: &str = ".generation.json";
const RETIRING: &str = ".retiring.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub file: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    schema: u32,
    pub unit: String,
    #[serde(default)]
    pub identity: serde_json::Value,
    pub files: Vec<Member>,
}

fn plain(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !meta.file_type().is_symlink()
}

fn segment(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.ends_with(['.', ' '])
}

impl Generation {
    pub fn new(unit: &str, mut files: Vec<Member>) -> io::Result<Self> {
        files.sort_by(|a, b| a.file.cmp(&b.file));
        if files.is_empty()
            || files.windows(2).any(|p| p[0].file == p[1].file)
            || files.iter().any(|m| {
                !segment(&m.file)
                    || m.sha256.len() != 64
                    || !m.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid artifact generation inventory",
            ));
        }
        Ok(Self {
            schema: 1,
            unit: unit.into(),
            identity: serde_json::Value::Null,
            files,
        })
    }

    pub fn key(&self) -> String {
        format!(
            "g-{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("serializable generation"))
        )
    }

    pub fn directory(&self, root: &Path) -> PathBuf {
        root.join(self.key())
    }

    pub fn verify(&self, directory: &Path) -> bool {
        self.files.iter().all(|m| {
            let path = directory.join(&m.file);
            fs::symlink_metadata(&path).is_ok_and(|meta| {
                plain(&meta) && meta.file_type().is_file() && meta.len() == m.size_bytes
            }) && super::verify_sha256(&path, &m.sha256).is_ok()
        })
    }

    pub fn is_sealed(&self, directory: &Path) -> bool {
        read(directory).as_ref() == Some(self)
            && self.exact_entries(directory, true)
            && self.verify(directory)
    }

    /// Caller holds the destination's exclusive use lock. Retain immutable
    /// legacy bytes without relocating them or allocating another model copy.
    pub fn back_from(&self, source: &Path, destination: &Path) -> io::Result<()> {
        fs::create_dir_all(destination)?;
        for member in &self.files {
            let target = destination.join(&member.file);
            if fs::symlink_metadata(&target).is_ok() {
                if !super::verify_sha256(&target, &member.sha256).is_ok() {
                    return Err(io::Error::other(
                        "generation member differs from its immutable pin",
                    ));
                }
            } else {
                fs::hard_link(source.join(&member.file), target)?;
            }
        }
        if self.unit == "local-model" {
            let name = ".solstone-provider-manifest.json";
            let metadata = fs::symlink_metadata(source.join(name))?;
            if !plain(&metadata) || !metadata.is_file() || metadata.len() > 65536 {
                return Err(io::Error::other("invalid legacy provider manifest"));
            }
            atomic_replace(
                destination.join(name),
                &fs::read(source.join(name))?,
                AtomicWriteOptions::default(),
            )
            .map_err(io::Error::other)?;
        }
        self.seal(destination)
    }

    /// Seal only an exact, verified flat inventory. The manifest is published last.
    pub fn seal(&self, directory: &Path) -> io::Result<()> {
        let marker = directory.join(RETIRING);
        if self.verify(directory)
            && fs::read(&marker)
                .ok()
                .and_then(|b| serde_json::from_slice::<Generation>(&b).ok())
                .as_ref()
                == Some(self)
        {
            fs::remove_file(marker)?;
        }
        if !self.verify(directory) || !self.exact_entries(directory, false) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "artifact generation is incomplete or contains unrecognized entries",
            ));
        }
        atomic_replace(
            directory.join(MANIFEST),
            &serde_json::to_vec(self)?,
            AtomicWriteOptions::default(),
        )
        .map_err(io::Error::other)
    }

    /// Resume an admitted retirement after a transient unlink failure. Unknown
    /// additions or changed remaining bytes stop deletion; missing members are
    /// evidence only after this exact retirement record was durably written.
    pub fn retire(&self, directory: &Path) -> io::Result<()> {
        if !fs::symlink_metadata(directory).is_ok_and(|m| plain(&m) && m.is_dir()) {
            return Err(io::Error::other(
                "generation retirement root is not a plain directory",
            ));
        }
        let marker = directory.join(RETIRING);
        let retiring = fs::read(&marker)
            .ok()
            .and_then(|b| serde_json::from_slice::<Generation>(&b).ok())
            .as_ref()
            == Some(self);
        if !retiring {
            if !self.exact_entries(directory, false) || !self.verify(directory) {
                return Err(io::Error::other("generation retirement inventory changed"));
            }
            atomic_replace(
                &marker,
                &serde_json::to_vec(self)?,
                AtomicWriteOptions::default(),
            )
            .map_err(io::Error::other)?;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let metadata = fs::symlink_metadata(entry.path())?;
            if !plain(&metadata) || !metadata.is_file() {
                return Err(io::Error::other("unknown retirement entry"));
            }
            if name == RETIRING
                || name == MANIFEST
                || (self.unit == "local-model" && name == ".solstone-provider-manifest.json")
            {
                continue;
            }
            let Some(member) = self.files.iter().find(|m| name == m.file.as_str()) else {
                return Err(io::Error::other("unknown retirement entry"));
            };
            if metadata.len() != member.size_bytes
                || super::verify_sha256(&entry.path(), &member.sha256).is_err()
            {
                return Err(io::Error::other("retiring generation member changed"));
            }
        }
        for member in &self.files {
            match fs::remove_file(directory.join(&member.file)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        for name in [MANIFEST, ".solstone-provider-manifest.json"] {
            if name.starts_with(".solstone") && self.unit != "local-model" {
                continue;
            }
            match fs::remove_file(directory.join(name)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        fs::remove_file(marker)?;
        fs::remove_dir(directory)
    }

    fn exact_entries(&self, directory: &Path, require_manifest: bool) -> bool {
        let Ok(entries) = fs::read_dir(directory) else {
            return false;
        };
        let mut count = 0;
        let mut manifest = false;
        for entry in entries {
            let Ok(entry) = entry else {
                return false;
            };
            if !fs::symlink_metadata(entry.path())
                .is_ok_and(|m| plain(&m) && m.file_type().is_file())
            {
                return false;
            }
            let name = entry.file_name();
            if name == MANIFEST {
                manifest = true;
            } else if self.unit == "local-model" && name == ".solstone-provider-manifest.json" {
                // The provider owns this control record; it never becomes a model member.
            } else if self.files.iter().any(|m| name == m.file.as_str()) {
                count += 1;
            } else {
                return false;
            }
        }
        count == self.files.len() && (!require_manifest || manifest)
    }
}

fn read(directory: &Path) -> Option<Generation> {
    if !fs::symlink_metadata(directory)
        .ok()
        .is_some_and(|m| plain(&m) && m.file_type().is_dir())
    {
        return None;
    }
    let path = if directory.join(MANIFEST).exists() {
        directory.join(MANIFEST)
    } else {
        directory.join(RETIRING)
    };
    read_record(&path)
}

/// Recover only a previously admitted retirement, including after its original
/// provider manifest was removed. Callers still apply current-use protection.
pub fn pending_retirement(directory: &Path) -> Option<Generation> {
    if !fs::symlink_metadata(directory).is_ok_and(|m| plain(&m) && m.is_dir()) {
        return None;
    }
    read_record(&directory.join(RETIRING))
}

fn read_record(path: &Path) -> Option<Generation> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !plain(&meta) || !meta.file_type().is_file() || meta.len() > 65536 {
        return None;
    }
    let parsed: Generation = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let mut normalized = Generation::new(&parsed.unit, parsed.files.clone()).ok()?;
    normalized.identity = parsed.identity.clone();
    (parsed == normalized).then_some(parsed)
}

pub fn reusable_member(root: &Path, member: &Member) -> Option<PathBuf> {
    if !fs::symlink_metadata(root)
        .ok()
        .is_some_and(|m| plain(&m) && m.is_dir())
    {
        return None;
    }
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        let Some(record) = read(&path) else {
            continue;
        };
        if record.files.contains(member) {
            let candidate = path.join(&member.file);
            if fs::symlink_metadata(&candidate)
                .is_ok_and(|m| plain(&m) && m.is_file() && m.len() == member.size_bytes)
                && super::verify_sha256(&candidate, &member.sha256).is_ok()
            {
                return Some(candidate);
            }
        }
    }
    None
}

fn use_path(directory: &Path) -> io::Result<PathBuf> {
    let name = directory
        .file_name()
        .ok_or_else(|| io::Error::other("generation has no name"))?;
    Ok(directory
        .parent()
        .ok_or_else(|| io::Error::other("generation has no parent"))?
        .join(".use")
        .join(name))
}

/// Keep this guard until the last native reader or process has stopped.
pub fn hold_use(directory: &Path) -> io::Result<FileLock> {
    hold_shared_lock(use_path(directory)?, LockOptions::default()).map_err(io::Error::other)
}

pub fn hold_write(directory: &Path) -> io::Result<FileLock> {
    hold_lock(
        use_path(directory)?,
        LockOptions {
            timeout: Duration::ZERO,
            ..LockOptions::default()
        },
    )
    .map_err(|error| match error {
        solstone_core_journal_io::LockError::Timeout(_) => {
            io::Error::new(io::ErrorKind::WouldBlock, "generation is in use")
        }
        error => io::Error::other(error),
    })
}

/// Delete only sealed, exact, unchanged generations, under a real exclusive
/// lock. Unknown content and unavailable predecessor metadata retain bytes.
pub fn collect(
    root: &Path,
    unit: &str,
    current: &[Generation],
    preserve_previous: bool,
) -> io::Result<Vec<PathBuf>> {
    let predecessor = if preserve_previous {
        match solstone_core_assets::windows_predecessor::retention_predecessor() {
            Some(Ok(record)) => Some(record),
            Some(Err(reason)) => {
                log::warn!("{reason}");
                return Ok(Vec::new());
            }
            None => None,
        }
    } else {
        None
    };
    match fs::symlink_metadata(root) {
        Ok(m) if plain(&m) && m.file_type().is_dir() => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        _ => return Err(io::Error::other("generation root is not a plain directory")),
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut removed = Vec::new();
    for entry in entries {
        let entry = entry?;
        let directory = entry.path();
        let Some(record) = read(&directory) else {
            continue;
        };
        if record.unit != unit {
            continue;
        }
        if directory
            .file_name()
            .is_none_or(|name| name != record.key().as_str())
            || current.contains(&record)
        {
            continue;
        }
        if predecessor.is_some_and(|p| {
            p.protects(
                &record.unit,
                &record
                    .files
                    .iter()
                    .map(|m| (m.sha256.as_str(), m.size_bytes))
                    .collect::<Vec<_>>(),
            )
        }) {
            continue;
        }
        let Ok(_hold) = hold_lock(
            use_path(&directory)?,
            LockOptions {
                timeout: Duration::ZERO,
                ..LockOptions::default()
            },
        ) else {
            continue;
        };
        if read(&directory).as_ref() != Some(&record) {
            continue;
        }
        let retiring = fs::read(directory.join(RETIRING))
            .ok()
            .and_then(|b| serde_json::from_slice::<Generation>(&b).ok())
            .as_ref()
            == Some(&record);
        if !retiring && (!record.exact_entries(&directory, true) || !record.verify(&directory)) {
            continue;
        }
        match record.retire(&directory) {
            Ok(()) => removed.push(directory),
            Err(error) => log::warn!("could not retire downloaded generation: {error}"),
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn generation(root: &Path, bytes: &[u8]) -> (Generation, PathBuf) {
        let record = Generation::new(
            "test",
            vec![Member {
                file: "model".into(),
                sha256: format!("{:x}", Sha256::digest(bytes)),
                size_bytes: bytes.len() as u64,
            }],
        )
        .unwrap();
        let path = record.directory(root);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("model"), bytes).unwrap();
        record.seal(&path).unwrap();
        (record, path)
    }
    #[test]
    fn collection_preserves_current_active_unknown_and_changed_generations() {
        let root = tempfile::tempdir().unwrap();
        let (_, obsolete) = generation(root.path(), b"obsolete");
        let (current, current_path) = generation(root.path(), b"current");
        let (_, active) = generation(root.path(), b"active");
        let active_hold = hold_use(&active).unwrap();
        let (_, unknown) = generation(root.path(), b"unknown");
        fs::write(unknown.join("owner-file"), b"preserve").unwrap();
        let (_, changed) = generation(root.path(), b"changed");
        fs::write(changed.join("model"), b"corrupt").unwrap();
        assert_eq!(
            collect(root.path(), "test", std::slice::from_ref(&current), false).unwrap(),
            vec![obsolete.clone()]
        );
        assert!(!obsolete.exists());
        assert!(active.exists());
        assert!(current_path.exists());
        assert!(unknown.exists());
        assert!(changed.exists());
        drop(active_hold);
        assert_eq!(
            collect(root.path(), "test", std::slice::from_ref(&current), false).unwrap(),
            vec![active]
        );
        assert!(
            collect(root.path(), "test", std::slice::from_ref(&current), false)
                .unwrap()
                .is_empty()
        );
    }
    #[cfg(feature = "runtime-fetch-test")]
    #[test]
    fn three_generations_keep_current_supported_predecessor_and_live_use() {
        use solstone_core_assets::windows_predecessor::{
            windows_predecessor, with_windows_predecessor_fixture,
        };
        let root = tempfile::tempdir().unwrap();
        let make = |value: u8| {
            let files: Vec<_> = ["containment.slpk", "venues.slpk"]
                .into_iter()
                .map(|name| Member {
                    file: name.into(),
                    sha256: format!("{:x}", Sha256::digest([value])),
                    size_bytes: 1,
                })
                .collect();
            let record = Generation::new("location-accuracy", files).unwrap();
            let path = record.directory(root.path());
            fs::create_dir_all(&path).unwrap();
            for f in &record.files {
                fs::write(path.join(&f.file), [value]).unwrap();
            }
            record.seal(&path).unwrap();
            (record, path)
        };
        let (_, a) = make(b'a');
        let (b, b_path) = make(b'b');
        let (c, c_path) = make(b'c');
        let mut previous = windows_predecessor().unwrap().clone();
        for asset in &mut previous
            .predecessor
            .groups
            .iter_mut()
            .find(|g| g.unit == "location-accuracy")
            .unwrap()
            .assets
        {
            let member = b
                .files
                .iter()
                .find(|m| asset.origin_key.ends_with(&format!("/{}", m.file)))
                .unwrap();
            asset.sha256 = member.sha256.clone();
            asset.size_bytes = member.size_bytes;
        }
        let hold = hold_use(&a).unwrap();
        with_windows_predecessor_fixture(previous.clone(), || {
            assert!(
                collect(
                    root.path(),
                    "location-accuracy",
                    std::slice::from_ref(&c),
                    true
                )
                .unwrap()
                .is_empty()
            );
            assert!(a.exists() && b_path.exists() && c_path.exists());
        });
        drop(hold);
        let mut malformed = previous.clone();
        malformed.schema_version = 999;
        with_windows_predecessor_fixture(malformed, || {
            assert!(
                collect(
                    root.path(),
                    "location-accuracy",
                    std::slice::from_ref(&c),
                    true
                )
                .unwrap()
                .is_empty()
            )
        });
        assert!(a.exists());
        with_windows_predecessor_fixture(previous, || {
            assert_eq!(
                collect(
                    root.path(),
                    "location-accuracy",
                    std::slice::from_ref(&c),
                    true
                )
                .unwrap(),
                vec![a.clone()]
            );
            for _ in 0..3 {
                assert!(
                    collect(
                        root.path(),
                        "location-accuracy",
                        std::slice::from_ref(&c),
                        true
                    )
                    .unwrap()
                    .is_empty()
                );
            }
        });
        assert!(b_path.exists() && c_path.exists());
    }

    #[test]
    fn admitted_retirement_retries_missing_members_but_keeps_unknown_owner_control() {
        let root = tempfile::tempdir().unwrap();
        let (record, path) = generation(root.path(), b"bytes");
        fs::write(path.join(RETIRING), serde_json::to_vec(&record).unwrap()).unwrap();
        fs::remove_file(path.join("model")).unwrap();
        assert_eq!(
            collect(root.path(), "test", &[], false).unwrap(),
            vec![path.clone()]
        );
        assert!(!path.exists());
        let record = Generation::new(
            "location-accuracy",
            vec![Member {
                file: "venues.slpk".into(),
                sha256: format!("{:x}", Sha256::digest(b"pack")),
                size_bytes: 4,
            }],
        )
        .unwrap();
        let path = record.directory(root.path());
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("venues.slpk"), b"pack").unwrap();
        record.seal(&path).unwrap();
        fs::write(path.join(".solstone-provider-manifest.json"), b"owner-file").unwrap();
        assert!(
            collect(root.path(), "location-accuracy", &[], false)
                .unwrap()
                .is_empty()
        );
        assert!(path.exists());
    }
}
