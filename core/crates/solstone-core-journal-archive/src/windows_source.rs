// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows-only source traversal layered on journal-io's retained-handle APIs.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use solstone_core_journal_io::{
    InventoryBudget, JournalEntryKind, JournalRoot, WindowsInventoryEntry, WindowsInventoryError,
    enumerate_windows_inventory, read_windows_inventory_file,
};

use crate::deny::{DenyAction, deny_member, deny_top_level};
use crate::entry::{DirectoryEntryProof, DirectoryProof, EntryProof, FileProof, ProofIdentity};
use crate::{
    ArchiveError, ArchiveMemberName, IncludedRootName, Inventory, InventoryEntry,
    OpenedInventoryFile, SkippedRootName,
};

const ARCHIVE_INVENTORY_BUDGET: InventoryBudget =
    InventoryBudget::new(100_000, 64, 255, 32 * 1024, 64 * 1024 * 1024);

/// A frozen, capability-rooted portable archive source on Windows.
pub struct ArchiveSource {
    root: JournalRoot,
    inventory: Inventory,
    observed: Vec<WindowsInventoryEntry>,
    budget: InventoryBudget,
}

impl ArchiveSource {
    /// Acquire `root` once and immediately freeze its portable archive inventory.
    pub fn open(root: &Path) -> Result<Self, ArchiveError> {
        Self::open_with_budget(root, ARCHIVE_INVENTORY_BUDGET)
    }

    fn open_with_budget(root: &Path, budget: InventoryBudget) -> Result<Self, ArchiveError> {
        let retained_root = JournalRoot::open(root).map_err(crate::error::map_root_error)?;
        let observed = enumerate_windows_inventory(&retained_root, budget)
            .map_err(|error| map_windows_error(&retained_root, error, false))?
            .into_entries();
        let (inventory, observed) = build_inventory(&observed)?;
        Ok(Self {
            root: retained_root,
            inventory,
            observed,
            budget,
        })
    }

    /// Return the inventory frozen when this source was opened.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// Return the verified canonical path acquired when this source was opened.
    pub fn canonical_source(&self) -> &Path {
        self.root.canonical_path()
    }

    /// Return a fully checked copy of one frozen regular archive member.
    pub fn open_file(&self, entry: &InventoryEntry) -> Result<OpenedInventoryFile, ArchiveError> {
        let bytes =
            read_windows_inventory_file(&self.root, &entry.proof().file.observed, self.budget)
                .map_err(|error| map_windows_error(&self.root, error, true))?;
        Ok(OpenedInventoryFile::from_bytes(bytes, entry.size()))
    }

    /// Confirm the complete frozen observed inventory remains unchanged.
    pub fn revalidate(&self) -> Result<(), ArchiveError> {
        let observed = enumerate_windows_inventory(&self.root, self.budget)
            .map_err(|error| map_windows_error(&self.root, error, true))?
            .into_entries();
        let (_, observed) = build_inventory(&observed)?;
        if observed != self.observed {
            return Err(ArchiveError::SourceChanged { member: None });
        }
        Ok(())
    }
}

fn build_inventory(
    observed: &[WindowsInventoryEntry],
) -> Result<(Inventory, Vec<WindowsInventoryEntry>), ArchiveError> {
    let mut inventory = Inventory::default();
    let mut selected = Vec::new();
    let mut directory_routes = BTreeMap::<PathBuf, Vec<DirectoryProof>>::new();
    let mut omitted_directories = Vec::<PathBuf>::new();

    for observed_entry in observed {
        let relative_path = observed_entry.relative_path();
        if omitted_directories
            .iter()
            .any(|omitted| relative_path.starts_with(omitted))
        {
            continue;
        }
        let components = path_components(relative_path)?;
        let member = archive_member_name(&components)?;
        let top_level = components.len() == 1;

        match observed_entry.kind() {
            JournalEntryKind::Directory => {
                if top_level && deny_top_level(member.as_str()) == DenyAction::TreePrune {
                    inventory
                        .skipped_root_names
                        .push(SkippedRootName::new(member.as_str().to_owned()));
                    omitted_directories.push(relative_path.to_path_buf());
                    continue;
                }
                if deny_member(member.as_str()) {
                    omitted_directories.push(relative_path.to_path_buf());
                    continue;
                }
                if top_level {
                    inventory
                        .included_root_names
                        .push(IncludedRootName::new(member.as_str().to_owned()));
                }
                count_directory(&components, &mut inventory);
                let parent = parent_route(relative_path)?;
                let mut directories = directory_routes.get(&parent).cloned().unwrap_or_default();
                directories.push(DirectoryProof {
                    identity: ProofIdentity::Windows(observed_entry.identity()),
                });
                inventory.directory_proofs.push(DirectoryEntryProof {
                    components: components.clone().into_boxed_slice(),
                    directories: directories.clone().into_boxed_slice(),
                });
                directory_routes.insert(relative_path.to_path_buf(), directories);
                selected.push(observed_entry.clone());
            }
            JournalEntryKind::RegularFile => {
                if deny_member(member.as_str()) {
                    continue;
                }
                count_file(&components, &mut inventory);
                let parent = parent_route(relative_path)?;
                let directories = if parent.as_os_str().is_empty() {
                    Vec::new()
                } else {
                    directory_routes.get(&parent).cloned().ok_or_else(|| {
                        ArchiveError::SourceChanged {
                            member: Some(member.clone()),
                        }
                    })?
                };
                inventory.entries.push(InventoryEntry::new(
                    member,
                    EntryProof {
                        components: components.into_boxed_slice(),
                        directories: directories.into_boxed_slice(),
                        file: FileProof {
                            identity: ProofIdentity::Windows(observed_entry.identity()),
                            size: observed_entry.size(),
                            observed: observed_entry.clone(),
                        },
                    },
                ));
                selected.push(observed_entry.clone());
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
    inventory.entries.sort_by(|left, right| {
        left.member_name()
            .as_str()
            .as_bytes()
            .cmp(right.member_name().as_str().as_bytes())
    });
    Ok((inventory, selected))
}

fn path_components(path: &Path) -> Result<Vec<OsString>, ArchiveError> {
    path.components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_os_string()),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => Err(ArchiveError::UnsafeJournalEntry {
                member: ArchiveMemberName::new("<invalid>".to_owned()),
                kind: JournalEntryKind::Other,
            }),
        })
        .collect()
}

fn archive_member_name(components: &[OsString]) -> Result<ArchiveMemberName, ArchiveError> {
    let mut rendered = Vec::with_capacity(components.len());
    for component in components {
        let Some(component) = component.to_str() else {
            return Err(ArchiveError::UnsafeJournalEntry {
                member: ArchiveMemberName::new("<invalid>".to_owned()),
                kind: JournalEntryKind::Other,
            });
        };
        rendered.push(component);
    }
    Ok(ArchiveMemberName::new(rendered.join("/")))
}

fn parent_route(path: &Path) -> Result<PathBuf, ArchiveError> {
    path.parent()
        .map(Path::to_path_buf)
        .ok_or(ArchiveError::SourceChanged { member: None })
}

fn count_directory(components: &[OsString], inventory: &mut Inventory) {
    if components.len() == 2
        && component_equals(&components[0], "chronicle")
        && components[1]
            .to_str()
            .is_some_and(|name| name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit()))
    {
        inventory.day_count = inventory.day_count.saturating_add(1);
    }
}

fn count_file(components: &[OsString], inventory: &mut Inventory) {
    if components.len() != 3 {
        return;
    }
    if component_equals(&components[0], "entities")
        && component_equals(&components[2], "entity.json")
    {
        inventory.entity_count = inventory.entity_count.saturating_add(1);
    }
    if component_equals(&components[0], "facets") && component_equals(&components[2], "facet.json")
    {
        inventory.facet_count = inventory.facet_count.saturating_add(1);
    }
}

fn component_equals(component: &OsString, expected: &str) -> bool {
    component.to_str() == Some(expected)
}

fn map_windows_error(
    root: &JournalRoot,
    error: WindowsInventoryError,
    after_observation: bool,
) -> ArchiveError {
    match error {
        WindowsInventoryError::Root(error) => crate::error::map_root_error(error),
        WindowsInventoryError::Unsupported { operation, .. } => ArchiveError::UnsupportedJournal {
            root: root.canonical_path().to_path_buf(),
            reason: operation,
        },
        WindowsInventoryError::BudgetExceeded { .. } => ArchiveError::UnsupportedJournal {
            root: root.canonical_path().to_path_buf(),
            reason: "archive source exceeds its supported inventory budget",
        },
        WindowsInventoryError::InvalidName { path }
        | WindowsInventoryError::ReparsePoint { path }
        | WindowsInventoryError::NotDirectory { path }
        | WindowsInventoryError::NotRegular { path }
            if after_observation =>
        {
            ArchiveError::SourceChanged {
                member: member_from_path(&path),
            }
        }
        WindowsInventoryError::InvalidName { path }
        | WindowsInventoryError::ReparsePoint { path }
        | WindowsInventoryError::NotDirectory { path }
        | WindowsInventoryError::NotRegular { path } => ArchiveError::UnsafeJournalEntry {
            member: member_from_path(&path)
                .unwrap_or_else(|| ArchiveMemberName::new("<invalid>".to_owned())),
            kind: JournalEntryKind::Other,
        },
        WindowsInventoryError::IdentityChanged { path }
        | WindowsInventoryError::NamespaceChanged { path } => ArchiveError::SourceChanged {
            member: member_from_path(&path),
        },
        WindowsInventoryError::Io {
            operation,
            path,
            source,
        } => ArchiveError::SourceIo {
            operation,
            member: member_from_path(&path),
            source,
        },
    }
}

fn member_from_path(path: &Path) -> Option<ArchiveMemberName> {
    let components = path_components(path).ok()?;
    archive_member_name(&components).ok()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use solstone_core_journal_io::InventoryBudget;

    use super::{ARCHIVE_INVENTORY_BUDGET, ArchiveSource, path_components};
    use crate::ArchiveError;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-journal-archive-windows-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create temporary directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write(root: &Path, member: &str, bytes: &[u8]) {
        let path = root.join(member);
        fs::create_dir_all(path.parent().expect("member has parent"))
            .expect("create member parent");
        fs::write(path, bytes).expect("write member");
    }

    fn fixture(name: &str) -> (TempDir, PathBuf) {
        let temporary = TempDir::new(name);
        let root = temporary.path.join("journal");
        fs::create_dir(&root).expect("create journal root");
        write(&root, "chronicle/20260101/segment.jsonl", b"segment");
        write(&root, "entities/alice/entity.json", b"entity");
        write(&root, "facets/work/facet.json", b"facet");
        write(&root, "config/journal.json", b"omitted");
        (temporary, root)
    }

    #[test]
    fn source_freezes_portable_members_and_checked_bytes() {
        let (_temporary, root) = fixture("ordinary");
        let source = ArchiveSource::open(&root).expect("open source");
        let members = source
            .inventory()
            .entries()
            .iter()
            .map(|entry| entry.member_name().as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            members,
            [
                "chronicle/20260101/segment.jsonl",
                "entities/alice/entity.json",
                "facets/work/facet.json",
            ]
        );
        assert_eq!(source.inventory().day_count(), 1);
        assert_eq!(source.inventory().entity_count(), 1);
        assert_eq!(source.inventory().facet_count(), 1);
        let entry = source
            .inventory()
            .entries()
            .iter()
            .find(|entry| entry.member_name().as_str() == "chronicle/20260101/segment.jsonl")
            .expect("segment entry");
        assert_eq!(
            source.open_file(entry).expect("checked read").into_bytes(),
            b"segment"
        );
        source.revalidate().expect("revalidate frozen source");
    }

    #[test]
    fn inventory_and_checked_read_budget_refusals_do_not_construct_partial_results() {
        let (_temporary, root) = fixture("budget");
        for budget in [
            InventoryBudget::new(0, 64, 255, 32 * 1024, 64 * 1024 * 1024),
            InventoryBudget::new(100, 0, 255, 32 * 1024, 64 * 1024 * 1024),
            InventoryBudget::new(100, 64, 1, 32 * 1024, 64 * 1024 * 1024),
            InventoryBudget::new(100, 64, 255, 1, 64 * 1024 * 1024),
        ] {
            assert!(matches!(
                ArchiveSource::open_with_budget(&root, budget),
                Err(ArchiveError::UnsupportedJournal {
                    reason: "archive source exceeds its supported inventory budget",
                    ..
                })
            ));
        }

        let source = ArchiveSource::open_with_budget(
            &root,
            InventoryBudget::new(
                ARCHIVE_INVENTORY_BUDGET.maximum_entries(),
                ARCHIVE_INVENTORY_BUDGET.maximum_depth(),
                ARCHIVE_INVENTORY_BUDGET.maximum_member_utf8_bytes(),
                ARCHIVE_INVENTORY_BUDGET.maximum_relative_path_utf16_bytes(),
                0,
            ),
        )
        .expect("inventory admits zero-byte checked-read budget");
        let entry = source
            .inventory()
            .entries()
            .first()
            .expect("inventory entry");
        assert!(matches!(
            source.open_file(entry),
            Err(ArchiveError::UnsupportedJournal {
                reason: "archive source exceeds its supported inventory budget",
                ..
            })
        ));
    }

    #[test]
    fn retained_root_survives_ancestor_rename_and_refuses_route_substitution() {
        let (temporary, root) = fixture("rename");
        let source = ArchiveSource::open(&root).expect("open source");
        let entry = source
            .inventory()
            .entries()
            .iter()
            .find(|entry| entry.member_name().as_str() == "chronicle/20260101/segment.jsonl")
            .expect("segment entry")
            .clone();

        let moved_outer = temporary.path.join("outer-moved");
        let outer = temporary.path.join("outer");
        fs::create_dir(&outer).expect("create ancestor directory");
        fs::rename(
            temporary.path.join("journal"),
            temporary.path.join("outer/journal"),
        )
        .expect("place journal below an ancestor");
        // The source was opened before this ancestor rename, so its retained root
        // remains authoritative despite the stale diagnostic spelling.
        fs::rename(&outer, &moved_outer).expect("rename ancestor");
        assert_eq!(
            source
                .open_file(&entry)
                .expect("read retained root")
                .into_bytes(),
            b"segment"
        );

        let original_child = moved_outer.join("journal/chronicle");
        let displaced_child = moved_outer.join("journal/chronicle-original");
        fs::rename(&original_child, &displaced_child).expect("move observed route directory");
        write(
            &moved_outer.join("journal"),
            "chronicle/20260101/segment.jsonl",
            b"replacement",
        );
        assert!(matches!(
            source.open_file(&entry),
            Err(ArchiveError::SourceChanged { .. })
        ));
    }

    #[test]
    fn malformed_component_boundary_is_unsafe() {
        assert!(matches!(
            path_components(Path::new("../untrusted")),
            Err(ArchiveError::UnsafeJournalEntry { .. })
        ));
    }

    #[test]
    fn reparse_descendant_refuses_when_link_creation_is_available() {
        let (_temporary, root) = fixture("reparse");
        let outside = root.parent().expect("temporary parent").join("outside");
        fs::create_dir(&outside).expect("create reparse target");
        let link = root.join("junction");
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            eprintln!("skipping reparse fixture: symlink creation unavailable");
            return;
        }
        assert!(matches!(
            ArchiveSource::open(&root),
            Err(ArchiveError::UnsafeJournalEntry { .. })
        ));
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn witness_faults_refuse_inventory_and_checked_bytes() {
        use solstone_core_journal_io::{
            WindowsInventoryPrimitive, run_with_windows_inventory_fault,
        };

        let (_temporary, root) = fixture("witness");
        let (result, consumed) =
            run_with_windows_inventory_fault(WindowsInventoryPrimitive::WatchArm, 1, 1, || {
                ArchiveSource::open(&root)
            });
        assert!(consumed);
        assert!(matches!(
            result,
            Err(ArchiveError::UnsupportedJournal { .. })
        ));

        let source = ArchiveSource::open(&root).expect("open ordinary source");
        let entry = source
            .inventory()
            .entries()
            .first()
            .expect("inventory entry");
        let (result, consumed) =
            run_with_windows_inventory_fault(WindowsInventoryPrimitive::WitnessCheck, 1, 1, || {
                source.open_file(entry)
            });
        assert!(consumed);
        assert!(matches!(
            result,
            Err(ArchiveError::UnsupportedJournal { .. })
        ));
    }
}
