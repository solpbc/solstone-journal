// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::time::Duration;

use solstone_core_journal_io::{
    JournalRoot, acquire_existing_parent_lock_bound, create_directory_bound, read_bytes_bound,
};

use crate::digest::digest_value;
use crate::error::{ConvergenceError, DurableRole, Refusal, map_root_error, random_hex};
use crate::layout::{ALLOCATOR, CONVERGENCE, DAYS, HEALTH, RECORDS, ROOT_WITNESS, TOPOLOGY_LOCK};
use crate::schema::{
    Allocator, RootWitness, SCHEMA_VERSION, now_rfc3339, read_json, write_json_exclusive,
};
use crate::walk::open_dir;

pub(crate) struct StoreDirs {
    pub convergence: OwnedFd,
    pub days: OwnedFd,
    pub records: OwnedFd,
}

pub(crate) fn open_store_dirs(root: &JournalRoot) -> Result<Option<StoreDirs>, ConvergenceError> {
    let Some(health) = open_dir(root, HEALTH)? else {
        return Ok(None);
    };
    let Some(convergence) = open_dir(&health, CONVERGENCE)? else {
        return Ok(None);
    };
    let Some(days) = open_dir(&convergence, DAYS)? else {
        return Ok(None);
    };
    let Some(records) = open_dir(&convergence, RECORDS)? else {
        return Ok(None);
    };
    Ok(Some(StoreDirs {
        convergence,
        days,
        records,
    }))
}

/// Read: true iff genesis files exist. Never creates anything on miss.
pub fn check_initialized(root: &JournalRoot) -> Result<bool, ConvergenceError> {
    let Some(health) = open_dir(root, HEALTH)? else {
        return Ok(false);
    };
    let Some(convergence) = open_dir(&health, CONVERGENCE)? else {
        return Ok(false);
    };
    if crate::walk::open_file(&convergence, ROOT_WITNESS)?.is_none() {
        return Ok(false);
    }
    if crate::walk::open_file(&convergence, ALLOCATOR)?.is_none() {
        return Ok(false);
    }
    let root_witness = match read_bytes_bound(&convergence, OsStr::new(ROOT_WITNESS)) {
        Ok(bytes) => bytes,
        Err(solstone_core_journal_io::ReadError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(false);
        }
        Err(solstone_core_journal_io::ReadError::Io { source, .. }) => {
            return Err(ConvergenceError::Io {
                operation: "read root witness",
                role: DurableRole::RootWitness,
                source,
            });
        }
        Err(solstone_core_journal_io::ReadError::Malformed(_)) => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RootWitness,
            });
        }
    };
    let allocator = match read_bytes_bound(&convergence, OsStr::new(ALLOCATOR)) {
        Ok(bytes) => bytes,
        Err(solstone_core_journal_io::ReadError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(false);
        }
        Err(solstone_core_journal_io::ReadError::Io { source, .. }) => {
            return Err(ConvergenceError::Io {
                operation: "read allocator",
                role: DurableRole::Allocator,
                source,
            });
        }
        Err(solstone_core_journal_io::ReadError::Malformed(_)) => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Allocator,
            });
        }
    };
    Ok(root_witness.is_some() && allocator.is_some())
}

/// Write: create never-replaced parents, topology.lock, root witness, allocator.
pub fn initialize(root: &JournalRoot) -> Result<(), ConvergenceError> {
    root.revalidate().map_err(map_root_error)?;
    if check_initialized(root)? {
        return Err(ConvergenceError::Refused(Refusal::AlreadyInitialized));
    }
    create_directory_bound(root, OsStr::new(HEALTH), 0o700).map_err(map_path)?;
    let health = open_dir(root, HEALTH)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })?;
    create_directory_bound(&health, OsStr::new(CONVERGENCE), 0o700).map_err(map_path)?;
    let convergence = open_dir(&health, CONVERGENCE)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })?;
    create_directory_bound(&convergence, OsStr::new(DAYS), 0o700).map_err(map_path)?;
    create_directory_bound(&convergence, OsStr::new(RECORDS), 0o700).map_err(map_path)?;
    let topology = acquire_existing_parent_lock_bound(
        &convergence,
        OsStr::new(TOPOLOGY_LOCK),
        Duration::from_secs(2),
        Duration::from_millis(20),
    )
    .map_err(|error| ConvergenceError::Io {
        operation: "acquire topology lock",
        role: DurableRole::TopologyLock,
        source: std::io::Error::other(error.to_string()),
    })?;
    if check_initialized(root)? {
        drop(topology);
        return Err(ConvergenceError::Refused(Refusal::AlreadyInitialized));
    }
    let witness = RootWitness {
        schema_version: SCHEMA_VERSION,
        journal_id: random_hex()?,
        auxiliary_time: now_rfc3339(),
    };
    let root_id = digest_value(&witness)?.as_hex().to_owned();
    write_json_exclusive(
        &convergence,
        OsStr::new(ROOT_WITNESS),
        &witness,
        DurableRole::RootWitness,
    )?;
    let allocator = Allocator {
        schema_version: SCHEMA_VERSION,
        journal_id: witness.journal_id,
        root_id,
        next_serial: 1,
        exhausted: false,
    };
    write_json_exclusive(
        &convergence,
        OsStr::new(ALLOCATOR),
        &allocator,
        DurableRole::Allocator,
    )?;
    drop(topology);
    Ok(())
}

pub(crate) fn load_allocator(dirs: &StoreDirs) -> Result<Allocator, ConvergenceError> {
    read_json(
        &dirs.convergence,
        OsStr::new(ALLOCATOR),
        DurableRole::Allocator,
    )?
    .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))
}

pub(crate) fn load_root_witness(dirs: &StoreDirs) -> Result<RootWitness, ConvergenceError> {
    read_json(
        &dirs.convergence,
        OsStr::new(ROOT_WITNESS),
        DurableRole::RootWitness,
    )?
    .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))
}

fn map_path(error: solstone_core_journal_io::PathError) -> ConvergenceError {
    match error {
        solstone_core_journal_io::PathError::Io { source, .. } => ConvergenceError::Io {
            operation: "create bound directory",
            role: DurableRole::Directory,
            source,
        },
        other => ConvergenceError::Io {
            operation: "create bound directory",
            role: DurableRole::Directory,
            source: std::io::Error::other(other.to_string()),
        },
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::test_support::{
        TempDir, assert_not_initialized_creates_nothing, initialized_store, open_root,
        snapshot_tree,
    };

    #[test]
    fn check_initialized_creates_nothing() {
        assert_not_initialized_creates_nothing();
        let (temporary, store) = initialized_store();
        let before = snapshot_tree(&temporary.journal_path());
        assert!(check_initialized(store.root()).unwrap());
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn initialized_is_read_and_creates_nothing() {
        let temporary = TempDir::new("init-read");
        let (_, root) = open_root(&temporary);
        assert!(!check_initialized(&root).unwrap());
        initialize(&root).unwrap();
        assert!(check_initialized(&root).unwrap());
        let error = initialize(&root).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::AlreadyInitialized)
        ));
    }

    #[test]
    fn initialize_is_the_write() {
        let temporary = TempDir::new("init-write");
        let (journal, root) = open_root(&temporary);
        initialize(&root).unwrap();
        assert!(journal.join("health/convergence/topology.lock").exists());
        assert!(journal.join("health/convergence/root.wit.json").exists());
        assert!(journal.join("health/convergence/allocator.json").exists());
        assert!(
            !journal
                .join("health/convergence/days/20260823.ever.wit.json")
                .exists()
        );
        assert!(
            !journal
                .join("health/convergence/days/20260823.head.json")
                .exists()
        );
    }

    #[test]
    fn root_witness_created_at_init() {
        let (temporary, _) = initialized_store();
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/root.wit.json")
                .exists()
        );
    }

    #[test]
    fn ever_not_created_at_init() {
        let (temporary, _) = initialized_store();
        let days = temporary.journal_path().join("health/convergence/days");
        let ever = fs_entries(&days);
        assert!(ever.iter().all(|name| !name.contains("ever")));
    }

    #[test]
    fn initialize_releases_topology_before_return() {
        let (_temporary, store) = initialized_store();
        let day = crate::layout::DayKey::parse("20260823").unwrap();
        store.acquire_days(&[day]).unwrap();
    }

    #[test]
    fn initialize_does_not_leave_topology_held() {
        let (_temporary, store) = initialized_store();
        let day = crate::layout::DayKey::parse("20260824").unwrap();
        store.acquire_days(&[day]).unwrap();
    }

    #[test]
    fn racing_initialize_yields_one_root() {
        let temporary = TempDir::new("race-init");
        let (journal, root_a) = open_root(&temporary);
        let root_b = solstone_core_journal_io::JournalRoot::open(&journal).unwrap();
        let first = std::thread::spawn(move || initialize(&root_a));
        let second = initialize(&root_b);
        let first = first.join().expect("thread");
        let ok = first.is_ok() as u8 + second.is_ok() as u8;
        assert_eq!(ok, 1);
        assert!(check_initialized(&root_b).unwrap());
    }

    fn fs_entries(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}
