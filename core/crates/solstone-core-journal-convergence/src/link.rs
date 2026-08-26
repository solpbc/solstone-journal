// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Immutable owner-intent links.
//!
//! A link is create-only and never overwritten. Links are keyed by exact
//! intent serial inside a per-operation directory, so a later-dirty successor
//! records its own link without rewriting the link of the transition it
//! follows.
//!
//! Admission absence therefore means the operation's link set is exactly
//! empty, which is the condition that holds on a fresh `begin` before any
//! allocation. The per-operation directory is created by linkage itself, so
//! its **existence** is the durable evidence that the operation has entered
//! linkage: it cannot appear before an intent exists, and a crash between
//! creating the directory and creating the link is exactly the
//! intent-without-link state that resumes by creating the link at the live
//! serial. Presence is deliberately a directory probe and not a listing, so
//! admission performs no scan.
//!
//! A link that exists but disagrees is **unknown**, not refused: it is
//! evidence that two different intents claim the same serial for one
//! operation, which no decision, resume, or grant may be built on. Nothing
//! overwrites it.

use std::ffi::OsStr;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::{create_directory_bound, sync_dir_bound};

use crate::error::{ConvergenceError, DurableRole};
use crate::layout::{LINKS, link_name, operation_links_dir};
use crate::owner::OwnerBinding;
use crate::registry::RegistrySection;
use crate::schema::{
    Intent, OwnerIntentLink, ROLE_OWNER_INTENT_LINK, SCHEMA_VERSION, read_json,
    write_json_exclusive,
};
use crate::walk::open_dir;

/// Read: whether the operation has entered linkage. Never creates.
pub(crate) fn operation_link_present(
    section: &RegistrySection<'_>,
    operation_id: &str,
) -> Result<bool, ConvergenceError> {
    Ok(!operation_link_serials(section, operation_id)?.is_empty())
}

/// Read: the operation's linked serials, ascending. Empty when the operation
/// has no link. Never creates.
///
/// This lists one small per-operation registry directory. It is bounded
/// resolver-file work, not a day-artifact scan, which is what a registry
/// section is forbidden to do.
pub(crate) fn operation_link_serials(
    section: &RegistrySection<'_>,
    operation_id: &str,
) -> Result<Vec<u64>, ConvergenceError> {
    let Some(links) = open_dir(section.registry(), LINKS)? else {
        return Ok(Vec::new());
    };
    let Some(directory) = open_dir(&links, &operation_links_dir(operation_id))? else {
        return Ok(Vec::new());
    };
    // Descriptor-relative listing of the already-bound directory itself.
    let mut listing = nix::dir::Dir::openat(
        &directory,
        ".",
        nix::fcntl::OFlag::O_RDONLY
            .union(nix::fcntl::OFlag::O_DIRECTORY)
            .union(nix::fcntl::OFlag::O_CLOEXEC),
        nix::sys::stat::Mode::empty(),
    )
    .map_err(|source| ConvergenceError::Io {
        operation: "list links directory",
        role: DurableRole::OwnerIntentLink,
        source: std::io::Error::from_raw_os_error(source as i32),
    })?;
    let mut serials = Vec::new();
    for entry in listing.iter() {
        let entry = entry.map_err(|source| ConvergenceError::Io {
            operation: "read links directory entry",
            role: DurableRole::OwnerIntentLink,
            source: std::io::Error::from_raw_os_error(source as i32),
        })?;
        let name = String::from_utf8_lossy(entry.file_name().to_bytes()).into_owned();
        if name == "." || name == ".." {
            continue;
        }
        let Some(stem) = name.strip_suffix(".json") else {
            // An unexpected name in the link directory is not interpretable as
            // linkage, and guessing past it would be worse than refusing.
            return Err(ConvergenceError::Unknown {
                role: DurableRole::OwnerIntentLink,
            });
        };
        let serial: u64 = stem.parse().map_err(|_| ConvergenceError::Unknown {
            role: DurableRole::OwnerIntentLink,
        })?;
        serials.push(serial);
    }
    serials.sort_unstable();
    Ok(serials)
}

/// Read: the operation's most recent link, or `None`. With later-dirty an
/// operation can hold several immutable links; the highest serial is the
/// transition its outbox belongs to. Never creates.
pub(crate) fn latest_link(
    section: &RegistrySection<'_>,
    operation_id: &str,
) -> Result<Option<OwnerIntentLink>, ConvergenceError> {
    let serials = operation_link_serials(section, operation_id)?;
    let Some(serial) = serials.last().copied() else {
        return Ok(None);
    };
    load_owner_intent_link(section, operation_id, serial)
}

/// Read: the operation's link at `serial`, or `None`. Never creates.
pub(crate) fn load_owner_intent_link(
    section: &RegistrySection<'_>,
    operation_id: &str,
    serial: u64,
) -> Result<Option<OwnerIntentLink>, ConvergenceError> {
    let Some(links) = open_dir(section.registry(), LINKS)? else {
        return Ok(None);
    };
    let Some(directory) = open_dir(&links, &operation_links_dir(operation_id))? else {
        return Ok(None);
    };
    read_json(&directory, &link_name(serial), DurableRole::OwnerIntentLink)
}

fn ensure_operation_links_dir(
    section: &RegistrySection<'_>,
    operation_id: &str,
) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(section.registry(), OsStr::new(LINKS), 0o700).map_err(map_dir)?;
    let links = open_dir(section.registry(), LINKS)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })?;
    let name = operation_links_dir(operation_id);
    create_directory_bound(&links, OsStr::new(&name), 0o700).map_err(map_dir)?;
    open_dir(&links, &name)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

fn map_dir(error: solstone_core_journal_io::PathError) -> ConvergenceError {
    ConvergenceError::Io {
        operation: "create links directory",
        role: DurableRole::Directory,
        source: std::io::Error::other(error.to_string()),
    }
}

/// Write: create-only owner-intent link, then file and parent sync, then an
/// exact re-read. Re-running on an exact existing link is idempotent and
/// resyncs; a disagreeing link is unknown and is never overwritten.
pub(crate) fn create_owner_intent_link(
    section: &RegistrySection<'_>,
    owner: &OwnerBinding,
    intent: &Intent,
) -> Result<OwnerIntentLink, ConvergenceError> {
    let link = OwnerIntentLink {
        role: ROLE_OWNER_INTENT_LINK.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: owner.journal_id().to_owned(),
        root_id: owner.root_id().to_owned(),
        operation_id: owner.operation_id().to_owned(),
        owner_binding_digest: owner.digest_hex().to_owned(),
        serial: intent.serial,
        intent_digest: intent.intent_digest.clone(),
        day_set: intent.day_set.clone(),
        day_set_subdigest: intent.day_set_subdigest.clone(),
        selector_digest: owner.selector_digest().to_owned(),
    };
    let directory = ensure_operation_links_dir(section, owner.operation_id())?;
    match write_json_exclusive(
        &directory,
        &link_name(intent.serial),
        &link,
        DurableRole::OwnerIntentLink,
    ) {
        Ok(_) => {}
        Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterOwnerIntentLink,
    ) {
        return Err(ConvergenceError::Io {
            operation: "inject after owner intent link",
            role: DurableRole::OwnerIntentLink,
            source: std::io::Error::other("injected"),
        });
    }
    sync_dir_bound(&directory).map_err(|source| ConvergenceError::Io {
        operation: "sync links directory",
        role: DurableRole::OwnerIntentLink,
        source,
    })?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterOwnerIntentLinkSync,
    ) {
        return Err(ConvergenceError::Io {
            operation: "inject after owner intent link sync",
            role: DurableRole::OwnerIntentLink,
            source: std::io::Error::other("injected"),
        });
    }
    let durable = load_owner_intent_link(section, owner.operation_id(), intent.serial)?.ok_or(
        ConvergenceError::Unknown {
            role: DurableRole::OwnerIntentLink,
        },
    )?;
    if durable != link {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::OwnerIntentLink,
        });
    }
    Ok(durable)
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::error::Refusal;
    use crate::preflight::{Admitted, Preflight, preflight};
    use crate::selector::{GrantRequestSelector, OperationId, TransactionClass};
    use crate::test_support::{
        PublishFault, TempDir, admit_days, admit_proof, continue_ok, fail_after, prepared_owner,
        snapshot_tree,
    };
    use std::path::PathBuf;

    fn links_root(temporary: &TempDir) -> PathBuf {
        temporary
            .journal_path()
            .join("health/convergence/registry/links")
    }

    fn link_path(temporary: &TempDir, operation: &str, serial: u64) -> PathBuf {
        links_root(temporary)
            .join(operation)
            .join(format!("{serial}.json"))
    }

    fn read_link(temporary: &TempDir, operation: &str, serial: u64) -> OwnerIntentLink {
        let bytes = std::fs::read(link_path(temporary, operation, serial)).unwrap();
        serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap()
    }

    fn write_link(temporary: &TempDir, operation: &str, serial: u64, link: &OwnerIntentLink) {
        let mut bytes = crate::digest::canonical_json_bytes(link).unwrap();
        bytes.push(b'\n');
        std::fs::write(link_path(temporary, operation, serial), bytes).unwrap();
    }

    /// Re-open the same journal so a later `Admitted` can retry the same
    /// external operation from a fresh process-equivalent state.
    fn reopen(temporary: &TempDir, days: &[&str]) -> Admitted {
        let root = solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        match preflight(days.iter().copied()).unwrap() {
            Preflight::Ready(set) => set.admit(root).unwrap(),
            Preflight::Empty => panic!("days"),
        }
    }

    fn prepare_for(admitted: &Admitted, operation: &OperationId) -> crate::owner::OwnerBinding {
        let selector = GrantRequestSelector::empty(admitted.days()).unwrap();
        crate::owner::OwnerBinding::prepare(
            admitted,
            operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap()
    }

    #[test]
    fn continue_with_creates_the_exact_link() {
        let (temporary, admitted) = admit_days("link-create", &["20260823"]);
        let held = continue_ok(&admitted);
        let operation = held.owner().operation_id().to_owned();
        let serial = held.serial.unwrap();
        let link = read_link(&temporary, &operation, serial);
        assert_eq!(link.role, ROLE_OWNER_INTENT_LINK);
        assert_eq!(link.schema_version, SCHEMA_VERSION);
        assert_eq!(link.operation_id, operation);
        assert_eq!(link.serial, serial);
        assert_eq!(link.owner_binding_digest, held.owner().digest().as_hex());
        assert_eq!(link.day_set, vec!["20260823".to_owned()]);
        drop(held);
    }

    #[test]
    fn exact_link_is_idempotent_and_resyncs() {
        let (temporary, admitted) = admit_days("link-idem", &["20260823"]);
        let held = continue_ok(&admitted);
        let operation = held.owner().operation_id().to_owned();
        let serial = held.serial.unwrap();
        let intent = crate::intent::read_intent(
            &crate::init::open_store_dirs(admitted.store().root())
                .unwrap()
                .unwrap(),
            serial,
        )
        .unwrap()
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let section = crate::registry::enter_registry(&dirs).unwrap();
        let again = create_owner_intent_link(&section, held.owner(), &intent).unwrap();
        drop(section);
        assert_eq!(again.serial, serial);
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert_eq!(operation, again.operation_id);
        drop(held);
    }

    #[test]
    fn conflicting_link_is_unknown_and_never_overwritten() {
        let (temporary, admitted) = admit_days("link-conflict", &["20260823"]);
        let held = continue_ok(&admitted);
        let operation = held.owner().operation_id().to_owned();
        let serial = held.serial.unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let intent = crate::intent::read_intent(&dirs, serial).unwrap().unwrap();
        let mut planted = read_link(&temporary, &operation, serial);
        planted.intent_digest = "22".repeat(32);
        write_link(&temporary, &operation, serial, &planted);
        let before = snapshot_tree(&temporary.journal_path());
        let section = crate::registry::enter_registry(&dirs).unwrap();
        let error = create_owner_intent_link(&section, held.owner(), &intent).unwrap_err();
        drop(section);
        assert!(matches!(
            error,
            ConvergenceError::Unknown {
                role: DurableRole::OwnerIntentLink
            }
        ));
        // No overwrite: the disagreeing bytes survive untouched.
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert_eq!(
            read_link(&temporary, &operation, serial).intent_digest,
            planted.intent_digest
        );
        drop(held);
    }

    #[test]
    fn malformed_link_is_refused_on_read() {
        let (temporary, admitted) = admit_days("link-malformed", &["20260823"]);
        let held = continue_ok(&admitted);
        let operation = held.owner().operation_id().to_owned();
        let serial = held.serial.unwrap();
        let path = link_path(&temporary, &operation, serial);
        let bytes = std::fs::read(&path).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("surprise".to_owned(), serde_json::Value::Bool(true));
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let section = crate::registry::enter_registry(&dirs).unwrap();
        let error = load_owner_intent_link(&section, &operation, serial).unwrap_err();
        drop(section);
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::UnknownField { .. })
        ));
        drop(held);
    }

    #[test]
    fn crash_before_link_sync_leaves_a_durable_link() {
        let (temporary, admitted) = admit_days("link-crash", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let owner = prepare_for(&admitted, &operation);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let guard = fail_after(PublishFault::AfterOwnerIntentLink);
        let error = held.continue_with(proof).unwrap_err();
        drop(guard);
        assert!(matches!(
            error,
            ConvergenceError::Io {
                role: DurableRole::OwnerIntentLink,
                ..
            }
        ));
        let serial = held.serial.unwrap();
        assert!(link_path(&temporary, operation.as_hex(), serial).is_file());
        drop(held);
        // A fresh admission of the same operation now classifies as recovery.
        let resumed = reopen(&temporary, &["20260823"]);
        let owner = prepare_for(&resumed, &operation);
        let held = resumed.begin(owner).unwrap();
        let outcome = crate::owner::ClaimAdmission::admit(&held, held.owner()).unwrap();
        assert!(outcome.is_existing_link());
    }

    #[test]
    fn crash_after_link_sync_leaves_a_durable_link() {
        let (temporary, admitted) = admit_days("link-crash-sync", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let owner = prepare_for(&admitted, &operation);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let guard = fail_after(PublishFault::AfterOwnerIntentLinkSync);
        let error = held.continue_with(proof).unwrap_err();
        drop(guard);
        assert!(matches!(
            error,
            ConvergenceError::Io {
                role: DurableRole::OwnerIntentLink,
                ..
            }
        ));
        assert!(link_path(&temporary, operation.as_hex(), held.serial.unwrap()).is_file());
        drop(held);
    }

    #[test]
    fn intent_without_link_resumes_at_the_same_serial() {
        let (temporary, admitted) = admit_days("link-resume", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let owner = prepare_for(&admitted, &operation);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let serial = held.serial.unwrap();
        drop(held);
        // Simulate the intent-without-link half: the intent is durable, the
        // link never landed.
        std::fs::remove_dir_all(links_root(&temporary).join(operation.as_hex())).unwrap();
        let claim_before = std::fs::read(
            temporary
                .journal_path()
                .join("health/convergence/claim/head.json"),
        )
        .unwrap();
        let resumed = reopen(&temporary, &["20260823"]);
        let owner = prepare_for(&resumed, &operation);
        let mut held = resumed.begin(owner).unwrap();
        // Absence again, so this is a resume rather than a recovery.
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        assert_eq!(
            held.serial.unwrap(),
            serial,
            "resume must not take a new serial"
        );
        assert!(link_path(&temporary, operation.as_hex(), serial).is_file());
        assert_eq!(
            claim_before,
            std::fs::read(
                temporary
                    .journal_path()
                    .join("health/convergence/claim/head.json")
            )
            .unwrap(),
            "resume must introduce no new claim"
        );
        drop(held);
    }

    #[test]
    fn later_dirty_links_its_own_serial_and_keeps_the_first() {
        let (temporary, admitted) = admit_days("link-later", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let operation = held.owner().operation_id().to_owned();
        let first = held.serial.unwrap();
        // Later-dirty is a successor on the still-live claim, before terminal.
        crate::test_support::advance_dirty_ok(&mut held);
        let second = held.serial.unwrap();
        assert_ne!(first, second);
        // Both links exist; the predecessor's link is not rewritten.
        let first_link = read_link(&temporary, &operation, first);
        let second_link = read_link(&temporary, &operation, second);
        assert_eq!(first_link.serial, first);
        assert_eq!(second_link.serial, second);
        assert_ne!(first_link.intent_digest, second_link.intent_digest);
        drop(held);
    }

    #[test]
    fn another_operation_has_its_own_empty_link_set() {
        let (temporary, admitted) = admit_days("link-foreign", &["20260823"]);
        let held = continue_ok(&admitted);
        let linked_operation = held.owner().operation_id().to_owned();
        let linked_serial = held.serial.unwrap();
        drop(held);
        // A different operation is a first admission even though the journal
        // already holds a link for another one: the absence rule is scoped to
        // the operation, not to the registry.
        let other = prepared_owner(&admitted).unwrap();
        let other_operation = other.operation_id().to_owned();
        assert_ne!(other_operation, linked_operation);
        let held = admitted.begin(other).unwrap();
        let outcome = crate::owner::ClaimAdmission::admit(&held, held.owner()).unwrap();
        assert!(!outcome.is_existing_link());
        assert!(!links_root(&temporary).join(&other_operation).exists());
        assert!(link_path(&temporary, &linked_operation, linked_serial).is_file());
        drop(held);
    }
}
