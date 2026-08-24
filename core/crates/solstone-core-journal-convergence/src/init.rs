// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::time::Duration;

use solstone_core_journal_io::{
    JournalRoot, acquire_existing_parent_lock_bound, create_directory_bound, read_bytes_bound,
    write_bytes_exclusive_bound,
};

use crate::digest::digest_value;
use crate::error::{ConvergenceError, DurableRole, Refusal, map_root_error, random_hex};
use crate::layout::{
    ALLOCATOR, BARRIERS, CONVERGENCE, DAYS, DECISIONS, GRANTS, HEALTH, LINKS, MEMBERS, OWNERS,
    RECONCILIATIONS, RECORDS, REGISTRY, REGISTRY_LOCK, REVOCATIONS, ROOT_WITNESS, TOMBSTONES,
    TOPOLOGY_LOCK,
};
use crate::schema::{
    Allocator, RootWitness, SCHEMA_VERSION, now_rfc3339, read_json, write_json_exclusive,
};
use crate::walk::{open_dir, open_file};

pub(crate) struct StoreDirs {
    pub convergence: OwnedFd,
    pub days: OwnedFd,
    pub records: OwnedFd,
    pub registry: OwnedFd,
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
    let Some(registry) = open_dir(&convergence, REGISTRY)? else {
        return Ok(None);
    };
    Ok(Some(StoreDirs {
        convergence,
        days,
        records,
        registry,
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
/// Completes a witness-without-allocator partial tree (L9).
pub fn initialize(root: &JournalRoot) -> Result<(), ConvergenceError> {
    root.revalidate().map_err(map_root_error)?;
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
    let witness = read_json::<RootWitness>(
        &convergence,
        OsStr::new(ROOT_WITNESS),
        DurableRole::RootWitness,
    )?;
    let allocator =
        read_json::<Allocator>(&convergence, OsStr::new(ALLOCATOR), DurableRole::Allocator)?;
    match (witness, allocator) {
        (Some(witness), Some(allocator)) => {
            let registry_was_complete = registry_tree_complete(&convergence)?;
            ensure_registry_tree(&convergence)?;
            let root_id = digest_value(&witness)?.as_hex().to_owned();
            drop(topology);
            if witness.journal_id != allocator.journal_id || allocator.root_id != root_id {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Allocator,
                });
            }
            if !registry_was_complete {
                return Ok(());
            }
            Err(ConvergenceError::Refused(Refusal::AlreadyInitialized))
        }
        (Some(witness), None) => {
            write_allocator(&convergence, &witness)?;
            ensure_registry_tree(&convergence)?;
            drop(topology);
            Ok(())
        }
        (None, None) => {
            let witness = RootWitness {
                schema_version: SCHEMA_VERSION,
                journal_id: random_hex()?,
                auxiliary_time: now_rfc3339(),
            };
            write_json_exclusive(
                &convergence,
                OsStr::new(ROOT_WITNESS),
                &witness,
                DurableRole::RootWitness,
            )?;
            write_allocator(&convergence, &witness)?;
            ensure_registry_tree(&convergence)?;
            drop(topology);
            Ok(())
        }
        (None, Some(_)) => {
            drop(topology);
            Err(ConvergenceError::Unknown {
                role: DurableRole::RootWitness,
            })
        }
    }
}

fn registry_tree_complete(convergence: &OwnedFd) -> Result<bool, ConvergenceError> {
    Ok(open_dir(convergence, REGISTRY)?.is_some()
        && open_file(convergence, REGISTRY_LOCK)?.is_some())
}

fn ensure_registry_tree(convergence: &OwnedFd) -> Result<(), ConvergenceError> {
    create_directory_bound(convergence, OsStr::new(REGISTRY), 0o700).map_err(map_path)?;
    let registry = open_dir(convergence, REGISTRY)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })?;
    create_directory_bound(&registry, OsStr::new(OWNERS), 0o700).map_err(map_path)?;
    create_directory_bound(&registry, OsStr::new(LINKS), 0o700).map_err(map_path)?;
    create_directory_bound(&registry, OsStr::new(DECISIONS), 0o700).map_err(map_path)?;
    create_directory_bound(&registry, OsStr::new(GRANTS), 0o700).map_err(map_path)?;
    let grants = open_dir(&registry, GRANTS)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })?;
    create_directory_bound(&grants, OsStr::new(MEMBERS), 0o700).map_err(map_path)?;
    create_directory_bound(&grants, OsStr::new(BARRIERS), 0o700).map_err(map_path)?;
    create_directory_bound(&grants, OsStr::new(REVOCATIONS), 0o700).map_err(map_path)?;
    create_directory_bound(&grants, OsStr::new(TOMBSTONES), 0o700).map_err(map_path)?;
    create_directory_bound(&grants, OsStr::new(RECONCILIATIONS), 0o700).map_err(map_path)?;
    create_registry_lock_file(convergence)?;
    Ok(())
}

fn create_registry_lock_file(convergence: &OwnedFd) -> Result<(), ConvergenceError> {
    match write_bytes_exclusive_bound(convergence, OsStr::new(REGISTRY_LOCK), b"", 0o600) {
        Ok(()) => Ok(()),
        Err(solstone_core_journal_io::AtomicWriteError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            Ok(())
        }
        Err(solstone_core_journal_io::AtomicWriteError::Io { source, .. }) => {
            Err(ConvergenceError::Io {
                operation: "create registry lock file",
                role: DurableRole::RegistryLock,
                source,
            })
        }
    }
}

fn write_allocator(convergence: &OwnedFd, witness: &RootWitness) -> Result<(), ConvergenceError> {
    let allocator = Allocator {
        schema_version: SCHEMA_VERSION,
        journal_id: witness.journal_id.clone(),
        root_id: digest_value(witness)?.as_hex().to_owned(),
        next_serial: 1,
        exhausted: false,
    };
    write_json_exclusive(
        convergence,
        OsStr::new(ALLOCATOR),
        &allocator,
        DurableRole::Allocator,
    )?;
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
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
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
        assert!(journal.join("health/convergence/registry.lock").exists());
        assert!(journal.join("health/convergence/registry").is_dir());
        assert!(journal.join("health/convergence/registry/owners").is_dir());
        assert!(journal.join("health/convergence/registry/links").is_dir());
        assert!(
            journal
                .join("health/convergence/registry/decisions")
                .is_dir()
        );
        assert!(journal.join("health/convergence/registry/grants").is_dir());
        assert!(
            journal
                .join("health/convergence/registry/grants/members")
                .is_dir()
        );
        assert!(
            journal
                .join("health/convergence/registry/grants/barriers")
                .is_dir()
        );
        assert!(
            journal
                .join("health/convergence/registry/grants/revocations")
                .is_dir()
        );
        assert!(
            journal
                .join("health/convergence/registry/grants/tombstones")
                .is_dir()
        );
        assert!(
            !journal
                .join("health/convergence/registry/secret.json")
                .exists()
        );
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

    fn second_store_allocates_promptly(temporary: &TempDir) {
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = crate::store::ConvergenceStore::open(root_b).unwrap();
        let day = crate::layout::DayKey::parse("20260824").unwrap();
        let started = std::time::Instant::now();
        let _locks = store_b.acquire_days(&[day]).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn initialize_releases_topology_before_return() {
        let (temporary, _store) = initialized_store();
        second_store_allocates_promptly(&temporary);
    }

    #[test]
    fn initialize_does_not_leave_topology_held() {
        let (temporary, store) = initialized_store();
        let day = crate::layout::DayKey::parse("20260823").unwrap();
        let _locks = store.acquire_days(&[day]).unwrap();
        second_store_allocates_promptly(&temporary);
    }

    #[test]
    fn racing_initialize_yields_one_root() {
        let temporary = TempDir::new("race-init");
        let (journal, root_a) = open_root(&temporary);
        let root_b = solstone_core_journal_io::JournalRoot::open(&journal).unwrap();
        let first = std::thread::spawn(move || initialize(&root_a));
        let second = initialize(&root_b);
        let first = first.join().expect("thread");
        match (&first, &second) {
            (Ok(()), Err(error)) | (Err(error), Ok(())) => {
                assert!(matches!(
                    error,
                    ConvergenceError::Refused(Refusal::AlreadyInitialized)
                ));
            }
            other => panic!("expected one Ok and one AlreadyInitialized, got {other:?}"),
        }
        assert!(check_initialized(&root_b).unwrap());
        let parse = |name: &str| {
            let raw = std::fs::read(journal.join("health/convergence").join(name)).unwrap();
            serde_json::from_slice::<serde_json::Value>(raw.strip_suffix(b"\n").unwrap_or(&raw))
                .unwrap()
        };
        let witness = parse("root.wit.json");
        let allocator = parse("allocator.json");
        assert_eq!(witness["journal_id"], allocator["journal_id"]);
    }

    #[test]
    fn initialize_completes_partial_witness_tree() {
        let temporary = TempDir::new("partial-init");
        let (journal, root) = open_root(&temporary);
        initialize(&root).unwrap();
        let witness_bytes =
            std::fs::read(journal.join("health/convergence/root.wit.json")).unwrap();
        std::fs::remove_file(journal.join("health/convergence/allocator.json")).unwrap();
        assert!(!check_initialized(&root).unwrap());
        initialize(&root).unwrap();
        assert!(check_initialized(&root).unwrap());
        let witness: RootWitness =
            serde_json::from_slice(witness_bytes.strip_suffix(b"\n").unwrap_or(&witness_bytes))
                .unwrap();
        let allocator_bytes =
            std::fs::read(journal.join("health/convergence/allocator.json")).unwrap();
        let allocator: Allocator = serde_json::from_slice(
            allocator_bytes
                .strip_suffix(b"\n")
                .unwrap_or(&allocator_bytes),
        )
        .unwrap();
        assert_eq!(allocator.journal_id, witness.journal_id);
        crate::store::ConvergenceStore::open(
            solstone_core_journal_io::JournalRoot::open(&journal).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn initialize_completes_missing_registry() {
        let temporary = TempDir::new("partial-registry");
        let (journal, root) = open_root(&temporary);
        initialize(&root).unwrap();
        std::fs::remove_dir_all(journal.join("health/convergence/registry")).unwrap();
        std::fs::remove_file(journal.join("health/convergence/registry.lock")).unwrap();
        assert!(check_initialized(&root).unwrap());
        let opened = crate::store::ConvergenceStore::open(
            solstone_core_journal_io::JournalRoot::open(&journal).unwrap(),
        );
        assert!(opened.is_err());
        initialize(&root).unwrap();
        assert!(journal.join("health/convergence/registry.lock").exists());
        assert!(journal.join("health/convergence/registry").is_dir());
        assert!(
            !journal
                .join("health/convergence/registry/secret.json")
                .exists()
        );
        crate::store::ConvergenceStore::open(
            solstone_core_journal_io::JournalRoot::open(&journal).unwrap(),
        )
        .unwrap();
        let error = initialize(&root).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::AlreadyInitialized)
        ));
    }

    fn fs_entries(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}
