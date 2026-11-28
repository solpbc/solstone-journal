// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;

use crate::deny::{DenyAction, deny_legacy_symlink, deny_member, deny_top_level};
use crate::entry::{DirectoryEntryProof, DirectoryProof, EntryProof};
use crate::live::SnapshotMode;
use crate::source::{
    classify, list_directory, member_name, open_initial_directory, open_initial_file,
    stat_entry_for_count,
};
use crate::{
    ArchiveError, ArchiveMemberName, IncludedRootName, Inventory, InventoryEntry, JournalEntryKind,
    SkippedRootName,
};

pub(crate) fn build(
    root: &impl AsFd,
    snapshot_mode: SnapshotMode,
) -> Result<Inventory, ArchiveError> {
    let mut inventory = Inventory::default();
    let mut entries = Vec::new();

    for name in list_directory(root, None)? {
        let name_str = utf8_name(&name, None)?;
        let member = ArchiveMemberName::new(name_str.clone());
        let stat = match stat_entry_for_count(root, &name, &member) {
            Ok(stat) => stat,
            Err(ArchiveError::SourceChanged { .. })
                if snapshot_mode == SnapshotMode::LiveJournal && deny_member(&name_str, false) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let kind = classify(&stat);
        if kind == JournalEntryKind::Directory && deny_top_level(&name_str) == DenyAction::TreePrune
        {
            inventory
                .skipped_root_names
                .push(SkippedRootName::new(name_str));
            continue;
        }
        if deny_member(&name_str, kind == JournalEntryKind::RegularFile) {
            continue;
        }
        match kind {
            JournalEntryKind::Directory => {
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
                    snapshot_mode,
                    &mut entries,
                    &mut inventory,
                )?;
            }
            JournalEntryKind::RegularFile => {
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
            JournalEntryKind::Symlink => {
                if !deny_legacy_symlink(member.as_str()) {
                    inventory.skipped_symlinks.push(member);
                }
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
    inventory
        .skipped_symlinks
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
    snapshot_mode: SnapshotMode,
    entries: &mut Vec<InventoryEntry>,
    inventory: &mut Inventory,
) -> Result<(), ArchiveError> {
    for name in list_directory(directory, member_name(&components).ok().as_ref())? {
        let mut child_components = components.clone();
        child_components.push(name.clone());
        let member = member_name(&child_components)?;
        let stat = match stat_entry_for_count(directory, &name, &member) {
            Ok(stat) => stat,
            Err(ArchiveError::SourceChanged { .. })
                if snapshot_mode == SnapshotMode::LiveJournal
                    && deny_member(member.as_str(), false) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let kind = classify(&stat);
        if deny_member(member.as_str(), kind == JournalEntryKind::RegularFile) {
            continue;
        }
        match kind {
            JournalEntryKind::Directory => {
                count_directory(&components, &name, inventory);
                let (child, proof) = open_initial_directory(directory, &name, &member, &stat)?;
                let mut child_proofs = directory_proofs.clone();
                child_proofs.push(proof);
                inventory.directory_proofs.push(DirectoryEntryProof {
                    components: child_components.clone().into_boxed_slice(),
                    directories: child_proofs.clone().into_boxed_slice(),
                });
                walk_directory(
                    &child,
                    child_components,
                    child_proofs,
                    snapshot_mode,
                    entries,
                    inventory,
                )?;
            }
            JournalEntryKind::RegularFile => {
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
            JournalEntryKind::Symlink => {
                if !deny_legacy_symlink(member.as_str()) {
                    inventory.skipped_symlinks.push(member);
                }
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
    fn operational_excludes_preserve_unrelated_owner_files() {
        let temporary = TempDir::new("ac2");
        let root = journal(&temporary);
        write(&root, "chronicle/20260101/foo.sqlite", b"db");
        write(&root, "chronicle/20260101/keep.txt", b"ok");
        write(&root, "chronicle/20260101/nested/apps/note.txt", b"nested");
        let source = ArchiveSource::open(&root).expect("open source");
        let names = members(&source);
        assert!(names.iter().any(|name| name.ends_with("foo.sqlite")));
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
    fn colliding_owner_names_survive_beside_operational_controls() {
        let temporary = TempDir::new("owner-names");
        let root = journal(&temporary);
        for member in [
            "mcp-endpoint/pop.ed25519.pk8",
            "peers/remote/client.key",
            "peers/remote/client.crt",
            "entities/cache/entity.json",
            "entities/indexer/entity.json",
            "facets/cache/facet.json",
            "facets/indexer/facet.json",
            "imports/item/original.key",
            "imports/health-dedupe.sqlite-notes/original.key",
            "imports/health-dedupe.sqlite-wal/original.key",
            "chronicle/20260101/indexer/120001_60.sqlite/original.key",
            "imports/item/.removing",
            "imports/item/notes.removing_original",
            "imports/health-dedupe.sqlite/original.key",
            "chronicle/20260102/indexer/indexer.sqlite/original.key",
            "imports/item/cache/owner.sqlite",
            "chronicle/20260101/indexer/120000_60/slides.key",
            "chronicle/20260101/source/120000_60/audio.jsonl.lock",
            "chronicle/20260101/source/120000_60/.tmp_123_4.tmp",
            "imports/item/peers/original.txt",
        ] {
            write(&root, member, b"owner bytes");
        }
        for member in [
            "indexer/journal.sqlite",
            ".removing_root/owner.json",
            "chronicle/20260101/.removing_flat/owner.json",
            "chronicle/20260101/source/.removing_qualified/owner.json",
            "imports/item/deep/.removing_deep/owner.json",
            "chronicle/20260101/120000_60/.removing_original",
            "imports/.health-dedupe.sqlite.rebuild-journal",
            "chronicle/20260101/indexer/indexer.sqlite",
            "health/brain-fingerprint.key",
            "config/.tmp_123_4.tmp",
        ] {
            write(&root, member, b"operational secret control");
        }
        let source = ArchiveSource::open(&root).unwrap();
        let names = members(&source);
        assert_eq!(names.len(), 20, "{names:?}");
        assert!(names.contains(&"entities/cache/entity.json".to_owned()));
        assert!(names.contains(&"facets/indexer/facet.json".to_owned()));
        assert!(names.contains(&"chronicle/20260101/indexer/120000_60/slides.key".to_owned()));
        assert!(
            !names
                .iter()
                .any(|name| name == "chronicle/20260101/indexer/indexer.sqlite")
        );
    }

    #[test]
    fn included_owner_roots_keep_apps_configuration_and_connections() {
        let temporary = TempDir::new("ac3");
        let root = journal(&temporary);
        write(&root, "config/journal.json", b"{}");
        write(&root, "apps/observer/x.json", b"{}");
        write(&root, "chronicle/20260101/a.txt", b"a");
        write(&root, "peers/remote/client.key", b"key");
        write(&root, "peers/remote/client.crt", b"crt");
        write(&root, "imports/item/peers/original.txt", b"txt");
        let source = ArchiveSource::open(&root).expect("open source");
        let names = members(&source);
        assert_eq!(
            names,
            vec![
                "apps/observer/x.json".to_owned(),
                "chronicle/20260101/a.txt".to_owned(),
                "config/journal.json".to_owned(),
                "imports/item/peers/original.txt".to_owned(),
                "peers/remote/client.crt".to_owned(),
                "peers/remote/client.key".to_owned(),
            ]
        );
        assert!(names.contains(&"config/journal.json".to_owned()));
        assert!(names.contains(&"peers/remote/client.key".to_owned()));
        assert!(names.contains(&"peers/remote/client.crt".to_owned()));
        assert_eq!(
            source
                .inventory()
                .skipped_root_names()
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>(),
            Vec::<&str>::new()
        );
        let included = source
            .inventory()
            .included_root_names()
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>();
        assert!(included.contains(&"apps"));
        assert!(included.contains(&"chronicle"));
        assert!(included.contains(&"config"));
        assert!(included.contains(&"imports"));
    }
}
