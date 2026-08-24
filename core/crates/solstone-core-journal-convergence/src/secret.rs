// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Create-only journal secret. Never written by `initialize`.

use std::ffi::OsStr;

use crate::error::{ConvergenceError, DurableRole, random_hex};
use crate::layout::SECRET;
use crate::registry::RegistrySection;
use crate::schema::{
    JournalSecret, ROLE_JOURNAL_SECRET, SCHEMA_VERSION, now_rfc3339, read_json,
    write_json_exclusive,
};

/// Write: exclusive create under a live registry section, then exact re-read.
// Wired by hook A in the next commit (prepared-owner issuance).
#[allow(dead_code)]
pub(crate) fn create_journal_secret(
    section: &RegistrySection<'_>,
    journal_id: &str,
    root_id: &str,
) -> Result<JournalSecret, ConvergenceError> {
    let secret = JournalSecret {
        role: ROLE_JOURNAL_SECRET.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: journal_id.to_owned(),
        root_id: root_id.to_owned(),
        key_hex: random_hex()?,
        auxiliary_time: now_rfc3339(),
    };
    match write_json_exclusive(
        section.registry(),
        OsStr::new(SECRET),
        &secret,
        DurableRole::JournalSecret,
    ) {
        Ok(_) => {
            let loaded =
                load_journal_secret(section.registry())?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                })?;
            if loaded != secret {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                });
            }
            Ok(loaded)
        }
        Err(ConvergenceError::PreservedPrior { .. }) => load_journal_secret(section.registry())?
            .ok_or(ConvergenceError::Unknown {
                role: DurableRole::JournalSecret,
            }),
        Err(error) => Err(error),
    }
}

/// Read: missing is `None`. Never creates.
pub(crate) fn load_journal_secret(
    registry: &std::os::fd::OwnedFd,
) -> Result<Option<JournalSecret>, ConvergenceError> {
    read_json(registry, OsStr::new(SECRET), DurableRole::JournalSecret)
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::init::open_store_dirs;
    use crate::layout::SECRET;
    use crate::registry::{enter_registry, enter_registry_with_timeout};
    use crate::test_support::initialized_store;
    use std::time::Duration;

    #[test]
    fn racing_create_yields_one_identical_secret() {
        let (temporary, store_a) = initialized_store();
        let journal_id = store_a.journal_id().to_owned();
        let root_id = store_a.root_id().to_owned();
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = crate::store::ConvergenceStore::open(root_b).unwrap();
        let dirs_a = open_store_dirs(store_a.root()).unwrap().unwrap();
        let dirs_b = open_store_dirs(store_b.root()).unwrap().unwrap();
        let journal_b = journal_id.clone();
        let root_b_id = root_id.clone();
        let first = std::thread::spawn(move || {
            let section = enter_registry(&dirs_a).unwrap();
            create_journal_secret(&section, &journal_b, &root_b_id)
        });
        let section_b = enter_registry(&dirs_b).unwrap();
        let second = create_journal_secret(&section_b, &journal_id, &root_id).unwrap();
        drop(section_b);
        let first = first.join().expect("thread").unwrap();
        assert_eq!(first.key_hex, second.key_hex);
        assert_eq!(first.journal_id, second.journal_id);
        assert_eq!(first.root_id, second.root_id);
        let path = temporary
            .journal_path()
            .join("health/convergence/registry")
            .join(SECRET);
        let bytes = std::fs::read(&path).unwrap();
        let on_disk: JournalSecret =
            serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap();
        assert_eq!(on_disk.key_hex, first.key_hex);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_name() == SECRET)
                .count(),
            1
        );
    }

    #[test]
    fn load_does_not_create() {
        let (temporary, store) = initialized_store();
        let dirs = open_store_dirs(store.root()).unwrap().unwrap();
        let section = enter_registry_with_timeout(&dirs, Duration::from_secs(2)).unwrap();
        assert!(load_journal_secret(section.registry()).unwrap().is_none());
        drop(section);
        assert!(
            !temporary
                .journal_path()
                .join("health/convergence/registry/secret.json")
                .exists()
        );
    }
}
