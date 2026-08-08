// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;

use crate::entry::{DirectoryEntryProof, DirectoryProof, EntryProof};
use crate::source::{
    classify, list_directory, member_name, open_initial_directory, open_initial_file,
    root_entry_missing, stat_entry_for_count,
};
use crate::{
    ArchiveError, ArchiveMemberName, Inventory, InventoryEntry, JournalEntryKind, SkippedRootName,
};

const ROOTS: [&str; 4] = ["chronicle", "entities", "facets", "imports"];

pub(crate) fn build(root: &OwnedFd) -> Result<Inventory, ArchiveError> {
    let mut inventory = Inventory {
        skipped_root_names: skipped_root_names(root)?,
        ..Inventory::default()
    };

    for root_name in ROOTS {
        let root_component = OsString::from(root_name);
        if root_entry_missing(root, &root_component)? {
            continue;
        }
        let root_member = ArchiveMemberName::new(root_name.to_owned());
        let root_stat = stat_entry_for_count(root, &root_component, &root_member)?;
        let (directory, root_proof) =
            open_initial_directory(root, &root_component, &root_member, &root_stat)?;
        inventory.directory_proofs.push(DirectoryEntryProof {
            components: vec![root_component.clone()].into_boxed_slice(),
            directories: vec![root_proof].into_boxed_slice(),
        });
        let mut entries = Vec::new();
        walk_directory(
            &directory,
            root_name,
            vec![root_component],
            vec![root_proof],
            &mut entries,
            &mut inventory,
        )?;
        entries.sort_by(|left: &InventoryEntry, right| {
            left.member_name()
                .as_str()
                .as_bytes()
                .cmp(right.member_name().as_str().as_bytes())
        });
        inventory.entries.extend(entries);
    }
    Ok(inventory)
}

fn skipped_root_names(root: &OwnedFd) -> Result<Vec<SkippedRootName>, ArchiveError> {
    let mut skipped = Vec::new();
    for name in list_directory(root, None)? {
        let name = match std::str::from_utf8(name.as_bytes()) {
            Ok(name) => name,
            Err(_) => {
                return Err(ArchiveError::UnsafeJournalEntry {
                    member: ArchiveMemberName::new("<invalid>".to_owned()),
                    kind: JournalEntryKind::Other,
                });
            }
        };
        if !ROOTS.contains(&name) {
            skipped.push(SkippedRootName::new(name.to_owned()));
        }
    }
    skipped.sort_by(|left, right| left.as_str().as_bytes().cmp(right.as_str().as_bytes()));
    Ok(skipped)
}

fn walk_directory(
    directory: &OwnedFd,
    root_name: &str,
    components: Vec<OsString>,
    directory_proofs: Vec<DirectoryProof>,
    entries: &mut Vec<InventoryEntry>,
    inventory: &mut Inventory,
) -> Result<(), ArchiveError> {
    for name in list_directory(directory, member_name(&components).ok().as_ref())? {
        let mut child_components = components.clone();
        child_components.push(name.clone());
        let member = member_name(&child_components)?;
        let stat = stat_entry_for_count(directory, &name, &member)?;
        match classify(&stat) {
            JournalEntryKind::Directory => {
                count_directory(root_name, &components, &name, inventory);
                let (child, proof) = open_initial_directory(directory, &name, &member, &stat)?;
                let mut child_proofs = directory_proofs.clone();
                child_proofs.push(proof);
                inventory.directory_proofs.push(DirectoryEntryProof {
                    components: child_components.clone().into_boxed_slice(),
                    directories: child_proofs.clone().into_boxed_slice(),
                });
                walk_directory(
                    &child,
                    root_name,
                    child_components,
                    child_proofs,
                    entries,
                    inventory,
                )?;
            }
            JournalEntryKind::RegularFile => {
                count_file(root_name, &components, &name, inventory);
                let file = open_initial_file(directory, &name, &member, &stat)?;
                entries.push(InventoryEntry::new(
                    member,
                    EntryProof {
                        components: child_components.into_boxed_slice(),
                        directories: directory_proofs.clone().into_boxed_slice(),
                        file,
                    },
                ));
            }
            kind => {
                return Err(ArchiveError::UnsafeJournalEntry { member, kind });
            }
        }
    }
    Ok(())
}

fn count_directory(
    root_name: &str,
    parent_components: &[OsString],
    name: &OsStr,
    inventory: &mut Inventory,
) {
    if root_name == "chronicle" && parent_components.len() == 1 && is_eight_digit_name(name) {
        inventory.day_count = inventory.day_count.saturating_add(1);
    }
}

fn count_file(
    root_name: &str,
    parent_components: &[OsString],
    name: &OsStr,
    inventory: &mut Inventory,
) {
    let is_immediate = parent_components.len() == 2;
    if root_name == "entities" && is_immediate && name.as_bytes() == b"entity.json" {
        inventory.entity_count = inventory.entity_count.saturating_add(1);
    }
    if root_name == "facets" && is_immediate && name.as_bytes() == b"facet.json" {
        inventory.facet_count = inventory.facet_count.saturating_add(1);
    }
}

fn is_eight_digit_name(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 8 && bytes.iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use nix::sys::stat::SFlag;

    use crate::{JournalEntryKind, source::classify_mode};

    #[test]
    fn file_type_classifier_keeps_device_kinds_distinct() {
        // Production obtains these bits from FileStat. Device values are tested
        // directly as synthetic SFlag bits, requiring no privileged mknod.
        for (flag, expected) in [
            (SFlag::S_IFCHR, JournalEntryKind::CharacterDevice),
            (SFlag::S_IFBLK, JournalEntryKind::BlockDevice),
        ] {
            assert_eq!(classify_mode(flag), expected);
        }
    }
}
