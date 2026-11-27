// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows-only source traversal layered on journal-io's retained-handle APIs.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use solstone_core_journal_io::{
    InventoryBudget, JournalEntryKind, JournalRoot, WindowsInventoryEntry, WindowsInventoryError,
    enumerate_windows_inventory_pruned, open_windows_inventory_file_reader,
    open_windows_inventory_file_reader_live, read_windows_inventory_live_snapshot,
    revalidate_windows_live_entry,
};

use crate::deny::{DenyAction, deny_member, deny_top_level};
use crate::entry::{DirectoryEntryProof, DirectoryProof, EntryProof, FileProof, ProofIdentity};
use crate::live::{
    LIVE_METADATA_MAX_FILE_BYTES, LIVE_METADATA_TOTAL_BUDGET_BYTES, SnapshotMode,
    is_live_append_only_jsonl, is_live_atomic_rewritable, is_live_snapshottable_metadata,
};
use crate::{
    ArchiveError, ArchiveMemberName, IncludedRootName, Inventory, InventoryEntry,
    OpenedInventoryFile, SkippedRootName,
};

// Streaming member reads do not allocate whole files in memory, so
// `maximum_checked_read_bytes` no longer bounds per-member archive size.
// `maximum_entries` (1_000_000, post-directory-prune) and
// `maximum_member_utf8_bytes` (ZIP u16::MAX) remain finite bounds alongside
// Windows' 32 KiB UTF-16 relative-path ceiling and depth 64 limit.
const ARCHIVE_INVENTORY_BUDGET: InventoryBudget = InventoryBudget::new(
    1_000_000,
    64,
    u16::MAX as usize,
    32 * 1024,
    64 * 1024 * 1024,
);

/// A frozen, capability-rooted portable archive source on Windows.
pub struct ArchiveSource {
    root: JournalRoot,
    inventory: Inventory,
    observed: Vec<WindowsInventoryEntry>,
    budget: InventoryBudget,
    snapshot_mode: SnapshotMode,
    live_snapshots: BTreeMap<String, Vec<u8>>,
}

impl ArchiveSource {
    /// Acquire `root` once and immediately freeze its portable archive inventory.
    pub fn open(root: &Path) -> Result<Self, ArchiveError> {
        Self::open_with_budget_and_mode(root, ARCHIVE_INVENTORY_BUDGET, SnapshotMode::Strict)
    }

    /// Acquire `root` for live-journal preparation on Windows.
    pub fn open_live(root: &Path) -> Result<Self, ArchiveError> {
        Self::open_with_budget_and_mode(root, ARCHIVE_INVENTORY_BUDGET, SnapshotMode::LiveJournal)
    }

    #[cfg(test)]
    fn open_with_budget(root: &Path, budget: InventoryBudget) -> Result<Self, ArchiveError> {
        Self::open_with_budget_and_mode(root, budget, SnapshotMode::Strict)
    }

    fn open_with_budget_and_mode(
        root: &Path,
        budget: InventoryBudget,
        snapshot_mode: SnapshotMode,
    ) -> Result<Self, ArchiveError> {
        let retained_root = JournalRoot::open(root).map_err(crate::error::map_root_error)?;
        let observed =
            enumerate_windows_inventory_pruned(&retained_root, budget, should_prune_directory)
                .map_err(|error| map_windows_error(&retained_root, error, false))?
                .into_entries();
        let (mut inventory, observed) = build_inventory(&observed)?;
        let mut live_snapshots = BTreeMap::new();
        if snapshot_mode == SnapshotMode::LiveJournal {
            let mut snapshotted_total = 0u64;
            for entry in &mut inventory.entries {
                let member = entry.member_name().as_str().to_owned();
                if !is_live_snapshottable_metadata(&member) {
                    continue;
                }
                if entry.size() > LIVE_METADATA_MAX_FILE_BYTES
                    || snapshotted_total.saturating_add(entry.size())
                        > LIVE_METADATA_TOTAL_BUDGET_BYTES
                {
                    continue;
                }
                let budget_left = LIVE_METADATA_MAX_FILE_BYTES
                    .min(LIVE_METADATA_TOTAL_BUDGET_BYTES.saturating_sub(snapshotted_total));
                let bytes = read_windows_inventory_live_snapshot(
                    &retained_root,
                    &entry.proof().file.observed,
                    budget_left,
                )
                .map_err(|error| map_windows_error(&retained_root, error, true))?;
                let len = bytes.len() as u64;
                entry.set_live_snapshot_size(len);
                snapshotted_total = snapshotted_total.saturating_add(len);
                live_snapshots.insert(member, bytes);
            }
        }
        Ok(Self {
            root: retained_root,
            inventory,
            observed,
            budget,
            snapshot_mode,
            live_snapshots,
        })
    }

    /// Return the snapshot concurrency mode for this source.
    pub fn snapshot_mode(&self) -> SnapshotMode {
        self.snapshot_mode
    }

    /// Return the inventory frozen when this source was opened.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// Return the verified canonical path acquired when this source was opened.
    pub fn canonical_source(&self) -> &Path {
        self.root.canonical_path()
    }

    /// Open a witnessed, bounded-memory streaming reader for one frozen regular archive member.
    pub fn open_file(
        &self,
        entry: &InventoryEntry,
    ) -> Result<OpenedInventoryFile<'_>, ArchiveError> {
        if let Some(bytes) = self.live_snapshots.get(entry.member_name().as_str()) {
            return Ok(OpenedInventoryFile::from_snapshot(bytes.as_slice()));
        }
        match self.snapshot_mode {
            SnapshotMode::Strict => {
                let reader =
                    open_windows_inventory_file_reader(&self.root, &entry.proof().file.observed)
                        .map_err(|error| map_windows_error(&self.root, error, true))?;
                Ok(OpenedInventoryFile::new(reader, &self.root, entry.size()))
            }
            SnapshotMode::LiveJournal => {
                let allow_atomic_replace = is_live_atomic_rewritable(entry.member_name().as_str());
                let allow_append_only = !allow_atomic_replace
                    && is_live_append_only_jsonl(entry.member_name().as_str());
                let reader = open_windows_inventory_file_reader_live(
                    &self.root,
                    &entry.proof().file.observed,
                    allow_append_only,
                    allow_atomic_replace,
                )
                .map_err(|error| map_windows_error(&self.root, error, true))?;
                Ok(OpenedInventoryFile::new_windows_live(
                    reader,
                    &self.root,
                    allow_append_only,
                ))
            }
        }
    }

    /// Confirm the complete frozen observed inventory remains unchanged.
    pub fn revalidate(&self) -> Result<(), ArchiveError> {
        match self.snapshot_mode {
            SnapshotMode::Strict => {
                let observed = enumerate_windows_inventory_pruned(
                    &self.root,
                    self.budget,
                    should_prune_directory,
                )
                .map_err(|error| map_windows_error(&self.root, error, true))?
                .into_entries();
                let (_, observed) = build_inventory(&observed)?;
                if observed != self.observed {
                    return Err(ArchiveError::SourceChanged { member: None });
                }
                Ok(())
            }
            SnapshotMode::LiveJournal => {
                self.root
                    .revalidate()
                    .map_err(crate::error::map_root_error)?;
                for observed in &self.observed {
                    if observed.kind() == JournalEntryKind::Directory {
                        revalidate_windows_live_entry(&self.root, observed, false, false)
                            .map_err(|error| map_windows_error(&self.root, error, true))?;
                    }
                }
                for entry in &self.inventory.entries {
                    if self
                        .live_snapshots
                        .contains_key(entry.member_name().as_str())
                    {
                        continue;
                    }
                    let allow_atomic_replace =
                        is_live_atomic_rewritable(entry.member_name().as_str());
                    let allow_append_only = !allow_atomic_replace
                        && is_live_append_only_jsonl(entry.member_name().as_str());
                    revalidate_windows_live_entry(
                        &self.root,
                        &entry.proof().file.observed,
                        allow_append_only,
                        allow_atomic_replace,
                    )
                    .map_err(|error| map_windows_error(&self.root, error, true))?;
                }
                Ok(())
            }
        }
    }
}

fn should_prune_directory(member: &str) -> bool {
    let top_level = !member.contains('/');
    (top_level && deny_top_level(member) == DenyAction::TreePrune) || deny_member(member, false)
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
                if deny_member(member.as_str(), false) {
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
                if deny_member(member.as_str(), true) {
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

pub(crate) fn map_windows_error(
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
    use std::io::Read;
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
        write(&root, "config/journal.json", b"settings");
        (temporary, root)
    }

    #[test]
    fn windows_export_preserves_owner_names_and_credentials() {
        let (_temporary, root) = fixture("owner-names");
        let kept = [
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
            "chronicle/20260101/indexer/120000_60/slides.key",
            "chronicle/20260101/source/120000_60/audio.jsonl.lock",
            "chronicle/20260101/source/120000_60/.tmp_123_4.tmp",
            "imports/item/peers/original.txt",
        ];
        for member in kept {
            write(&root, member, b"owner bytes");
        }
        let omitted = [
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
        ];
        for member in omitted {
            write(&root, member, b"secret control");
        }
        let new_kept = [
            (
                "config/journal.json",
                b"kept settings and provider credentials" as &[u8],
            ),
            ("config/chatgpt-sign-in.json", b"kept authorization"),
            ("backup/hosted/binding.json", b"kept binding"),
            ("config/push-vapid.json", b"kept vapid"),
            ("config/push-registry.json", b"kept push registry"),
            ("link/service-ca.key", b"kept pairing private key"),
            ("link/authorized_clients.json", b"kept client authorization"),
            ("backup/hosted/note.txt", b"kept hosted note" as &[u8]),
            ("config/schedules.json", b"kept schedules"),
            ("awareness/current.json", b"kept awareness"),
            ("link/client-descriptions.json", b"kept owner device labels"),
            ("tokens/20260101.jsonl", b"kept usage history"),
            ("apps/owner/data.json", b"kept app data"),
            ("solstone/apps/owner/data.json", b"kept legacy app data"),
        ];
        for (member, bytes) in new_kept {
            write(&root, member, bytes);
        }
        let new_omitted = [
            (
                "mcp-endpoint/local-door-state.json",
                b"omitted listener status" as &[u8],
            ),
            (
                "awareness/discovery_clusters.json",
                b"omitted clusters" as &[u8],
            ),
            ("mcp-endpoint/.create.lock", b"omitted lock"),
            (
                "mcp-endpoint/tls/.tmp_candidate",
                b"omitted temporary candidate",
            ),
        ];
        for (member, bytes) in new_omitted {
            write(&root, member, bytes);
        }
        let source = ArchiveSource::open(&root).unwrap();
        let mut output = std::fs::File::create(root.parent().unwrap().join("copy.zip")).unwrap();
        crate::encode_archive(
            &crate::EncodeArchiveRequest {
                source: &source,
                solstone_version: "2.0.0",
                exported_at: "2026-01-01T00:00:00Z",
                day_window: None,
            },
            &mut output,
        )
        .unwrap();
        assert_eq!(
            fs::read(root.join("peers/remote/client.key")).unwrap(),
            b"owner bytes"
        );
        assert_eq!(
            fs::read(root.join("peers/remote/client.crt")).unwrap(),
            b"owner bytes"
        );
        for member in kept {
            let entry = source
                .inventory()
                .entries()
                .iter()
                .find(|entry| entry.member_name().as_str() == member)
                .expect(member);
            assert_eq!(
                source.open_file(entry).unwrap().read_all_checked().unwrap(),
                b"owner bytes"
            );
        }
        for member in omitted {
            assert!(
                !source
                    .inventory()
                    .entries()
                    .iter()
                    .any(|entry| entry.member_name().as_str() == member),
                "{member}"
            );
        }
        let copy_zip_path = root.parent().unwrap().join("copy.zip");
        let mut archive = zip::ZipArchive::new(fs::File::open(&copy_zip_path).unwrap()).unwrap();
        for (member, expected_bytes) in new_kept {
            let mut file = archive.by_name(member).unwrap();
            let mut contents = Vec::new();
            file.read_to_end(&mut contents).unwrap();
            assert_eq!(
                &contents, expected_bytes,
                "zip content mismatch for {member}"
            );
            assert_eq!(
                fs::read(root.join(member)).unwrap(),
                expected_bytes,
                "source file changed on disk for {member}"
            );
        }
        for (member, expected_bytes) in new_omitted {
            assert!(
                archive.by_name(member).is_err(),
                "omitted member {member} found in zip"
            );
            assert_eq!(
                fs::read(root.join(member)).unwrap(),
                expected_bytes,
                "source file changed on disk for {member}"
            );
        }
    }

    #[test]
    fn windows_export_keeps_credentials_with_windows_casing() {
        let temporary = TempDir::new("reserved-path-casing");
        let root = temporary.path.join("journal");
        fs::create_dir(&root).unwrap();
        const LOG_LEAF: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";
        let log_path = format!("Chronicle/20260101/Health/{LOG_LEAF}");
        let jsonl_path = format!(
            "Chronicle/20260101/Health/{}",
            LOG_LEAF.replace(".log", ".jsonl")
        );
        let kept = [
            ("Config/Journal.JSON", b"owner credential" as &[u8]),
            ("Config/Push-Vapid.JSON", b"owner credential" as &[u8]),
            ("Config/Push-Registry.JSON", b"owner credential" as &[u8]),
            ("Config/ChatGPT-Sign-In.JSON", b"owner credential" as &[u8]),
            ("Link/Service-CA.KEY", b"owner credential" as &[u8]),
            ("Peers/remote/Client.KEY", b"owner credential" as &[u8]),
            ("MCP-Endpoint/POP.ed25519.pk8", b"owner credential" as &[u8]),
            ("Backup/Hosted/Binding.JSON", b"owner credential" as &[u8]),
            ("Link/Client-Descriptions.JSON", b"owner labels" as &[u8]),
            ("Config/Owner-Preferences.JSON", b"owner settings"),
            ("imports/item/Config/Push-Vapid.JSON", b"owner original"),
            (
                "imports/item/.REMOVING_owner-note",
                b"owner differently cased name",
            ),
            (jsonl_path.as_str(), b"owner operational history"),
            (
                "Chronicle/20260101/Health/oplog--not-a-name.log",
                b"owner malformed-name original",
            ),
        ];
        let omitted = [
            "MCP-Endpoint/byo/ingress.sock",
            "MCP-Endpoint/byo/cutover.sock",
            "MCP-Endpoint/LAN-Door-State.JSON",
            "MCP-Endpoint/.create.lock",
            "MCP-Endpoint/tls/.tmp_candidate",
            "MCP-Endpoint/nested/.removing_old/key",
            "Config/.tmp_fixture.tmp",
            log_path.as_str(),
        ];
        for (member, bytes) in kept {
            write(&root, member, bytes);
        }
        for member in omitted {
            write(&root, member, b"throwaway state");
        }
        let source = ArchiveSource::open(&root).unwrap();
        let path = temporary.path.join("cased.zip");
        let mut output = fs::File::create(&path).unwrap();
        crate::encode_archive(
            &crate::EncodeArchiveRequest {
                source: &source,
                solstone_version: "2.0.0",
                exported_at: "2026-01-01T00:00:00Z",
                day_window: None,
            },
            &mut output,
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        for (member, expected) in kept {
            let mut body = Vec::new();
            archive
                .by_name(member)
                .unwrap()
                .read_to_end(&mut body)
                .unwrap();
            assert_eq!(body, expected);
            assert_eq!(fs::read(root.join(member)).unwrap(), expected);
        }
        for member in omitted {
            assert!(
                archive.by_name(member).is_err(),
                "throwaway state included: {member}"
            );
            assert_eq!(fs::read(root.join(member)).unwrap(), b"throwaway state");
        }
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
                "config/journal.json",
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
            source
                .open_file(entry)
                .expect("checked read")
                .read_all_checked()
                .expect("complete checked read"),
            b"segment"
        );
        source.revalidate().expect("revalidate frozen source");
    }

    #[test]
    fn inventory_budget_refusals_do_not_construct_partial_results_and_streaming_ignores_read_cap() {
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
        let bytes = source
            .open_file(entry)
            .expect("streaming reader does not apply checked-read buffer cap")
            .read_all_checked()
            .expect("verified stream finishes");
        assert!(!bytes.is_empty());
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
        // Windows may prevent an ancestor rename while a descendant is retained.
        let active_outer = match fs::rename(&outer, &moved_outer) {
            Ok(()) => moved_outer,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                assert!(outer.is_dir());
                assert!(!moved_outer.exists());
                outer
            }
            Err(error) => panic!("rename ancestor: {error}"),
        };
        assert_eq!(
            source
                .open_file(&entry)
                .expect("read retained root")
                .read_all_checked()
                .expect("complete retained root read"),
            b"segment"
        );

        let original_child = active_outer.join("journal").join("chronicle");
        let displaced_child = active_outer.join("journal").join("chronicle-original");
        match fs::rename(&original_child, &displaced_child) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                assert!(original_child.is_dir());
                assert!(!displaced_child.exists());
                assert_eq!(
                    source
                        .open_file(&entry)
                        .expect("retained source")
                        .read_all_checked()
                        .expect("complete retained source read"),
                    b"segment"
                );
                return;
            }
            Err(error) => panic!("move observed route directory: {error}"),
        }
        write(
            &active_outer.join("journal"),
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
    fn witness_arm_failure_refuses_inventory_without_fallback() {
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
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn post_read_witness_refusal_releases_no_checked_bytes() {
        use solstone_core_journal_io::{
            WindowsInventoryPrimitive, run_with_windows_inventory_fault,
        };

        let (_temporary, root) = fixture("post-read-witness");
        let source = ArchiveSource::open(&root).expect("open ordinary source");
        let entry = source
            .inventory()
            .entries()
            .first()
            .expect("inventory entry");
        // `WindowsCheckedFileReader::finish` reaches its first `WitnessCheck` only
        // after reading the member and re-checking leaf metadata. Injecting this
        // failure therefore proves the streaming checked-read boundary returns an
        // error instead of releasing already-read bytes.
        let (result, consumed) =
            run_with_windows_inventory_fault(WindowsInventoryPrimitive::WitnessCheck, 1, 1, || {
                source
                    .open_file(entry)
                    .and_then(|file| file.read_all_checked())
            });
        assert!(consumed);
        assert!(matches!(
            result,
            Err(ArchiveError::UnsupportedJournal { .. })
        ));
    }

    #[test]
    fn pruned_directories_do_not_consume_inventory_entry_budget() {
        let (_temporary, root) = fixture("pruned-budget");
        for index in 0..40 {
            write(
                &root,
                &format!("indexer/shard-{index:02}.sqlite"),
                b"rebuildable",
            );
            write(
                &root,
                &format!("imports/archive-merge-work/extract-{index:02}.bin"),
                b"scratch",
            );
        }
        // Only the 4 fixture files + their parents + pruned roots/dirs are visited (~16 entries),
        // well below 30 even though 80 throwaway files sit inside pruned subtrees.
        let tight = InventoryBudget::new(30, 64, u16::MAX as usize, 32 * 1024, 64 * 1024 * 1024);
        let source =
            ArchiveSource::open_with_budget(&root, tight).expect("pruned trees do not count");
        assert!(
            source
                .inventory()
                .skipped_root_names()
                .iter()
                .any(|name| name.as_str() == "indexer")
        );
        source.revalidate().expect("revalidate keeps pruning");
    }

    #[test]
    fn member_paths_over_255_utf8_bytes_are_inventoried_and_exported() {
        let (_temporary, root) = fixture("long-member-path");
        let deep = format!(
            "imports/{}/{}/{}/{}/artifact.txt",
            "segment-a".repeat(8),
            "segment-b".repeat(8),
            "segment-c".repeat(8),
            "segment-d".repeat(8),
        );
        assert!(deep.len() > 255, "test path must exceed 255 bytes");
        write(&root, &deep, b"deep-member-bytes");
        let source = ArchiveSource::open(&root).expect("open source with >255-byte member path");
        let entry = source
            .inventory()
            .entries()
            .iter()
            .find(|entry| entry.member_name().as_str() == deep)
            .expect("deep member inventoried");
        assert_eq!(
            source
                .open_file(entry)
                .expect("open deep member")
                .read_all_checked()
                .expect("read deep member"),
            b"deep-member-bytes"
        );
    }
}
