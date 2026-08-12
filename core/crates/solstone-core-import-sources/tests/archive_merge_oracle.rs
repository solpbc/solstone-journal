// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use solstone_core_entity::{load_all_journal_entities, save_entity_identity};
use solstone_core_import_sources::archive::{
    ArchiveMergeOptions, FullReindexRequester, PrincipalAdoption, ReindexStatus, RetryDisposition,
    merge_journal_archive,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const ORACLE: &str = include_str!("../../../fixtures/journal_archive_merge_oracle.json");
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct RejectReindex;
impl FullReindexRequester for RejectReindex {
    fn request_full_reindex(&self) -> Result<bool, String> {
        Ok(false)
    }
}

#[test]
fn native_archive_merge_matches_the_captured_verb_oracle_and_reports_principal_refusal() {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let tree = TempTree::new();
    let target = tree.path.join("target");
    let source = tree.path.join("source");
    save_entity_identity(
        &target,
        "john-smith-owner",
        &oracle["owner_entity_before"],
        None,
    )
    .unwrap();
    save_entity_identity(&source, "john-smith", &oracle["archive_entity"], None).unwrap();
    fs::create_dir_all(source.join("chronicle/20260811/120000_60")).unwrap();
    fs::write(
        source.join("chronicle/20260811/120000_60/segment.json"),
        b"{}\n",
    )
    .unwrap();
    let archive = tree.path.join("archive.zip");
    zip_tree(&source, &archive);
    let options = ArchiveMergeOptions {
        working_root: tree.path.join("work"),
        ..ArchiveMergeOptions::default()
    };
    let result = merge_journal_archive(&archive, &target, &options, Some(&RejectReindex)).unwrap();

    let owner = load_all_journal_entities(&target)
        .unwrap()
        .into_iter()
        .find(|entity| entity.id == "john-smith-owner")
        .unwrap()
        .value;
    assert_eq!(owner, oracle["owner_entity_after"]);
    assert_eq!(result.principal_collision, None);
    assert!(result.errors.is_empty());
    assert_eq!(
        result.entries_written,
        oracle["result"]["entries_written"].as_u64().unwrap() as usize
    );
    assert_eq!(result.merge_summary.entities_merged, 1);
    assert_eq!(
        result.entity_dispositions[0].principal_adoption,
        PrincipalAdoption::RefusedOnNameMatch
    );
    assert_eq!(
        result.reindex_status,
        ReindexStatus::NotAccepted {
            detail: "request was not accepted".to_owned()
        }
    );
    assert_eq!(result.retry_disposition, RetryDisposition::Incomplete);
}

fn zip_tree(root: &Path, archive: &Path) {
    let mut writer = ZipWriter::new(File::create(archive).unwrap());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    fn add(writer: &mut ZipWriter<File>, root: &Path, current: &Path, options: SimpleFileOptions) {
        let mut entries = fs::read_dir(current)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                add(writer, root, &path, options);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                writer.start_file(relative, options).unwrap();
                writer.write_all(&fs::read(path).unwrap()).unwrap();
            }
        }
    }
    add(&mut writer, root, root, options);
    writer.finish().unwrap();
}

struct TempTree {
    path: PathBuf,
}
impl TempTree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-archive-oracle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }
}
impl Drop for TempTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
