// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Immutable directly-addressed owner-intent links.
//!
//! The initial link name is derived from the prepared owner binding and the
//! exact request-selector digest, both known before allocation. Therefore an
//! admission needs one bounded open, never a registry directory listing.

use std::ffi::OsStr;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::{create_directory_bound, sync_dir_bound};

use crate::access::RegistrySection;
use crate::error::{ConvergenceError, DurableRole};
use crate::layout::{LINKS, link_name, successor_link_name};
use crate::owner::OwnerBinding;
use crate::schema::{
    Intent, OwnerIntentLink, ROLE_OWNER_INTENT_LINK, SCHEMA_VERSION, read_json,
    write_json_exclusive,
};
use crate::walk::open_dir;

/// The only admission classification for one addressed link.
pub(crate) enum LinkResolution {
    Absent,
    Exact(Box<OwnerIntentLink>),
    Unknown,
}

fn links_dir(section: &RegistrySection<'_>) -> Result<Option<OwnedFd>, ConvergenceError> {
    open_dir(section.registry(), LINKS)
}

fn ensure_links_dir(section: &RegistrySection<'_>) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(section.registry(), OsStr::new(LINKS), 0o700).map_err(|error| {
        ConvergenceError::Io {
            operation: "create links directory",
            role: DurableRole::Directory,
            source: std::io::Error::other(error.to_string()),
        }
    })?;
    links_dir(section)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

fn expected_link(owner: &OwnerBinding, intent: &Intent) -> OwnerIntentLink {
    OwnerIntentLink {
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
    }
}

fn link_matches_owner(link: &OwnerIntentLink, owner: &OwnerBinding) -> bool {
    let expected_days: Vec<String> = owner
        .selector()
        .days()
        .iter()
        .map(|day| day.as_str().to_owned())
        .collect();
    let expected_subdigest = match crate::schema::day_set_subdigest(owner.selector().days()) {
        Ok(digest) => digest.as_hex().to_owned(),
        Err(_) => return false,
    };
    link.role == ROLE_OWNER_INTENT_LINK
        && link.schema_version == SCHEMA_VERSION
        && link.journal_id == owner.journal_id()
        && link.root_id == owner.root_id()
        && link.operation_id == owner.operation_id()
        && link.owner_binding_digest == owner.digest_hex()
        && link.day_set == expected_days
        && link.day_set_subdigest == expected_subdigest
        && link.selector_digest == owner.selector_digest()
}

/// One bounded descriptor-relative open of the initial link name.
pub(crate) fn resolve_owner_intent_link(
    section: &RegistrySection<'_>,
    owner: &OwnerBinding,
) -> Result<LinkResolution, ConvergenceError> {
    let Some(links) = links_dir(section)? else {
        return Ok(LinkResolution::Absent);
    };
    let name = link_name(owner.digest_hex(), owner.selector_digest());
    match read_json::<OwnerIntentLink>(&links, &name, DurableRole::OwnerIntentLink) {
        Ok(None) => Ok(LinkResolution::Absent),
        Ok(Some(link)) if link_matches_owner(&link, owner) => {
            Ok(LinkResolution::Exact(Box::new(link)))
        }
        Ok(Some(_)) | Err(_) => Ok(LinkResolution::Unknown),
    }
}

/// Read the initial link or an exactly addressed successor. The caller knows
/// the serial from the linked intent, so this still never lists a directory.
pub(crate) fn load_owner_intent_link(
    section: &RegistrySection<'_>,
    owner: &OwnerBinding,
    serial: u64,
) -> Result<Option<OwnerIntentLink>, ConvergenceError> {
    match resolve_owner_intent_link(section, owner)? {
        LinkResolution::Exact(link) if link.serial == serial => Ok(Some(*link)),
        LinkResolution::Exact(_) => {
            let Some(links) = links_dir(section)? else {
                return Ok(None);
            };
            let name = successor_link_name(owner.digest_hex(), owner.selector_digest(), serial);
            match read_json::<OwnerIntentLink>(&links, &name, DurableRole::OwnerIntentLink) {
                Ok(Some(link)) if link_matches_owner(&link, owner) && link.serial == serial => {
                    Ok(Some(link))
                }
                Ok(None) => Ok(None),
                Ok(Some(_)) | Err(_) => Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink,
                }),
            }
        }
        LinkResolution::Absent => Ok(None),
        LinkResolution::Unknown => Err(ConvergenceError::Unknown {
            role: DurableRole::OwnerIntentLink,
        }),
    }
}

/// Create the direct initial link, or a direct successor link. The one
/// addressed initial read decides which name is lawful: re-running its serial
/// is idempotent, while a different serial is a successor. No caller-supplied
/// stage flag chooses the durable shape.
pub(crate) fn create_owner_intent_link(
    section: &RegistrySection<'_>,
    owner: &OwnerBinding,
    intent: &Intent,
) -> Result<OwnerIntentLink, ConvergenceError> {
    let link = expected_link(owner, intent);
    let name = match resolve_owner_intent_link(section, owner)? {
        LinkResolution::Absent => link_name(owner.digest_hex(), owner.selector_digest()),
        LinkResolution::Exact(link) if link.serial == intent.serial => {
            link_name(owner.digest_hex(), owner.selector_digest())
        }
        LinkResolution::Exact(_) => {
            successor_link_name(owner.digest_hex(), owner.selector_digest(), intent.serial)
        }
        LinkResolution::Unknown => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::OwnerIntentLink,
            });
        }
    };
    let links = ensure_links_dir(section)?;
    match write_json_exclusive(&links, &name, &link, DurableRole::OwnerIntentLink) {
        Ok(_) => {}
        Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync_dir_bound(&links).map_err(|source| ConvergenceError::Io {
        operation: "sync links directory",
        role: DurableRole::OwnerIntentLink,
        source,
    })?;
    let durable = read_json::<OwnerIntentLink>(&links, &name, DurableRole::OwnerIntentLink)
        .map_err(|_| ConvergenceError::Unknown {
            role: DurableRole::OwnerIntentLink,
        })?
        .ok_or(ConvergenceError::Unknown {
            role: DurableRole::OwnerIntentLink,
        })?;
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
    use crate::test_support::{admit_days, continue_ok, prepared_owner};

    fn link_path(
        temporary: &crate::test_support::TempDir,
        owner: &OwnerBinding,
    ) -> std::path::PathBuf {
        temporary
            .journal_path()
            .join("health/convergence/registry/links")
            .join(link_name(owner.digest_hex(), owner.selector_digest()))
    }

    fn rewrite_link(
        temporary: &crate::test_support::TempDir,
        owner: &OwnerBinding,
        mutate: impl FnOnce(&mut OwnerIntentLink),
    ) {
        let path = link_path(temporary, owner);
        let bytes = std::fs::read(&path).unwrap();
        let mut link: OwnerIntentLink =
            serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap();
        mutate(&mut link);
        let mut bytes = crate::digest::canonical_json_bytes(&link).unwrap();
        bytes.push(b'\n');
        std::fs::write(path, bytes).unwrap();
    }

    fn assert_untrusted_link(mutate: impl FnOnce(&mut OwnerIntentLink)) {
        let (temporary, admitted) = admit_days("link-untrusted", &["20260823"]);
        let held = continue_ok(&admitted);
        let serial = held.serial.unwrap();
        rewrite_link(&temporary, held.owner(), mutate);
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            let error = load_owner_intent_link(section, held.owner(), serial).unwrap_err();
            assert!(matches!(
                error,
                ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink
                }
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn first_link_is_directly_addressed() {
        let (_temporary, admitted) = admit_days("link-direct", &["20260823"]);
        let held = continue_ok(&admitted);
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            assert!(matches!(
                resolve_owner_intent_link(section, held.owner())?,
                LinkResolution::Exact(_)
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn unrelated_owner_is_absent() {
        let (_temporary, admitted) = admit_days("link-absent", &["20260823"]);
        let _held = continue_ok(&admitted);
        let other = prepared_owner(&admitted).unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            assert!(matches!(
                resolve_owner_intent_link(section, &other)?,
                LinkResolution::Absent
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn later_dirty_uses_direct_successor_name() {
        let (_temporary, admitted) = admit_days("link-successor", &["20260823"]);
        let mut held = continue_ok(&admitted);
        crate::test_support::advance_dirty_ok(&mut held);
        let serial = held.serial.unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            assert!(load_owner_intent_link(section, held.owner(), serial)?.is_some());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn malformed_direct_link_is_unknown() {
        let (temporary, admitted) = admit_days("link-malformed", &["20260823"]);
        let held = continue_ok(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/registry/links")
            .join(link_name(
                held.owner().digest_hex(),
                held.owner().selector_digest(),
            ));
        std::fs::write(path, b"{bad").unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            assert!(matches!(
                resolve_owner_intent_link(section, held.owner())?,
                LinkResolution::Unknown
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn exact_link_is_recovery_evidence() {
        let (_temporary, admitted) = admit_days("link-recovery", &["20260823"]);
        let held = continue_ok(&admitted);
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            assert!(matches!(
                resolve_owner_intent_link(section, held.owner())?,
                LinkResolution::Exact(_)
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn selector_mismatch_is_unknown() {
        assert_untrusted_link(|link| link.selector_digest = "00".repeat(32));
    }

    #[test]
    fn wrong_journal_is_unknown() {
        assert_untrusted_link(|link| link.journal_id = "other-journal".to_owned());
    }

    #[test]
    fn wrong_root_is_unknown() {
        assert_untrusted_link(|link| link.root_id = "other-root".to_owned());
    }

    #[test]
    fn wrong_operation_is_unknown() {
        assert_untrusted_link(|link| link.operation_id = "00".repeat(32));
    }

    #[test]
    fn wrong_owner_binding_is_unknown() {
        assert_untrusted_link(|link| link.owner_binding_digest = "00".repeat(32));
    }

    #[test]
    fn day_set_mismatch_is_unknown() {
        assert_untrusted_link(|link| link.day_set = vec!["20260824".to_owned()]);
    }

    #[test]
    fn day_set_subdigest_mismatch_is_unknown() {
        assert_untrusted_link(|link| link.day_set_subdigest = "00".repeat(32));
    }

    #[test]
    fn wrong_role_is_unknown() {
        assert_untrusted_link(|link| link.role = "wrong-role".to_owned());
    }

    #[test]
    fn wrong_version_is_unknown() {
        assert_untrusted_link(|link| link.schema_version += 1);
    }

    #[test]
    fn unexpected_link_field_is_unknown() {
        let (temporary, admitted) = admit_days("link-unexpected", &["20260823"]);
        let held = continue_ok(&admitted);
        let serial = held.serial.unwrap();
        let path = link_path(&temporary, held.owner());
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_owned(), serde_json::Value::Bool(true));
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            assert!(matches!(
                load_owner_intent_link(section, held.owner(), serial),
                Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink
                })
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn exact_link_create_is_idempotent_after_crash_half() {
        let (_temporary, admitted) = admit_days("link-idempotent", &["20260823"]);
        let held = continue_ok(&admitted);
        let serial = held.serial.unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let intent = crate::intent::read_intent(&dirs, serial).unwrap().unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            let first = create_owner_intent_link(section, held.owner(), &intent)?;
            let second = create_owner_intent_link(section, held.owner(), &intent)?;
            assert_eq!(first, second);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn conflicting_link_blocks_recovery() {
        let (temporary, admitted) = admit_days("link-conflict", &["20260823"]);
        let held = continue_ok(&admitted);
        let serial = held.serial.unwrap();
        rewrite_link(&temporary, held.owner(), |link| {
            link.intent_digest = "00".repeat(32)
        });
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let intent = crate::intent::read_intent(&dirs, serial).unwrap().unwrap();
        crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            assert!(matches!(
                create_owner_intent_link(section, held.owner(), &intent),
                Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink
                })
            ));
            Ok(())
        })
        .unwrap();
    }
}
