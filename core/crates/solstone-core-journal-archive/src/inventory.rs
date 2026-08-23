// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;

use crate::deny::{DenyAction, deny_member, deny_top_level};
use crate::entry::{DirectoryEntryProof, DirectoryProof, EntryProof};
use crate::source::{
    classify, list_directory, member_name, open_initial_directory, open_initial_file,
    stat_entry_for_count,
};
use crate::{
    ArchiveError, ArchiveMemberName, IncludedRootName, Inventory, InventoryEntry, JournalEntryKind,
    SkippedRootName,
};

pub(crate) fn build(root: &impl AsFd) -> Result<Inventory, ArchiveError> {
    let mut inventory = Inventory::default();
    let mut entries = Vec::new();

    for name in list_directory(root, None)? {
        let name_str = utf8_name(&name, None)?;
        let member = ArchiveMemberName::new(name_str.clone());
        let stat = stat_entry_for_count(root, &name, &member)?;
        match classify(&stat) {
            JournalEntryKind::Directory => {
                if deny_top_level(&name_str) == DenyAction::TreePrune {
                    inventory
                        .skipped_root_names
                        .push(SkippedRootName::new(name_str));
                    continue;
                }
                if deny_member(&name_str) {
                    continue;
                }
                inventory
                    .included_root_names
                    .push(IncludedRootName::new(name_str));
                let (directory, root_proof) = open_initial_directory(root, &name, &member, &stat)?;
                inventory.directory_proofs.push(DirectoryEntryProof {
                    components: vec![name.clone()].into_boxed_slice(),
                    directories: vec![root_proof].into_boxed_slice(),
                });
                walk_directory(
                    &directory,
                    vec![name],
                    vec![root_proof],
                    &mut entries,
                    &mut inventory,
                )?;
            }
            JournalEntryKind::RegularFile => {
                if deny_member(&name_str) {
                    continue;
                }
                count_file(&[], &name, &mut inventory);
                let file = open_initial_file(root, &name, &member, &stat)?;
                entries.push(InventoryEntry::new(
                    member,
                    EntryProof {
                        components: vec![name].into_boxed_slice(),
                        directories: Box::new([]),
                        file,
                    },
                ));
            }
            kind => {
                return Err(ArchiveError::UnsafeJournalEntry { member, kind });
            }
        }
    }

    inventory
        .included_root_names
        .sort_by(|left, right| left.as_str().as_bytes().cmp(right.as_str().as_bytes()));
    inventory
        .skipped_root_names
        .sort_by(|left, right| left.as_str().as_bytes().cmp(right.as_str().as_bytes()));
    entries.sort_by(|left: &InventoryEntry, right| {
        left.member_name()
            .as_str()
            .as_bytes()
            .cmp(right.member_name().as_str().as_bytes())
    });
    inventory.entries = entries;
    Ok(inventory)
}

fn walk_directory(
    directory: &OwnedFd,
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
                if deny_member(member.as_str()) {
                    continue;
                }
                count_directory(&components, &name, inventory);
                let (child, proof) = open_initial_directory(directory, &name, &member, &stat)?;
                let mut child_proofs = directory_proofs.clone();
                child_proofs.push(proof);
                inventory.directory_proofs.push(DirectoryEntryProof {
                    components: child_components.clone().into_boxed_slice(),
                    directories: child_proofs.clone().into_boxed_slice(),
                });
                walk_directory(&child, child_components, child_proofs, entries, inventory)?;
            }
            JournalEntryKind::RegularFile => {
                if deny_member(member.as_str()) {
                    continue;
                }
                count_file(&components, &name, inventory);
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

fn count_directory(parent_components: &[OsString], name: &OsStr, inventory: &mut Inventory) {
    if first_component(parent_components) == Some(b"chronicle")
        && parent_components.len() == 1
        && is_eight_digit_name(name)
    {
        inventory.day_count = inventory.day_count.saturating_add(1);
    }
}

fn count_file(parent_components: &[OsString], name: &OsStr, inventory: &mut Inventory) {
    let is_immediate = parent_components.len() == 2;
    if first_component(parent_components) == Some(b"entities")
        && is_immediate
        && name.as_bytes() == b"entity.json"
    {
        inventory.entity_count = inventory.entity_count.saturating_add(1);
    }
    if first_component(parent_components) == Some(b"facets")
        && is_immediate
        && name.as_bytes() == b"facet.json"
    {
        inventory.facet_count = inventory.facet_count.saturating_add(1);
    }
}

fn first_component(parent_components: &[OsString]) -> Option<&[u8]> {
    parent_components.first().map(|name| name.as_bytes())
}

fn is_eight_digit_name(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 8 && bytes.iter().all(u8::is_ascii_digit)
}

fn utf8_name(name: &OsStr, member: Option<&ArchiveMemberName>) -> Result<String, ArchiveError> {
    std::str::from_utf8(name.as_bytes())
        .map(str::to_owned)
        .map_err(|_| ArchiveError::UnsafeJournalEntry {
            member: member
                .cloned()
                .unwrap_or_else(|| ArchiveMemberName::new("<invalid>".to_owned())),
            kind: JournalEntryKind::Other,
        })
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

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod membership_tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::ArchiveSource;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = PathBuf::from("/var/tmp").join(format!(
                "sja-inv-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create temporary test directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn journal(temp: &TempDir) -> PathBuf {
        let root = temp.path.join("journal");
        fs::create_dir(&root).expect("create journal root");
        root
    }

    fn write(root: &Path, member: &str, bytes: &[u8]) {
        let path = root.join(member);
        fs::create_dir_all(path.parent().expect("member has parent")).expect("create parent");
        fs::write(path, bytes).expect("write member");
    }

    fn members(source: &ArchiveSource) -> Vec<String> {
        source
            .inventory()
            .entries()
            .iter()
            .map(|entry| entry.member_name().as_str().to_owned())
            .collect()
    }

    #[test]
    fn membership_is_the_deny_list_not_an_allow_list() {
        let temporary = TempDir::new("ac1");
        let root = journal(&temporary);
        write(&root, "identity/partner.md", b"hello");
        write(&root, "chronicle/20260101/a.txt", b"a");
        let source = ArchiveSource::open(&root).expect("open source");
        let names = members(&source);
        assert!(names.iter().any(|name| name == "identity/partner.md"));
        assert!(names.iter().any(|name| name == "chronicle/20260101/a.txt"));
        assert!(
            source
                .inventory()
                .included_root_names()
                .iter()
                .any(|name| name.as_str() == "identity")
        );
    }

    #[test]
    fn basename_exclude_is_any_depth_and_star_does_not_cross_slash() {
        let temporary = TempDir::new("ac2");
        let root = journal(&temporary);
        write(&root, "chronicle/20260101/foo.sqlite", b"db");
        write(&root, "chronicle/20260101/keep.txt", b"ok");
        write(&root, "chronicle/20260101/nested/apps/note.txt", b"nested");
        let source = ArchiveSource::open(&root).expect("open source");
        let names = members(&source);
        assert!(!names.iter().any(|name| name.ends_with("foo.sqlite")));
        assert!(
            names
                .iter()
                .any(|name| name == "chronicle/20260101/keep.txt")
        );
        assert!(
            names
                .iter()
                .any(|name| name == "chronicle/20260101/nested/apps/note.txt"),
            "trailing-slash apps/ prunes only at top level: {names:?}"
        );
    }

    #[test]
    fn top_level_tree_prunes_drop_config_and_apps() {
        let temporary = TempDir::new("ac3");
        let root = journal(&temporary);
        write(&root, "config/journal.json", b"{}");
        write(&root, "apps/observer/x.json", b"{}");
        write(&root, "chronicle/20260101/a.txt", b"a");
        let source = ArchiveSource::open(&root).expect("open source");
        let names = members(&source);
        assert_eq!(names, vec!["chronicle/20260101/a.txt".to_owned()]);
        assert_eq!(
            source
                .inventory()
                .skipped_root_names()
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>(),
            vec!["apps", "config"]
        );
    }
}
