// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use chrono::{DateTime, Local};
use image::{ImageBuffer, Rgb};
use solstone_core_generate::{
    ClientError, GenerateRequest, GenerateResponse, RefusalReason, RefusedResponse,
};
use solstone_core_import::{
    CreatedSegment, NativePublicationOperations, PublicationOperations, observe_source_immutability,
};
use solstone_core_import_sources::image::{
    DescriptionOutcome, ImageImportError, ProgressUpdate, WireClient, import_image,
};
use solstone_core_import_sources::{chatgpt, claude, gemini, ics, kindle, obsidian};
use solstone_core_indexer_store::scan::RescanFileStatus;
use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};
use solstone_core_segment::{StreamAdvance, UnboundStreamAdvanceError};
use support::TempTree;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

struct SuccessWire;

impl WireClient for SuccessWire {
    fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Generated(Box::new(
            solstone_core_generate::GeneratedResponse {
                id: None,
                text: "A red square.".to_owned(),
                model: "test".to_owned(),
                usage: serde_json::json!({}),
                finish_reason: "stop".to_owned(),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            },
        )))
    }
}

struct NoEngineWire;

impl WireClient for NoEngineWire {
    fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason: RefusalReason::NoEngineConfigured,
            reason_code: None,
            retryable: false,
            blocking: true,
            reset_at_ms: None,
            provider: None,
            detail: "no engine".to_owned(),
        }))
    }
}

struct FailingWire;

impl WireClient for FailingWire {
    fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Err(ClientError::Io("wire unavailable".to_owned()))
    }
}

#[test]
fn real_source_operations_leave_owner_exports_byte_identical() {
    let tree = TempTree::new();
    let claude_path = support::claude_archive(&tree);
    let chatgpt_path = support::chatgpt_archive(&tree);
    let gemini_path = support::gemini_archive(&tree);
    let kindle_path = support::kindle_clippings(&tree);

    // Image, archive, and document are real transactional sources and are deliberately absent
    // from MODULE_STUBS; the implemented-source tests below cover their real read/write behavior.
    // Document imports have their own source-immutability coverage because they install artifacts
    // from a real owner-controlled source file. Every remaining entry still represents a
    // non-mutating reserved seam.
    let report = observe_source_immutability(tree.path(), |_| {
        assert!(claude::detect(&claude_path).unwrap());
        claude::preview(&claude_path).unwrap();
        claude::plan(&claude_path).unwrap();
        assert!(chatgpt::detect(&chatgpt_path).unwrap());
        chatgpt::preview(&chatgpt_path).unwrap();
        chatgpt::plan(&chatgpt_path).unwrap();
        assert!(gemini::detect(&gemini_path).unwrap());
        gemini::preview(&gemini_path).unwrap();
        gemini::plan(&gemini_path).unwrap();
        assert!(kindle::detect(&kindle_path).unwrap());
        kindle::preview(&kindle_path).unwrap();
        kindle::plan(&kindle_path).unwrap();
    })
    .unwrap();

    assert!(!report.violated());
}

#[test]
fn image_import_preserves_source_and_records_its_segment_contract() {
    let tree = TempTree::new();
    let source_root = tree.path().join("source");
    let journal = tree.path().join("journal");
    fs::create_dir(&source_root).unwrap();
    let source = source_root.join("picture.PNG");
    ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
        .save(&source)
        .unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    let source_mtime = UNIX_EPOCH + Duration::from_secs(1_768_977_600);
    let source_time: DateTime<Local> = source_mtime.into();
    let day = source_time.format("%Y%m%d").to_string();
    let segment_key = format!("{}_0", source_time.format("%H%M%S"));
    fs::File::open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(source_mtime))
        .unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let source_mode = fs::metadata(&source).unwrap().permissions().mode() & 0o777;
    let mut progress = Vec::new();

    let report = observe_source_immutability(&source_root, |_| {
        let result = import_image(
            &source,
            &journal,
            "import-1",
            Some(&mut |update: &ProgressUpdate| progress.push(update.clone())),
            &NativePublicationOperations,
            &SuccessWire,
        )
        .unwrap();
        assert_eq!(result.created_segment.stream, "import.image");
        assert_eq!(result.created_segment.segment, segment_key);
        assert_eq!(result.days_affected, vec![day.clone()]);
        assert!(matches!(
            result.description,
            DescriptionOutcome::Generated(_)
        ));
        assert_eq!(result.files_created.len(), 1);
        let segment = journal
            .join("chronicle")
            .join(&day)
            .join("import.image")
            .join(&segment_key);
        let installed = segment.join("original.png");
        assert_eq!(fs::read(&installed).unwrap(), source_bytes);
        assert_eq!(
            fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&installed).unwrap().modified().unwrap(),
            source_mtime
        );
        assert!(segment.join("image_transcript.md").is_file());
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(journal.join("imports/import-1/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["days_affected"], serde_json::json!([day.as_str()]));
    })
    .unwrap();

    assert!(!report.violated());
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        source_mode
    );
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        source_mtime
    );
    assert_eq!(
        progress,
        [ProgressUpdate {
            current: 1,
            total: 1,
            earliest_date: day.clone(),
            latest_date: day,
            entities_found: 0,
        }]
    );
}

#[test]
fn unavailable_description_doors_preserve_the_installed_original() {
    for (label, wire) in [
        ("no-engine", &NoEngineWire as &dyn WireClient),
        ("wire-failure", &FailingWire as &dyn WireClient),
    ] {
        let tree = TempTree::new();
        let source = tree.path().join("picture.png");
        let journal = tree.path().join("journal");
        ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
            .save(&source)
            .unwrap();

        let result = import_image(
            &source,
            &journal,
            label,
            None,
            &NativePublicationOperations,
            wire,
        )
        .unwrap();
        let DescriptionOutcome::Unavailable { reason } = &result.description else {
            panic!("unavailable wire door must report a missing description");
        };
        let segment = journal.join(format!(
            "chronicle/{}/import.image/{}",
            result.created_segment.day, result.created_segment.segment
        ));
        assert_eq!(
            fs::read(segment.join("original.png")).unwrap(),
            fs::read(&source).unwrap()
        );
        let transcript = fs::read_to_string(segment.join("image_transcript.md")).unwrap();
        let (header, model) = transcript
            .split_once("\n---\n\n")
            .expect("transcript must contain a deterministic header divider");
        assert!(header.lines().all(|line| !line.starts_with('>')));
        assert!(model.lines().all(|line| line.starts_with('>')));
        assert!(!header.contains(reason));
        assert!(model.contains(reason));
    }
}

#[test]
fn image_original_is_dirtied_before_a_later_transcript_failure() {
    let tree = TempTree::new();
    let source = tree.path().join("picture.png");
    let journal = tree.path().join("journal");
    ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
        .save(&source)
        .unwrap();
    let source_mtime = UNIX_EPOCH + Duration::from_secs(1_768_977_600);
    fs::File::open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(source_mtime))
        .unwrap();
    let day = DateTime::<Local>::from(source_mtime)
        .format("%Y%m%d")
        .to_string();
    let publication = ImagePublication::sabotage_transcript();

    let error = import_image(
        &source,
        &journal,
        "later-failure",
        None,
        &publication,
        &SuccessWire,
    )
    .unwrap_err();

    assert!(
        matches!(error, ImageImportError::JournalIo { path, .. } if path.ends_with("image_transcript.md"))
    );
    let stream_dir = journal.join("chronicle").join(&day).join("import.image");
    let segment_dir = fs::read_dir(stream_dir)
        .unwrap()
        .next()
        .expect("partial image segment")
        .unwrap()
        .path();
    assert!(segment_dir.join("original.png").is_file());
    assert!(matches!(
        read_health_marker(&journal, &day, HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Versioned { marker, .. } if marker.generation == 1
    ));
    assert!(
        !journal
            .join("chronicle/20260819/health/stream.updated")
            .exists()
    );
}

#[test]
fn image_install_marker_failure_is_typed_and_retains_original() {
    let tree = TempTree::new();
    let source = tree.path().join("picture.png");
    let journal = tree.path().join("journal");
    ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
        .save(&source)
        .unwrap();
    let source_mtime = UNIX_EPOCH + Duration::from_secs(1_768_977_600);
    fs::File::open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(source_mtime))
        .unwrap();
    let day = DateTime::<Local>::from(source_mtime)
        .format("%Y%m%d")
        .to_string();
    let publication = ImagePublication::failing_marker();

    let error = import_image(
        &source,
        &journal,
        "marker-failure",
        None,
        &publication,
        &SuccessWire,
    )
    .unwrap_err();

    assert!(matches!(
        &error,
        ImageImportError::StreamMarker { day: failed_day, detail }
            if failed_day == &day && detail == "blocked marker"
    ));
    assert!(error.to_string().contains("remains installed"));
    let stream_dir = journal.join("chronicle").join(&day).join("import.image");
    let segment_dir = fs::read_dir(stream_dir)
        .unwrap()
        .next()
        .expect("partial image segment")
        .unwrap()
        .path();
    assert!(segment_dir.join("original.png").is_file());
    assert!(!segment_dir.join("image_transcript.md").exists());
}

#[derive(Clone, Copy)]
enum ImageMarkerBehavior {
    SabotageTranscript,
    Fail,
}

struct ImagePublication {
    behavior: ImageMarkerBehavior,
    days: Mutex<Vec<String>>,
}

impl ImagePublication {
    fn sabotage_transcript() -> Self {
        Self {
            behavior: ImageMarkerBehavior::SabotageTranscript,
            days: Mutex::new(Vec::new()),
        }
    }

    fn failing_marker() -> Self {
        Self {
            behavior: ImageMarkerBehavior::Fail,
            days: Mutex::new(Vec::new()),
        }
    }
}

impl PublicationOperations for ImagePublication {
    fn advance_stream(
        &self,
        _: &Path,
        _: &CreatedSegment,
    ) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
        Ok(StreamAdvance {
            prev_day: None,
            prev_segment: None,
            seq: 1,
        })
    }

    fn rescan_file(&self, _: &Path, _: &Path) -> Result<RescanFileStatus, String> {
        Ok(RescanFileStatus::Declined)
    }

    fn touch_stream_health_marker(&self, journal: &Path, day: &str) -> Result<(), String> {
        self.days.lock().unwrap().push(day.to_owned());
        match self.behavior {
            ImageMarkerBehavior::Fail => Err("blocked marker".to_owned()),
            ImageMarkerBehavior::SabotageTranscript => {
                PublicationOperations::touch_stream_health_marker(
                    &NativePublicationOperations,
                    journal,
                    day,
                )?;
                let stream_dir = journal.join("chronicle").join(day).join("import.image");
                let segment_dir = fs::read_dir(stream_dir)
                    .map_err(|error| error.to_string())?
                    .next()
                    .ok_or_else(|| "missing image segment".to_owned())?
                    .map_err(|error| error.to_string())?
                    .path();
                fs::create_dir(segment_dir.join("image_transcript.md"))
                    .map_err(|error| error.to_string())
            }
        }
    }

    fn emit_observed(&self, _: &Path, _: Option<&str>, _: &str, _: &str, _: &str) {}

    fn emit_enrichment_ready(
        &self,
        _: &Path,
        _: Option<&str>,
        _: &str,
        _: &str,
        _: &[String],
        _: u64,
    ) {
    }

    fn emit_drain(&self, _: &Path, _: Option<&str>, _: &str) {}
}

#[test]
fn implemented_source_reads_leave_the_owner_source_unchanged() {
    let tree = TempTree::new();
    let archive = tree.path().join("source-immutable/calendar.zip");
    let vault = tree.path().join("source-immutable/vault");
    fs::create_dir_all(&vault).unwrap();
    write_calendar_archive(&archive);
    fs::create_dir(vault.join(".obsidian")).unwrap();
    fs::write(vault.join(".obsidian/app.json"), "{}").unwrap();
    fs::write(vault.join("2024-01-02.md"), "# Daily\n[[Source Topic]]\n").unwrap();
    fs::write(vault.join("Reference.md"), "# Reference\n").unwrap();
    let before_metadata = metadata_snapshot(tree.path());

    let report = observe_source_immutability(tree.path(), |_| {
        assert!(ics::detect(&archive));
        let events = ics::parse_events(&archive).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(ics::preview(&archive).unwrap().item_count, 1);
        assert!(!ics::attendee_entities(&events).is_empty());

        assert!(obsidian::detect(&vault));
        let notes = obsidian::collect_notes(&vault).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(obsidian::preview(&vault).unwrap().item_count, 2);
        assert!(!obsidian::wikilink_entities(&notes).is_empty());
    })
    .unwrap();

    assert!(!report.violated());
    assert_eq!(metadata_snapshot(tree.path()), before_metadata);
}

fn write_calendar_archive(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = fs::File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
        .start_file("calendar.ics", SimpleFileOptions::default())
        .unwrap();
    archive
        .write_all(b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260102T120000Z\r\nATTENDEE;CN=Source Placeholder:mailto:source@example.test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n")
        .unwrap();
    archive.finish().unwrap();
}

#[derive(Debug, Eq, PartialEq)]
struct SourceMetadata {
    length: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    inode: u64,
}

fn metadata_snapshot(root: &Path) -> BTreeMap<PathBuf, SourceMetadata> {
    let mut snapshot = BTreeMap::new();
    collect_metadata(root, root, &mut snapshot);
    snapshot
}

fn collect_metadata(
    root: &Path,
    directory: &Path,
    snapshot: &mut BTreeMap<PathBuf, SourceMetadata>,
) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        snapshot.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            SourceMetadata {
                length: metadata.len(),
                modified: metadata.modified().unwrap(),
                #[cfg(unix)]
                mode: {
                    use std::os::unix::fs::MetadataExt;
                    metadata.mode()
                },
                #[cfg(unix)]
                inode: {
                    use std::os::unix::fs::MetadataExt;
                    metadata.ino()
                },
            },
        );
        if metadata.is_dir() {
            collect_metadata(root, &path, snapshot);
        }
    }
}
