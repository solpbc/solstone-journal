// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Worker checks for admitted folder sources. They stay inside this crate's library tests.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use chrono_tz::Tz;
use solstone_core_import::audio_archive::{
    AudioArchiveSize, AudioArchiveStatus, DurationInput, SourceRejection,
};
use solstone_core_import::contract::SyncPreviewRequest;
use solstone_core_import::sync_plaud::{
    ImportPipeline, PipelineImportRequest, PipelineOutcome, PlaudCatalogue, PlaudCredential,
    PlaudDownload, PlaudFailureKind, PlaudFile, PlaudManifestLookup, PlaudPreviewSeams,
    PlaudStateWriter, PlaudSyncRequest, SyncClock, sync_plaud_preview,
};
use solstone_core_import::{
    BackendName, SyncState, SyncStateRead, admit_folder, begin_folder, hash_source,
    read_sync_state, reauthorize, replace_folder, validate_timestamp, write_sync_state,
};
use solstone_core_import_sources::document::{self, DocumentPreviewRequest};
use solstone_core_import_sources::gemini;
use solstone_core_import_sources::obsidian;
use solstone_core_import_sources::{NullPdfWorker, SkipReason};

use crate::audio_archive::{AudioArchiveDispatchOptions, read_manifest_members, run_audio_archive};
use crate::call::{self, ImportSeams};
use crate::registry::IndexScan;

struct IdleText;

impl solstone_core_import::text::WireClient for IdleText {
    fn execute(
        &self,
        _request: &solstone_core_generate::GenerateRequest,
    ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError> {
        Ok(solstone_core_generate::GenerateResponse::Cancelled)
    }
}

fn idle_seams(journal: &Path) -> ImportSeams {
    ImportSeams {
        text: Box::new(IdleText),
        image: Box::new(solstone_core_import_sources::NullWireClient),
        model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
        pdf: Box::new(NullPdfWorker),
        reindex: Box::new(IndexScan {
            journal: journal.to_path_buf(),
        }),
    }
}

fn files_of(state: &SyncState) -> &serde_json::Map<String, serde_json::Value> {
    state
        .root()
        .get("files")
        .and_then(serde_json::Value::as_object)
        .expect("sync files")
}

fn load(journal: &Path, backend: BackendName) -> SyncState {
    match read_sync_state(journal, backend) {
        SyncStateRead::Loaded(state) => state,
        other => panic!("sync state was not loaded: {other:?}"),
    }
}

fn file_stamp(path: &Path) -> (u64, Option<SystemTime>) {
    let meta = fs::symlink_metadata(path).unwrap();
    (meta.len(), meta.modified().ok())
}

#[test]
fn obsidian_notes_keep_bytes_and_a_changed_twin_is_a_new_snapshot() {
    let vault = tempfile::tempdir().unwrap();
    fs::create_dir(vault.path().join(".obsidian")).unwrap();
    fs::write(vault.path().join("keep.md"), b"keep note\n").unwrap();
    fs::write(vault.path().join("edit.md"), b"edit note\n").unwrap();
    let notes = obsidian::collect_notes(vault.path()).unwrap();
    assert_eq!(notes.len(), 2);
    let keep = notes
        .iter()
        .find(|note| note.source_path.ends_with("keep.md"))
        .unwrap();
    assert_eq!(keep.content, "keep note\n");
    assert_eq!(obsidian::collect_notes(vault.path()).unwrap().len(), 2);

    let twin = tempfile::tempdir().unwrap();
    fs::create_dir(twin.path().join(".obsidian")).unwrap();
    fs::write(twin.path().join("keep.md"), b"KEEP NOTE\n").unwrap();
    fs::write(twin.path().join("edit.md"), b"EDIT NOTE\n").unwrap();
    assert_eq!(
        hash_source(vault.path()).unwrap(),
        hash_source(twin.path()).unwrap()
    );
    assert_ne!(
        solstone_core_import::snapshot::fingerprint(vault.path()).unwrap(),
        solstone_core_import::snapshot::fingerprint(twin.path()).unwrap()
    );
    let twin_keep = obsidian::collect_notes(twin.path())
        .unwrap()
        .into_iter()
        .find(|note| note.source_path.ends_with("keep.md"))
        .unwrap();
    assert_eq!(twin_keep.content, "KEEP NOTE\n");
}

#[test]
fn obsidian_sync_observes_a_change_without_duplicating_keys() {
    let journal = tempfile::tempdir().unwrap();
    let vault = tempfile::tempdir().unwrap();
    fs::create_dir(vault.path().join(".obsidian")).unwrap();
    fs::write(vault.path().join("keep.md"), b"keep note\n").unwrap();
    fs::write(vault.path().join("edit.md"), b"edit note\n").unwrap();
    let mut created = SyncState::empty(BackendName::Obsidian);
    let identity = begin_folder(&mut created, "Vault", true, Some("grant-material")).unwrap();
    write_sync_state(journal.path(), &created).unwrap();

    let args = vec![
        "--sync".to_owned(),
        "obsidian".to_owned(),
        "--path".to_owned(),
        vault.path().display().to_string(),
    ];
    let preview = call::preview(&args, journal.path(), &idle_seams(journal.path()));
    assert_eq!(preview.exit_code, 0, "{}", preview.stderr);
    let started = call::start(
        args.clone(),
        journal.path().to_path_buf(),
        idle_seams(journal.path()),
        solstone_core_import::WorkStop::new(),
    )
    .unwrap();
    let (started, _) = started.join();
    assert_eq!(started.exit_code, 0, "{}", started.stderr);

    let first = load(journal.path(), BackendName::Obsidian);
    assert_eq!(files_of(&first).len(), 2);
    let keep_hash = files_of(&first)["keep.md"]["content_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let edit_hash = files_of(&first)["edit.md"]["content_hash"]
        .as_str()
        .unwrap()
        .to_owned();

    fs::write(vault.path().join("edit.md"), b"edit note!\n").unwrap();
    let again = call::preview(&args, journal.path(), &idle_seams(journal.path()));
    assert_eq!(again.exit_code, 0, "{}", again.stderr);
    let second = load(journal.path(), BackendName::Obsidian);
    assert_eq!(files_of(&second).len(), 2);
    assert_eq!(
        files_of(&second)["keep.md"]["content_hash"].as_str(),
        Some(keep_hash.as_str())
    );
    assert_ne!(
        files_of(&second)["edit.md"]["content_hash"].as_str(),
        Some(edit_hash.as_str())
    );

    let mut kept = second.clone();
    reauthorize(&mut kept, "Vault", true, Some("grant-material-2")).unwrap();
    assert_eq!(
        kept.root()
            .get("source_identity")
            .and_then(serde_json::Value::as_str),
        Some(identity.as_str())
    );
    assert_eq!(files_of(&kept).len(), 2);
    assert_eq!(
        files_of(&kept)["keep.md"]["content_hash"].as_str(),
        Some(keep_hash.as_str())
    );
    let replaced = replace_folder(&mut kept, "Vault", false, None).unwrap();
    assert_ne!(replaced, identity);
    assert!(files_of(&kept).is_empty());
}

#[test]
fn obsidian_force_reads_the_new_note_and_leaves_the_admitted_bytes() {
    let journal = tempfile::tempdir().unwrap();
    let vault = tempfile::tempdir().unwrap();
    fs::create_dir(vault.path().join(".obsidian")).unwrap();
    fs::write(vault.path().join("note.md"), b"original-bytes").unwrap();
    let admitted = admit_folder(
        journal.path(),
        vault.path(),
        "vault-attempt",
        &validate_timestamp("20260616_120000").unwrap(),
    )
    .unwrap();
    let admitted_note = journal
        .path()
        .join("imports")
        .join(admitted.import_id.as_str())
        .join("tree")
        .join("note.md");
    let original = fs::read(&admitted_note).unwrap();
    fs::write(vault.path().join("note.md"), b"changed-bytes!").unwrap();
    let args = vec![
        "--sync".to_owned(),
        "obsidian".to_owned(),
        "--path".to_owned(),
        vault.path().display().to_string(),
        "--force".to_owned(),
    ];
    let preview = call::preview(&args, journal.path(), &idle_seams(journal.path()));
    assert_eq!(preview.exit_code, 0, "{}", preview.stderr);
    let state = load(journal.path(), BackendName::Obsidian);
    let expected = hash_source(&vault.path().join("note.md"))
        .unwrap()
        .into_inner();
    assert_eq!(
        files_of(&state)["note.md"]["content_hash"].as_str(),
        Some(expected.as_str())
    );
    assert_eq!(fs::read(admitted_note).unwrap(), original);
}

#[test]
fn gemini_plan_is_write_free_and_does_not_invent_a_missing_time() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("MyActivity.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!([
            {
                "header": "Gemini Apps",
                "title": "Prompted hello",
                "time": "2026-06-16T12:00:00.000Z",
                "subtitles": [{"value": "hello folder"}],
                "safeHtmlItem": [{"html": "<p>answer</p>"}]
            },
            {
                "header": "Gemini Apps",
                "title": "Prompted later",
                "subtitles": [{"value": "untimed folder"}],
                "safeHtmlItem": [{"html": "<p>no time</p>"}]
            }
        ]))
        .unwrap(),
    )
    .unwrap();
    let before = file_stamp(&path);
    let plan = gemini::plan(root.path(), &Tz::UTC).unwrap();
    assert_eq!(file_stamp(&path), before);
    assert!(plan.item_count > 0);
    assert!(
        plan.skipped
            .iter()
            .any(|entry| { entry.reason == SkipReason::MissingActivityTimestamp })
    );
    let planned = plan
        .segments
        .iter()
        .flat_map(|segment| segment.entries.iter())
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>();
    assert!(planned.iter().any(|text| text.contains("hello folder")));
    assert!(planned.iter().all(|text| !text.contains("untimed folder")));
}

#[test]
fn pdf_preview_counts_immediate_children_only() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.pdf"), b"%PDF-1.1\n").unwrap();
    fs::create_dir(root.path().join("nested")).unwrap();
    fs::write(root.path().join("nested").join("b.pdf"), b"%PDF-1.1\n").unwrap();
    let only_nested = tempfile::tempdir().unwrap();
    fs::create_dir(only_nested.path().join("nested")).unwrap();
    fs::write(
        only_nested.path().join("nested").join("c.pdf"),
        b"%PDF-1.1\n",
    )
    .unwrap();
    assert_eq!(pdf_items(root.path()), 1);
    assert_eq!(pdf_items(only_nested.path()), 0);
}

fn pdf_items(source: &Path) -> u64 {
    document::preview(
        DocumentPreviewRequest {
            source,
            password: None,
            now: SystemTime::UNIX_EPOCH,
            zone: Tz::UTC,
            owner_timestamp: None,
        },
        &NullPdfWorker,
    )
    .item_count
}

#[test]
fn audio_archive_dry_run_is_not_the_size_schema_and_saved_stays_distinct() {
    let missing = tempfile::tempdir().unwrap();
    fs::write(
        missing.path().join("manifest.json"),
        br#"{"members":[{"path":"missing.opus","original_relative_name":"missing.opus"}]}"#,
    )
    .unwrap();
    let members = read_manifest_members(
        &missing.path().join("manifest.json"),
        missing.path(),
        Some("UTC"),
    )
    .unwrap();
    assert!(matches!(
        members[0].duration,
        DurationInput::Rejected(SourceRejection::Unreadable)
    ));

    let journal = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    fs::write(folder.path().join("clip.wav"), b"not-an-opus").unwrap();
    let dry = run_audio_archive(
        AudioArchiveDispatchOptions {
            path: folder.path().to_path_buf(),
            source_zone: Some("UTC".to_owned()),
            resume: None,
            dry_run: true,
            json: true,
        },
        journal.path(),
    );
    assert_eq!(dry.exit_code, 0, "{}", dry.stderr);
    assert!(!dry.stdout.contains("solstone.import.size.v1"));
    let parsed: serde_json::Value = serde_json::from_str(dry.stdout.trim()).unwrap();
    for key in [
        "operation",
        "size",
        "members",
        "source_bytes",
        "bytes",
        "days",
        "tiles",
        "span",
        "left_out",
    ] {
        assert!(parsed.get(key).is_some(), "{key}");
    }
    assert_eq!(parsed["operation"], "dry_run");

    let resumed = run_audio_archive(
        AudioArchiveDispatchOptions {
            path: folder.path().to_path_buf(),
            source_zone: Some("UTC".to_owned()),
            resume: Some("20260616_120000".to_owned()),
            dry_run: true,
            json: true,
        },
        journal.path(),
    );
    assert_ne!(resumed.exit_code, 0);
    assert!(!resumed.stdout.contains("\"operation\":\"dry_run\""));
    assert!(!journal.path().join("imports/20260616_120000").exists());

    let status = AudioArchiveStatus {
        size: AudioArchiveSize {
            members: 1,
            source_bytes: 4,
            ..AudioArchiveSize::default()
        },
        total_parts: 1,
        saved_parts: 1,
        saved_members: 1,
        saved_tiles: 0,
        processed_parts: 0,
        pending_processing: 1,
        unavailable_parts: 0,
        rejected_parts: 0,
        save_complete: true,
    };
    assert_ne!(status.saved_parts, status.processed_parts);
    let restored: AudioArchiveStatus =
        serde_json::from_str(&serde_json::to_string(&status).unwrap()).unwrap();
    assert_eq!(restored.saved_parts, 1);
    assert_eq!(restored.processed_parts, 0);
}

#[test]
fn audio_sync_keeps_one_key_and_an_imported_hash_is_not_forced() {
    let journal = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    fs::write(folder.path().join("keep.wav"), b"keep-wav").unwrap();
    fs::write(folder.path().join("edit.wav"), b"edit-wav").unwrap();
    let mut created = SyncState::empty(BackendName::Audio);
    let identity = begin_folder(&mut created, "Recordings", true, Some("grant-material")).unwrap();
    write_sync_state(journal.path(), &created).unwrap();
    let args = vec![
        "--sync".to_owned(),
        "audio".to_owned(),
        "--path".to_owned(),
        folder.path().display().to_string(),
    ];
    let preview = call::preview(&args, journal.path(), &idle_seams(journal.path()));
    assert_eq!(preview.exit_code, 0, "{}", preview.stderr);
    let started = call::start(
        args.clone(),
        journal.path().to_path_buf(),
        idle_seams(journal.path()),
        solstone_core_import::WorkStop::new(),
    )
    .unwrap();
    let (started, _) = started.join();
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let first = load(journal.path(), BackendName::Audio);
    assert_eq!(files_of(&first).len(), 2);
    let keep_hash = files_of(&first)["keep.wav"]["hash"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::write(folder.path().join("edit.wav"), b"edit-wav!").unwrap();
    let again = call::preview(&args, journal.path(), &idle_seams(journal.path()));
    assert_eq!(again.exit_code, 0, "{}", again.stderr);
    let second = load(journal.path(), BackendName::Audio);
    assert_eq!(files_of(&second).len(), 2);
    assert_eq!(
        files_of(&second)["keep.wav"]["hash"].as_str(),
        Some(keep_hash.as_str())
    );
    assert_ne!(
        files_of(&second)["edit.wav"]["hash"].as_str(),
        Some(files_of(&first)["edit.wav"]["hash"].as_str().unwrap())
    );
    let mut kept = second.clone();
    reauthorize(&mut kept, "Recordings", false, None).unwrap();
    assert_eq!(files_of(&kept).len(), 2);
    assert_eq!(
        kept.root()
            .get("source_identity")
            .and_then(serde_json::Value::as_str),
        Some(identity.as_str())
    );
    replace_folder(&mut kept, "Recordings", false, None).unwrap();
    assert!(files_of(&kept).is_empty());

    let skipped = tempfile::tempdir().unwrap();
    let source = skipped.path().join("one.wav");
    fs::write(&source, b"already-imported").unwrap();
    let digest = hash_source(&source).unwrap().into_inner();
    let prior = skipped.path().join("imports").join("prior");
    fs::create_dir_all(&prior).unwrap();
    fs::write(
        prior.join("manifest.json"),
        format!(r#"{{"source_hash":"{digest}"}}"#),
    )
    .unwrap();
    let skip_args = vec![
        "--sync".to_owned(),
        "audio".to_owned(),
        "--path".to_owned(),
        source.parent().unwrap().display().to_string(),
    ];
    let skipped_run = call::preview(&skip_args, skipped.path(), &idle_seams(skipped.path()));
    assert_eq!(skipped_run.exit_code, 0, "{}", skipped_run.stderr);
    let skipped_state = load(skipped.path(), BackendName::Audio);
    assert_eq!(files_of(&skipped_state).len(), 1);
    assert_eq!(
        files_of(&skipped_state)["one.wav"]["status"].as_str(),
        Some("imported")
    );
}

#[test]
fn plaud_preview_uses_a_synthetic_catalogue_and_stores_no_token() {
    // `call::preview --sync plaud` builds PlaudApi against https://api.plaud.ai.
    // This test stays on the in-process synthetic catalogue instead.
    let journal = tempfile::tempdir().unwrap();
    let request = PlaudSyncRequest::<SyncPreviewRequest>::new(journal.path().to_path_buf());
    let credential = Token;
    let mut catalogue = Catalogue;
    let manifests = Matches;
    let clock = Clock;
    let mut writer = MemoryWriter;
    let mut seams = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut writer,
    };
    let outcome = sync_plaud_preview(&request, &mut seams).unwrap();
    let rendered = format!(
        "{:?} {}",
        outcome.state,
        serde_json::to_string(outcome.state.root()).unwrap()
    );
    assert!(!rendered.contains("credential-not-for-state"));
    assert!(outcome.errors.is_empty());
    assert_eq!(files_of(&outcome.state).len(), 1);
}

#[test]
fn plaud_save_marks_the_file_imported_and_does_not_repeat() {
    // This save calls `run_plaud_with`; the preview test above still must not call `PlaudApi`.
    let journal = tempfile::tempdir().unwrap();
    let token = "plaud-token-not-for-state";
    let config = journal.path().join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("journal.json"),
        format!(r#"{{"env":{{"PLAUD_ACCESS_TOKEN":"{token}"}}}}"#),
    )
    .unwrap();
    let mut catalogue = Catalogue;
    let mut download = KnownDownload::default();
    let mut pipeline = ImportedPipeline::default();
    let first = crate::cli_argv::run_plaud_with(
        journal.path(),
        true,
        &mut catalogue,
        &mut download,
        &mut pipeline,
    );
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    let state = load(journal.path(), BackendName::Plaud);
    assert_eq!(
        files_of(&state)["file-1"]["status"].as_str(),
        Some("imported")
    );
    let stored = fs::read_to_string(journal.path().join("imports/plaud.json")).unwrap();
    assert!(!stored.contains(token));
    let written = download
        .written
        .clone()
        .expect("download wrote the recording");
    assert_eq!(fs::read(&written).unwrap(), b"known-plaud-bytes");
    let mut catalogue = Catalogue;
    let second = crate::cli_argv::run_plaud_with(
        journal.path(),
        true,
        &mut catalogue,
        &mut download,
        &mut pipeline,
    );
    assert_eq!(second.exit_code, 0, "{}", second.stderr);
    assert_eq!(download.calls, 1);
    assert_eq!(pipeline.calls, 1);
    assert_eq!(fs::read(written).unwrap(), b"known-plaud-bytes");
}

struct Token;

impl PlaudCredential for Token {
    fn access_token(&self) -> Option<&str> {
        Some("credential-not-for-state")
    }
}

struct Clock;

impl SyncClock for Clock {
    fn now(&self) -> String {
        "2026-06-16T12:00:00+00:00".to_owned()
    }
}

struct Catalogue;

impl PlaudCatalogue for Catalogue {
    fn list_files(&mut self, _token: &str) -> Result<Vec<PlaudFile>, PlaudFailureKind> {
        Ok(vec![PlaudFile {
            id: "file-1".to_owned(),
            filename: "note.m4a".to_owned(),
            fullname: "note.m4a".to_owned(),
            filesize: 4,
            start_time: 1_700_000_000.0,
            duration: 60_000.0,
            is_trash: false,
        }])
    }
}

struct Matches;

impl PlaudManifestLookup for Matches {
    fn matching_imports(
        &self,
        _files: &[PlaudFile],
    ) -> Result<std::collections::BTreeMap<String, String>, PlaudFailureKind> {
        Ok(std::collections::BTreeMap::new())
    }
}

struct MemoryWriter;

impl PlaudStateWriter for MemoryWriter {
    fn checkpoint(&mut self, _journal_root: &Path, _state: &SyncState) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct KnownDownload {
    calls: u32,
    written: Option<std::path::PathBuf>,
}

impl PlaudDownload for KnownDownload {
    fn temporary_url(&mut self, _token: &str, _file_id: &str) -> Result<String, PlaudFailureKind> {
        Ok("synthetic".to_owned())
    }

    fn download(&mut self, _url: &str, destination: &Path) -> Result<(), PlaudFailureKind> {
        self.calls += 1;
        fs::write(destination, b"known-plaud-bytes").map_err(|_| PlaudFailureKind::Download)?;
        self.written = Some(destination.to_path_buf());
        Ok(())
    }
}

#[derive(Default)]
struct ImportedPipeline {
    calls: u32,
}

impl ImportPipeline for ImportedPipeline {
    fn import_one(
        &mut self,
        _request: PipelineImportRequest<'_>,
    ) -> Result<PipelineOutcome, String> {
        self.calls += 1;
        Ok(PipelineOutcome::Imported)
    }
}
