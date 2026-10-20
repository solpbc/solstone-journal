// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fingerprint of the inputs a shared daily location part is rendered from.
//!
//! A successful read, including a day with no location files, is a SHA-256 hex
//! of those inputs. An error is never hashed.

use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::accuracy::read_installed_strict;
use crate::day::LOCATION_FILE;
use crate::places::PlaceStore;

/// Pending-only fingerprint of raw observations and their canonical naming inputs.
pub fn daily_location_fingerprint(journal: &Path, day: &str) -> io::Result<String> {
    fingerprint_with_area_identity(journal, day, crate::asset::AREA_TIER_SHA256)
}

fn fingerprint_with_area_identity(
    journal: &Path,
    day: &str,
    area_identity: &str,
) -> io::Result<String> {
    let mut hasher = Sha256::new();
    hash_segments(&mut hasher, journal, day)?;
    let store = PlaceStore::read_strict(journal)?;
    write_len_bytes(
        &mut hasher,
        &serde_json::to_vec(&store).map_err(io::Error::other)?,
    );
    hash_accuracy(&mut hasher, journal)?;
    write_len_bytes(&mut hasher, area_identity.as_bytes());
    // Pin effective wording too: the canonical reader consumes device
    // attribution, zone metadata and place association, not names alone.
    let view = crate::read_journal_day(journal, day).map_err(io::Error::other)?;
    write_len_bytes(
        &mut hasher,
        crate::projection::render_day_document(&view, &store).as_bytes(),
    );
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_segments(hasher: &mut Sha256, journal: &Path, day: &str) -> io::Result<()> {
    let mut files = Vec::new();
    collect_location_files(&journal.join("chronicle").join(day), journal, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    hasher.update(b"segments");
    hasher.update((files.len() as u64).to_be_bytes());
    for (path, bytes) in files {
        write_len_bytes(hasher, path.as_bytes());
        write_len_bytes(hasher, &bytes);
    }
    Ok(())
}

fn collect_location_files(
    directory: &Path,
    journal: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_location_files(&path, journal, files)?;
            continue;
        }
        if !kind.is_file() || path.file_name() != Some(LOCATION_FILE.as_ref()) {
            continue;
        }
        let relative = path
            .strip_prefix(journal)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let relative = relative.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "location segment path is not UTF-8",
            )
        })?;
        let bytes = fs::read(&path)?;
        files.push((relative.replace('\\', "/"), bytes));
    }
    Ok(())
}

fn hash_accuracy(hasher: &mut Sha256, journal: &Path) -> io::Result<()> {
    hasher.update(b"accuracy");
    let publication = crate::accuracy::hold_publication(journal).map_err(io::Error::other)?;
    let Some(installed) = read_installed_strict(journal)? else {
        hasher.update(b"absent");
        return Ok(());
    };
    let _generation_hold = crate::cache::hold_use(journal, &installed)?;
    drop(publication);
    hasher.update(b"present");
    write_len_bytes(hasher, installed.version.as_bytes());
    let directory = installed.directory(journal);
    let mut files = installed.files;
    files.sort_by(|left, right| left.file.cmp(&right.file));
    hasher.update((files.len() as u64).to_be_bytes());
    for file in files {
        let path = directory.join(&file.file);
        let pack = crate::pack::Pack::open_verified(&path, &file.sha256, file.size_bytes)
            .map_err(io::Error::other)?;
        if !matches!(
            pack.kind(),
            crate::pack::PackKind::Venues | crate::pack::PackKind::Containment
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid installed accuracy pack kind",
            ));
        }
        write_len_bytes(hasher, pack.version().as_bytes());
        write_len_bytes(hasher, file.file.as_bytes());
        write_len_bytes(hasher, file.sha256.as_bytes());
        hasher.update(file.size_bytes.to_be_bytes());
    }
    Ok(())
}

fn write_len_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accuracy::{INSTALLED_RECORD, Installed, InstalledFile, accuracy_root};
    use crate::places::{Place, PlaceStore, store_path};

    fn write_location(journal: &Path, relative: &str, bytes: &[u8]) {
        let path = journal.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn bundled_naming_identity_changes_pending_freshness_even_on_an_empty_day() {
        let journal = tempfile::tempdir().unwrap();
        let current = daily_location_fingerprint(journal.path(), "20261001").unwrap();
        let earlier =
            fingerprint_with_area_identity(journal.path(), "20261001", "earlier-bundled-area-tier")
                .unwrap();
        assert_ne!(current, earlier);
        assert_eq!(
            current,
            fingerprint_with_area_identity(
                journal.path(),
                "20261001",
                crate::asset::AREA_TIER_SHA256
            )
            .unwrap()
        );
    }

    #[test]
    fn fingerprint_is_hex_of_bytes_names_and_accuracy_and_errors_are_not_hex() {
        let journal = tempfile::tempdir().unwrap();
        let empty = daily_location_fingerprint(journal.path(), "20261001").unwrap();
        assert_eq!(empty.len(), 64);
        assert!(empty.chars().all(|character| character.is_ascii_hexdigit()));

        write_location(
            journal.path(),
            "chronicle/20261001/phone/120000_1/location.jsonl",
            b"one\n",
        );
        write_location(
            journal.path(),
            "chronicle/20261001/phone/130000_1/location.jsonl",
            b"two\n",
        );
        fs::write(
            journal.path().join("chronicle/20261001/location.md"),
            b"derived document",
        )
        .unwrap();
        let with_segments = daily_location_fingerprint(journal.path(), "20261001").unwrap();
        assert_ne!(with_segments, empty);
        let swapped = {
            write_location(
                journal.path(),
                "chronicle/20261001/phone/120000_1/location.jsonl",
                b"two\n",
            );
            write_location(
                journal.path(),
                "chronicle/20261001/phone/130000_1/location.jsonl",
                b"one\n",
            );
            daily_location_fingerprint(journal.path(), "20261001").unwrap()
        };
        assert_ne!(swapped, with_segments);
        write_location(
            journal.path(),
            "chronicle/20261001/phone/120000_1/location.jsonl",
            b"one\n",
        );
        write_location(
            journal.path(),
            "chronicle/20261001/phone/130000_1/location.jsonl",
            b"two\n",
        );
        assert_eq!(
            daily_location_fingerprint(journal.path(), "20261001").unwrap(),
            with_segments
        );

        let store = PlaceStore {
            version: 1,
            places: vec![Place {
                id: "place-1".to_owned(),
                latitude: 1.0,
                longitude: 2.0,
                name: Some("gym".to_owned()),
                alias: None,
                hidden: false,
            }],
        };
        store.write(journal.path()).unwrap();
        let with_names = daily_location_fingerprint(journal.path(), "20261001").unwrap();
        assert_ne!(with_names, with_segments);

        let pack_bytes =
            fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy/venues.slpk"))
                .unwrap();
        let installed = Installed {
            version: "v1".to_owned(),
            folder: String::new(),
            cache_generation: None,
            files: vec![InstalledFile {
                file: "venues.slpk".to_owned(),
                sha256: format!("{:x}", Sha256::digest(&pack_bytes)),
                size_bytes: pack_bytes.len() as u64,
            }],
        };
        let root = accuracy_root(journal.path());
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(INSTALLED_RECORD),
            serde_json::to_vec(&installed).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(installed.directory(journal.path())).unwrap();
        let pack_path = installed.directory(journal.path()).join("venues.slpk");
        fs::write(&pack_path, &pack_bytes).unwrap();
        let with_accuracy = daily_location_fingerprint(journal.path(), "20261001").unwrap();
        assert_ne!(with_accuracy, with_names);
        assert_eq!(with_accuracy.len(), 64);
        fs::remove_file(&pack_path).unwrap();
        assert!(daily_location_fingerprint(journal.path(), "20261001").is_err());
        fs::write(&pack_path, &pack_bytes[..12]).unwrap();
        assert!(daily_location_fingerprint(journal.path(), "20261001").is_err());
        fs::write(&pack_path, &pack_bytes).unwrap();
        assert_eq!(
            daily_location_fingerprint(journal.path(), "20261001").unwrap(),
            with_accuracy
        );

        fs::write(store_path(journal.path()), b"not-json").unwrap();
        assert!(daily_location_fingerprint(journal.path(), "20261001").is_err());
        fs::write(store_path(journal.path()), br#"{"version":9,"places":[]}"#).unwrap();
        assert!(daily_location_fingerprint(journal.path(), "20261001").is_err());
        fs::remove_file(store_path(journal.path())).unwrap();
        fs::write(root.join(INSTALLED_RECORD), b"{").unwrap();
        assert!(daily_location_fingerprint(journal.path(), "20261001").is_err());
        fs::remove_file(root.join(INSTALLED_RECORD)).unwrap();

        let segment = journal.path().join("chronicle/20261002/phone/120000_1");
        fs::create_dir_all(&segment).unwrap();
        let location = segment.join("location.jsonl");
        fs::write(&location, b"bytes").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&location).unwrap().permissions();
            permissions.set_mode(0o000);
            fs::set_permissions(&location, permissions).unwrap();
            let error = daily_location_fingerprint(journal.path(), "20261002");
            let mut restore = fs::metadata(&location).unwrap().permissions();
            restore.set_mode(0o644);
            fs::set_permissions(&location, restore).unwrap();
            assert!(error.is_err());
        }
        let day_file = journal.path().join("chronicle/20261003");
        fs::create_dir_all(day_file.parent().unwrap()).unwrap();
        fs::write(&day_file, b"not a directory").unwrap();
        assert!(daily_location_fingerprint(journal.path(), "20261003").is_err());
    }
}
