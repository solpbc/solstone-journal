// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strava tiles filed under a new facet that search hasn't been told about.
//!
//! Search files a tile under a facet when the tile is published, and a tile's
//! facet file changing is not something search notices on its own. So before a
//! Strava import writes a tile's new facet, it lists the tile in
//! `imports/.strava-refile.json`, and it takes the tile off only once the import
//! that published it is complete. A run stopped between the two leaves the
//! tile listed, and the next Strava import publishes it again. Publishing a
//! tile twice is harmless. Every read and write here is made under
//! `imports/.strava`, the lock Strava's import holds.
//!
//! A list that can't be parsed never stops an import: a change to the list
//! sets it aside beside itself, logs it, and starts a new one. Tiles it held
//! are not published again on its account.

use std::collections::BTreeSet;
use std::path::Path;

use solstone_core_journal_io::durability::set_aside;
use solstone_core_journal_io::{AtomicWriteOptions, FileLock, atomic_replace};

/// The list, relative to the journal.
pub const STRAVA_REFILE_FILE: &str = "imports/.strava-refile.json";

const FILE_MODE: u32 = 0o600;

/// A tile, by its day and segment key.
pub type RefiledTile = (String, String);

/// The tiles listed. A missing list has none; one that can't be parsed is an
/// error, which a change to the list sets aside.
pub fn read_strava_refiles(
    journal: &Path,
    _held: &FileLock,
) -> Result<BTreeSet<RefiledTile>, String> {
    match listed_bytes(journal)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        None => Ok(BTreeSet::new()),
    }
}

fn listed_bytes(journal: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(journal.join(STRAVA_REFILE_FILE)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// The tiles listed, for a change to the list. A list that can't be parsed
/// is set aside beside itself and logged, and counts as none.
fn listed_for_change(journal: &Path) -> Result<BTreeSet<RefiledTile>, String> {
    let Some(bytes) = listed_bytes(journal)? else {
        return Ok(BTreeSet::new());
    };
    match serde_json::from_slice(&bytes) {
        Ok(listed) => Ok(listed),
        Err(error) => {
            let aside = set_aside(&journal.join(STRAVA_REFILE_FILE))
                .map_err(|aside| format!("{STRAVA_REFILE_FILE} unreadable: {aside}"))?;
            log::warn!(
                "{STRAVA_REFILE_FILE} unreadable, set aside as {}: {error}",
                aside.display()
            );
            Ok(BTreeSet::new())
        }
    }
}

/// List `tiles`, beside any already listed.
pub fn add_strava_refiles(
    journal: &Path,
    tiles: &[RefiledTile],
    _held: &FileLock,
) -> Result<(), String> {
    let mut listed = listed_for_change(journal)?;
    let before = listed.len();
    listed.extend(tiles.iter().cloned());
    if listed.len() == before {
        return Ok(());
    }
    save(journal, &listed)
}

/// Take `tiles` off the list, once they have been published.
pub fn remove_strava_refiles(
    journal: &Path,
    tiles: &BTreeSet<RefiledTile>,
    _held: &FileLock,
) -> Result<(), String> {
    let mut listed = listed_for_change(journal)?;
    let before = listed.len();
    listed.retain(|tile| !tiles.contains(tile));
    if listed.len() == before {
        return Ok(());
    }
    if listed.is_empty() {
        return match std::fs::remove_file(journal.join(STRAVA_REFILE_FILE)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        };
    }
    save(journal, &listed)
}

fn save(journal: &Path, listed: &BTreeSet<RefiledTile>) -> Result<(), String> {
    let bytes = serde_json::to_vec(listed).map_err(|error| error.to_string())?;
    atomic_replace(
        journal.join(STRAVA_REFILE_FILE),
        &bytes,
        AtomicWriteOptions {
            mode: Some(FILE_MODE),
        },
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strava_names::hold_strava_lock;

    fn tile(day: &str, segment: &str) -> RefiledTile {
        (day.to_owned(), segment.to_owned())
    }

    #[test]
    fn listed_tiles_stay_until_taken_off() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("imports")).unwrap();
        let held = hold_strava_lock(journal.path()).unwrap();
        assert!(
            read_strava_refiles(journal.path(), &held)
                .unwrap()
                .is_empty()
        );
        add_strava_refiles(
            journal.path(),
            &[
                tile("20261004", "070000_300"),
                tile("20261004", "070500_300"),
            ],
            &held,
        )
        .unwrap();
        add_strava_refiles(journal.path(), &[tile("20261005", "090000_300")], &held).unwrap();
        remove_strava_refiles(
            journal.path(),
            &BTreeSet::from([tile("20261004", "070000_300")]),
            &held,
        )
        .unwrap();
        assert_eq!(
            read_strava_refiles(journal.path(), &held).unwrap(),
            BTreeSet::from([
                tile("20261004", "070500_300"),
                tile("20261005", "090000_300")
            ])
        );
        let all = read_strava_refiles(journal.path(), &held).unwrap();
        remove_strava_refiles(journal.path(), &all, &held).unwrap();
        assert!(!journal.path().join(STRAVA_REFILE_FILE).exists());
    }

    #[test]
    fn a_list_that_cant_be_parsed_is_set_aside_and_a_new_one_started() {
        let journal = tempfile::tempdir().unwrap();
        let imports = journal.path().join("imports");
        std::fs::create_dir_all(&imports).unwrap();
        let held = hold_strava_lock(journal.path()).unwrap();
        std::fs::write(journal.path().join(STRAVA_REFILE_FILE), b"{not a list").unwrap();
        assert!(read_strava_refiles(journal.path(), &held).is_err());

        add_strava_refiles(journal.path(), &[tile("20261004", "070000_300")], &held).unwrap();

        assert_eq!(
            read_strava_refiles(journal.path(), &held).unwrap(),
            BTreeSet::from([tile("20261004", "070000_300")])
        );
        let aside = std::fs::read_dir(&imports)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".strava-refile.wedged-"))
            })
            .collect::<Vec<_>>();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert_eq!(std::fs::read(&aside[0]).unwrap(), b"{not a list");
    }
}
