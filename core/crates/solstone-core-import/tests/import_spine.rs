// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
#[cfg(all(unix, not(target_os = "macos")))]
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value, json};
use solstone_core_import::dedupe::{ManifestSkipReason, ManifestWriteRequest};
use solstone_core_import::staging::{
    SourceLocation, StageDisposition, StageRequest, classify_source_location,
};
use solstone_core_import::{
    AuditSinkError, ForceReimportAudit, ImportError, ImportForceEffects, RemovalError, SourceHash,
    find_manifest_by_hash, hash_source, observe_source_immutability, read_import_metadata,
    stage_source, windowed_source_hash, write_import_metadata, write_manifest,
};

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);
static CLEANUP_FAILURES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

#[test]
fn source_hash_matches_file_and_directory_oracles() {
    let tree = TempTree::new();
    let file = tree.path().join("file.txt");
    fs::write(&file, b"the owner's words\n").unwrap();
    assert_eq!(
        hash_source(&file).unwrap().as_str(),
        "db7360d06318e2d1f9148fb25cb2922faee93eb0954bee2a3c3600ce283a85f0"
    );

    let directory = tree.path().join("directory");
    fs::create_dir_all(directory.join("sub")).unwrap();
    fs::write(directory.join("a.md"), vec![b'a'; 8]).unwrap();
    fs::write(directory.join("sub/b.md"), vec![b'b'; 10]).unwrap();
    fs::write(directory.join("sub/c.bin"), vec![b'c'; 256]).unwrap();
    assert_eq!(
        hash_source(&directory).unwrap().as_str(),
        "4584c2bd1e437650ec375ae4d773ccb5114465bb0bb6b243bd04b8897c824c50"
    );

    let same_size = tree.path().join("same-size");
    fs::create_dir_all(&same_size).unwrap();
    fs::write(same_size.join("a.md"), vec![b'z'; 8]).unwrap();
    fs::create_dir_all(same_size.join("sub")).unwrap();
    fs::write(same_size.join("sub/b.md"), vec![b'y'; 10]).unwrap();
    fs::write(same_size.join("sub/c.bin"), vec![b'x'; 256]).unwrap();
    assert_eq!(
        hash_source(&same_size).unwrap(),
        hash_source(&directory).unwrap()
    );
}

#[test]
fn directory_hash_uses_component_tuple_order_and_dot_paths() {
    let tree = TempTree::new();
    let discriminating = tree.path().join("discriminating");
    fs::create_dir_all(discriminating.join("sub")).unwrap();
    fs::write(discriminating.join("sub.txt"), b"x").unwrap();
    fs::write(discriminating.join("sub/a.txt"), b"xy").unwrap();
    assert_eq!(
        hash_source(&discriminating).unwrap().as_str(),
        "b4c867b6d93347e772bb59b59be1619e56d895f251b573465b58457677ed572a"
    );

    let dots = tree.path().join("dots");
    fs::create_dir_all(dots.join(".dotdir")).unwrap();
    fs::create_dir_all(dots.join("sub")).unwrap();
    fs::write(dots.join(".dotdir/in.txt"), b"x").unwrap();
    fs::write(dots.join(".hidden"), b"xy").unwrap();
    fs::write(dots.join("a.md"), vec![b'a'; 8]).unwrap();
    fs::write(dots.join("sub/b.md"), vec![b'b'; 10]).unwrap();
    assert_eq!(
        hash_source(&dots).unwrap().as_str(),
        "2f7a064f786450f38df79cb8fff8f7a2fd31305a64030549adf6b85e9830a691"
    );
}

#[cfg(unix)]
#[test]
fn directory_hash_follows_file_links_but_not_directory_links() {
    use std::os::unix::fs::symlink;

    let tree = TempTree::new();
    let directory = tree.path().join("links");
    fs::create_dir_all(directory.join("real")).unwrap();
    fs::write(directory.join("f.md"), vec![b'f'; 5]).unwrap();
    fs::write(directory.join("real/big.bin"), vec![b'b'; 100]).unwrap();
    symlink("real/big.bin", directory.join("link_to_file")).unwrap();
    symlink("real", directory.join("link_to_dir")).unwrap();
    symlink("nope", directory.join("dangling")).unwrap();
    assert_eq!(
        hash_source(&directory).unwrap().as_str(),
        "4a1c562fade8961abbc2834ed9d1c01989b64ef28fcca998dd3b49b446d0ceb9"
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn directory_hash_refuses_the_first_non_utf8_relative_entry_deterministically() {
    use std::os::unix::ffi::OsStringExt;

    let tree = TempTree::new();
    let directory = tree.path().join("bad-names");
    fs::create_dir(&directory).unwrap();
    let first = directory.join(OsString::from_vec(vec![b'a', 0xff]));
    let second = directory.join(OsString::from_vec(vec![b'z', 0x80]));
    fs::write(&second, b"x").unwrap();
    fs::write(&first, b"x").unwrap();
    assert!(matches!(
        hash_source(&directory),
        Err(ImportError::NonUtf8DirectoryEntry { path }) if path == first
    ));

    let root_with_bad_name = tree.path().join(OsString::from_vec(vec![b'r', 0xff]));
    fs::create_dir(&root_with_bad_name).unwrap();
    fs::write(root_with_bad_name.join("good.txt"), b"x").unwrap();
    assert!(hash_source(&root_with_bad_name).is_ok());
}

#[test]
fn directory_hash_accepts_utf8_names_and_is_deterministic_by_name_and_size() {
    let tree = TempTree::new();
    let directory = tree.path().join("utf8-names");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("café.md"), b"x").unwrap();
    let original = hash_source(&directory).unwrap();
    assert_eq!(hash_source(&directory).unwrap(), original);

    fs::rename(directory.join("café.md"), directory.join("cafe.md")).unwrap();
    assert_ne!(hash_source(&directory).unwrap(), original);
    fs::write(directory.join("cafe.md"), b"xy").unwrap();
    assert_ne!(hash_source(&directory).unwrap(), original);
}

#[test]
fn empty_sources_hash_empty_but_missing_source_is_typed_error() {
    let tree = TempTree::new();
    let file = tree.path().join("empty");
    let directory = tree.path().join("empty-dir");
    fs::write(&file, b"").unwrap();
    fs::create_dir(&directory).unwrap();
    let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(hash_source(&file).unwrap().as_str(), empty);
    assert_eq!(hash_source(&directory).unwrap().as_str(), empty);
    assert!(matches!(
        hash_source(&tree.path().join("missing")),
        Err(ImportError::SourceMissing { .. })
    ));
}

#[test]
fn windowed_hash_preserves_open_bounds() {
    let tree = TempTree::new();
    let file = tree.path().join("hello");
    fs::write(&file, b"hello").unwrap();
    assert_eq!(
        windowed_source_hash(&file, Some("2026-01-01"), None)
            .unwrap()
            .as_str(),
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824#window:20260101:open"
    );
}

#[cfg(unix)]
#[test]
fn containment_preserves_the_original_alias_until_resolution() {
    use std::os::unix::fs::symlink;

    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let staged = journal.join("imports/item/source.txt");
    fs::create_dir_all(staged.parent().unwrap()).unwrap();
    fs::write(&staged, b"staged").unwrap();
    let alias = tree.path().join("cloud-alias");
    symlink(journal.join("imports"), &alias).unwrap();
    let original = alias.join("item/source.txt");
    assert_ne!(original, staged);
    assert!(matches!(
        classify_source_location(&journal, &original).unwrap(),
        SourceLocation::AlreadyInImports { source } if source == staged
    ));
}

#[test]
fn containment_does_not_confuse_an_imports_prefix_sibling_for_imports() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let sibling_source = journal.join("imports-backup/source.md");
    fs::create_dir_all(sibling_source.parent().unwrap()).unwrap();
    fs::write(&sibling_source, b"owner bytes").unwrap();

    assert!(matches!(
        classify_source_location(&journal, &sibling_source).unwrap(),
        SourceLocation::External { .. }
    ));
}

#[test]
fn staging_is_create_only_and_preserves_the_source() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let source = tree.path().join("owner.txt");
    fs::write(&source, b"owner bytes").unwrap();
    let metadata = metadata("source-a");
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("owner.txt"),
        &metadata,
        false,
        false,
    );
    let outcome = stage_source(&request, &NoopEffects).unwrap();
    assert_eq!(outcome.disposition, StageDisposition::Staged);
    assert_eq!(fs::read(&source).unwrap(), b"owner bytes");
    assert_eq!(fs::read(&outcome.path).unwrap(), b"owner bytes");
    assert!(matches!(
        stage_source(&request, &NoopEffects),
        Err(ImportError::ExistingImportDirectory { .. })
    ));
}

#[test]
fn staging_rejects_a_destination_name_that_escapes_the_import_directory() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let source = tree.path().join("owner.txt");
    fs::write(&source, b"owner bytes").unwrap();
    let metadata = metadata("source-a");
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("../outside.txt"),
        &metadata,
        false,
        false,
    );
    assert!(matches!(
        stage_source(&request, &NoopEffects),
        Err(ImportError::InvalidDestinationName { .. })
    ));
    assert!(!journal.join("outside.txt").exists());
}

#[test]
fn metadata_round_trip_keeps_known_and_unknown_fields_in_order() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let mut record = metadata("source-a");
    record.insert("unknown".to_owned(), json!({"nested": [true, null]}));
    write_import_metadata(&journal, "item", &record).unwrap();
    let loaded = read_import_metadata(&journal, "item").unwrap();
    assert_eq!(loaded, record);
    assert_eq!(
        loaded.keys().collect::<Vec<_>>(),
        record.keys().collect::<Vec<_>>()
    );
    fs::write(journal.join("imports/item/import.json"), b"[]").unwrap();
    assert!(matches!(
        read_import_metadata(&journal, "item"),
        Err(ImportError::MetadataCorrupt { .. })
    ));
}

#[test]
fn force_orders_audit_before_removal_and_preserves_failures() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let source = tree.path().join("replacement.txt");
    fs::write(&source, b"replacement").unwrap();
    let metadata = metadata("source-a");
    let import_dir = journal.join("imports/item");
    fs::create_dir_all(&import_dir).unwrap();
    fs::write(import_dir.join("old.txt"), b"old").unwrap();
    write_import_metadata(&journal, "item", &metadata).unwrap();

    let effects = RecordingEffects::new(&journal, FailureStage::None);
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("replacement.txt"),
        &metadata,
        true,
        false,
    );
    let outcome = stage_source(&request, &effects).unwrap();
    assert_eq!(effects.events(), vec!["audit", "remove"]);
    assert!(outcome.force_audit_recorded);
    assert_eq!(fs::read(outcome.path).unwrap(), b"replacement");

    let audit_failure = RecordingEffects::new(&journal, FailureStage::Audit);
    assert!(matches!(
        stage_source(&request, &audit_failure),
        Err(ImportError::AuditSinkFailed { .. })
    ));
    assert_eq!(audit_failure.events(), vec!["audit"]);

    let removal_failure = RecordingEffects::new(&journal, FailureStage::Removal);
    assert!(matches!(
        stage_source(&request, &removal_failure),
        Err(ImportError::RemovalFailed { .. })
    ));
    assert_eq!(removal_failure.events(), vec!["audit", "remove"]);
}

#[test]
fn non_force_dry_run_creates_nothing_and_a_real_retry_stages() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let source = tree.path().join("owner.txt");
    fs::write(&source, b"owner bytes").unwrap();
    let metadata = metadata("source-a");
    let preview_request = request(
        &journal,
        "item",
        &source,
        OsStr::new("owner.txt"),
        &metadata,
        false,
        true,
    );

    let preview = stage_source(&preview_request, &NoopEffects).unwrap();
    assert_eq!(preview.disposition, StageDisposition::Preview);
    assert!(!journal.join("imports").exists());

    let stage_request = request(
        &journal,
        "item",
        &source,
        OsStr::new("owner.txt"),
        &metadata,
        false,
        false,
    );
    assert_eq!(
        stage_source(&stage_request, &NoopEffects)
            .unwrap()
            .disposition,
        StageDisposition::Staged
    );
}

#[cfg(unix)]
#[test]
fn force_allows_a_relocated_imports_root() {
    use std::os::unix::fs::symlink;

    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let relocated_imports = tree.path().join("external-imports");
    let source = tree.path().join("replacement.txt");
    fs::create_dir(&journal).unwrap();
    fs::create_dir_all(relocated_imports.join("item")).unwrap();
    fs::write(relocated_imports.join("item/old.txt"), b"old").unwrap();
    symlink(&relocated_imports, journal.join("imports")).unwrap();
    fs::write(&source, b"replacement").unwrap();
    let metadata = metadata("source-a");
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("replacement.txt"),
        &metadata,
        true,
        false,
    );
    let effects = RecordingEffects::new(&journal, FailureStage::None);

    let outcome = stage_source(&request, &effects).unwrap();
    assert!(outcome.force_audit_recorded);
    assert_eq!(effects.events(), vec!["audit", "remove"]);
    assert_eq!(
        fs::read(relocated_imports.join("item/replacement.txt")).unwrap(),
        b"replacement"
    );
}

#[cfg(unix)]
#[test]
fn force_without_metadata_audits_file_links_in_component_order() {
    use std::os::unix::fs::symlink;

    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let import_dir = journal.join("imports/item");
    let source = tree.path().join("replacement.txt");
    fs::create_dir_all(import_dir.join("real")).unwrap();
    fs::create_dir_all(import_dir.join("sub")).unwrap();
    fs::write(import_dir.join("real/payload.bin"), b"abc").unwrap();
    fs::write(import_dir.join("sub/a.txt"), b"xy").unwrap();
    fs::write(import_dir.join("sub.txt"), b"x").unwrap();
    symlink("real/payload.bin", import_dir.join("link_to_file")).unwrap();
    fs::write(&source, b"replacement").unwrap();
    let metadata = metadata("source-a");
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("replacement.txt"),
        &metadata,
        true,
        false,
    );
    let effects = RecordingEffects::new(&journal, FailureStage::None);

    stage_source(&request, &effects).unwrap();
    let inventories = effects.inventories();
    let files = inventories[0]["files"].as_array().unwrap();
    let names = files
        .iter()
        .map(|file| file["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["link_to_file", "real/payload.bin", "sub/a.txt", "sub.txt"]
    );
    assert_eq!(files[0]["bytes"], json!(3));
    assert!(files.iter().all(|file| file["hash"].is_string()));
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn force_inventory_refuses_a_non_utf8_entry_without_removing_it() {
    use std::os::unix::ffi::OsStringExt;

    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let import_dir = journal.join("imports/item");
    let source = tree.path().join("replacement.txt");
    let invalid = import_dir.join(OsString::from_vec(vec![b'a', 0xff]));
    fs::create_dir_all(&import_dir).unwrap();
    fs::write(&invalid, b"old").unwrap();
    fs::write(&source, b"replacement").unwrap();
    let metadata = metadata("source-a");
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("replacement.txt"),
        &metadata,
        true,
        false,
    );
    let effects = RecordingEffects::new(&journal, FailureStage::None);

    assert!(matches!(
        stage_source(&request, &effects),
        Err(ImportError::NonUtf8DirectoryEntry { path }) if path == invalid
    ));
    assert!(effects.events().is_empty());
    assert_eq!(fs::read(invalid).unwrap(), b"old");
}

#[test]
fn forced_staging_leaves_the_owner_source_tree_unchanged() {
    let tree = TempTree::new();
    let owner_root = tree.path().join("owner");
    let journal = tree.path().join("journal");
    let source = owner_root.join("recording.txt");
    fs::create_dir(&owner_root).unwrap();
    fs::write(&source, b"owner bytes").unwrap();
    let metadata = metadata("source-a");
    fs::create_dir_all(journal.join("imports/item")).unwrap();
    fs::write(journal.join("imports/item/old.txt"), b"old").unwrap();
    write_import_metadata(&journal, "item", &metadata).unwrap();
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("recording.txt"),
        &metadata,
        true,
        false,
    );
    let effects = RecordingEffects::new(&journal, FailureStage::None);

    let report = observe_source_immutability(&owner_root, |_| {
        stage_source(&request, &effects).unwrap();
    })
    .unwrap();
    assert!(!report.violated());
}

#[test]
fn force_dry_run_audits_without_removing() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let source = tree.path().join("replacement.txt");
    fs::write(&source, b"replacement").unwrap();
    let metadata = metadata("source-a");
    fs::create_dir_all(journal.join("imports/item")).unwrap();
    fs::write(journal.join("imports/item/old.txt"), b"old").unwrap();
    write_import_metadata(&journal, "item", &metadata).unwrap();
    let effects = RecordingEffects::new(&journal, FailureStage::None);
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("replacement.txt"),
        &metadata,
        true,
        true,
    );
    let outcome = stage_source(&request, &effects).unwrap();
    assert_eq!(outcome.disposition, StageDisposition::Preview);
    assert!(outcome.force_audit_recorded);
    assert_eq!(effects.events(), vec!["audit"]);
    assert_eq!(
        fs::read(journal.join("imports/item/old.txt")).unwrap(),
        b"old"
    );
    assert_eq!(effects.dry_run_values(), vec![true]);
}

#[test]
fn force_refuses_metadata_mismatch_even_during_dry_run() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let source = tree.path().join("replacement.txt");
    fs::write(&source, b"replacement").unwrap();
    fs::create_dir_all(journal.join("imports/item")).unwrap();
    let existing = metadata("source-a");
    write_import_metadata(&journal, "item", &existing).unwrap();
    let replacement = metadata("source-b");
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("replacement.txt"),
        &replacement,
        true,
        true,
    );
    let effects = RecordingEffects::new(&journal, FailureStage::None);
    assert!(matches!(
        stage_source(&request, &effects),
        Err(ImportError::MetadataMismatchOnForce {
            key: "source_hash",
            ..
        })
    ));
    assert!(effects.events().is_empty());
}

#[cfg(unix)]
#[test]
fn force_refuses_a_symlinked_import_directory_before_the_audit() {
    use std::os::unix::fs::symlink;

    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let source = tree.path().join("replacement.txt");
    let target = tree.path().join("target");
    fs::write(&source, b"replacement").unwrap();
    fs::create_dir(&target).unwrap();
    fs::create_dir_all(journal.join("imports")).unwrap();
    symlink(&target, journal.join("imports/item")).unwrap();
    let metadata = metadata("source-a");
    let request = request(
        &journal,
        "item",
        &source,
        OsStr::new("replacement.txt"),
        &metadata,
        true,
        false,
    );
    let effects = RecordingEffects::new(&journal, FailureStage::None);
    assert!(matches!(
        stage_source(&request, &effects),
        Err(ImportError::ImportDirectoryIsSymlink { .. })
    ));
    assert!(effects.events().is_empty());
    assert!(target.is_dir());
}

#[test]
fn manifest_scan_reports_skipped_corruption() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let hash = SourceHash::new("source-a".to_owned());
    let request = ManifestWriteRequest {
        journal_root: &journal,
        import_id: "good",
        source_type: "audio",
        source_hash: &hash,
        entry_count: 3,
        days_affected: &[],
        files_created: &[],
        imported_via: "cli",
        link_id: None,
        observer_handle: None,
        raw_retention: Some("delete_after_processing"),
    };
    write_manifest(&request).unwrap();
    fs::create_dir_all(journal.join("imports/bad")).unwrap();
    fs::write(journal.join("imports/bad/manifest.json"), b"[]").unwrap();
    let scan = find_manifest_by_hash(&journal, &hash).unwrap();
    assert!(scan.found.is_some());
    assert_eq!(scan.skipped.len(), 1);
    assert_eq!(scan.skipped[0].reason, ManifestSkipReason::WrongJsonShape);
}

#[test]
fn facets_adapter_writes_a_durable_force_audit_record() {
    let tree = TempTree::new();
    let journal = tree.path().join("journal");
    let effects = FacetEffects {
        journal: journal.clone(),
    };
    effects
        .append_force_reimport(&ForceReimportAudit {
            import_dir: journal.join("imports/item"),
            inventory: json!({"files": []}),
            days_affected: vec!["20260101".to_owned()],
            dry_run: true,
        })
        .unwrap();
    let actions = journal.join("config/actions");
    let action = fs::read_dir(actions)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let content = fs::read_to_string(action).unwrap();
    let record: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(record["action"], "import_force_reimport");
    assert_eq!(record["params"]["dry_run"], true);
}

fn metadata(source_hash: &str) -> Map<String, Value> {
    let mut record = Map::new();
    record.insert("client_item_id".to_owned(), json!("client"));
    record.insert("source_hash".to_owned(), json!(source_hash));
    record.insert("task_id".to_owned(), json!("task"));
    record
}

fn request<'a>(
    journal: &'a Path,
    import_id: &'a str,
    source: &'a Path,
    destination_name: &'a OsStr,
    metadata: &'a Map<String, Value>,
    force: bool,
    dry_run: bool,
) -> StageRequest<'a> {
    StageRequest {
        journal_root: journal,
        import_id,
        source,
        destination_name,
        metadata,
        force,
        dry_run,
        days_affected: &["20260101"],
    }
}

struct NoopEffects;

impl ImportForceEffects for NoopEffects {
    fn append_force_reimport(&self, _audit: &ForceReimportAudit) -> Result<(), AuditSinkError> {
        Ok(())
    }

    fn remove_import_directory(&self, _import_dir: &Path) -> Result<(), RemovalError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum FailureStage {
    None,
    Audit,
    Removal,
}

struct RecordingEffects {
    journal: PathBuf,
    failure: FailureStage,
    events: Mutex<Vec<&'static str>>,
    dry_runs: Mutex<Vec<bool>>,
    inventories: Mutex<Vec<Value>>,
}

impl RecordingEffects {
    fn new(journal: &Path, failure: FailureStage) -> Self {
        Self {
            journal: journal.to_path_buf(),
            failure,
            events: Mutex::new(Vec::new()),
            dry_runs: Mutex::new(Vec::new()),
            inventories: Mutex::new(Vec::new()),
        }
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }

    fn dry_run_values(&self) -> Vec<bool> {
        self.dry_runs.lock().unwrap().clone()
    }

    fn inventories(&self) -> Vec<Value> {
        self.inventories.lock().unwrap().clone()
    }
}

impl ImportForceEffects for RecordingEffects {
    fn append_force_reimport(&self, audit: &ForceReimportAudit) -> Result<(), AuditSinkError> {
        self.events.lock().unwrap().push("audit");
        self.dry_runs.lock().unwrap().push(audit.dry_run);
        self.inventories
            .lock()
            .unwrap()
            .push(audit.inventory.clone());
        if matches!(self.failure, FailureStage::Audit) {
            return Err(AuditSinkError {
                message: "injected audit failure".to_owned(),
            });
        }
        Ok(())
    }

    fn remove_import_directory(&self, import_dir: &Path) -> Result<(), RemovalError> {
        self.events.lock().unwrap().push("remove");
        if matches!(self.failure, FailureStage::Removal) {
            return Err(RemovalError {
                message: "injected removal failure".to_owned(),
            });
        }
        let import_id = import_dir.file_name().unwrap().to_str().unwrap();
        solstone_core_journal_io::remove_dir_all(&self.journal.join("imports"), import_id).map_err(
            |error| RemovalError {
                message: error.to_string(),
            },
        )
    }
}

struct FacetEffects {
    journal: PathBuf,
}

impl ImportForceEffects for FacetEffects {
    fn append_force_reimport(&self, audit: &ForceReimportAudit) -> Result<(), AuditSinkError> {
        solstone_core_facets::append_action_log(
            &self.journal,
            None,
            "import",
            "import",
            "import_force_reimport",
            audit.params(),
        )
        .map_err(|error| AuditSinkError {
            message: error.to_string(),
        })
    }

    fn remove_import_directory(&self, import_dir: &Path) -> Result<(), RemovalError> {
        let import_id = import_dir.file_name().unwrap().to_str().unwrap();
        solstone_core_journal_io::remove_dir_all(&self.journal.join("imports"), import_id).map_err(
            |error| RemovalError {
                message: error.to_string(),
            },
        )
    }
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let index = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-import-spine-{}-{index}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        if fs::remove_dir_all(&self.path).is_err() {
            CLEANUP_FAILURES.lock().unwrap().push(self.path.clone());
        }
    }
}
