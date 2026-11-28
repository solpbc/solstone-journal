// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::fs::{self, File, FileTimes};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::time::Instant;

use image::{ImageBuffer, ImageFormat, Rgba};
#[cfg(unix)]
use nix::{sys::signal::kill, unistd::Pid};
use serde_json::Value;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, RefusalReason,
    RefusedResponse,
};
use solstone_core_import::QueueFileOutcome;
use solstone_core_import::{CreatedSegment, PublicationOperations, observe_source_immutability};
#[cfg(unix)]
use solstone_core_import_sources::document::SystemPdfWorker;
use solstone_core_import_sources::document::{
    DocumentImportRequest, DocumentModelClient, DocumentPreviewRequest, LocalPdfOcrOptions,
    PdfCommand, PdfMetadata, PdfPage, PdfPageOcr, PdfPageOcrError, PdfPayload, PdfWorker,
    PdfWorkerRequest, WorkerFailure, extract_pdf_bytes_with_local_ocr, import,
    prepare_document_import, prepare_document_import_with_ocr, preview, preview_size,
};
use solstone_core_segment::{StreamAdvance, UnboundStreamAdvanceError, advance_unbound_stream};

#[cfg(unix)]
const PDF_WORKER_STDOUT_MAX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(unix)]
const PDF_WORKER_STDERR_MAX_BYTES: usize = 64 * 1024;
#[cfg(unix)]
const WORKER_SUCCESS_RESPONSE: &str =
    r#"{"schema":"sol-pdf/1","engine":"test","page_count":1,"pages":[{"index":1,"text":"text"}]}"#;

fn only_segment_directory(stream: &Path) -> PathBuf {
    let entries = fs::read_dir(stream)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    let segments = entries
        .iter()
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 1, "one partial segment in {stream:?}");
    let segment = segments[0].path();
    let mut lock_name = segment.file_name().unwrap().to_os_string();
    lock_name.push(".lock");
    for entry in entries {
        if entry.path() != segment {
            assert!(entry.file_type().unwrap().is_file());
            assert_eq!(entry.file_name(), lock_name);
        }
    }
    segment
}

#[test]
fn ac1_document_text_layer_imports_verbatim_with_zero_model_calls() {
    let tree = TestTree::new();
    let source = tree.pdf("plain.pdf", b"source");
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(
        1,
        50,
        0.0,
        Some("Raw text\n"),
    )]))]);
    let preview_worker = FakeWorker::new(vec![Ok(payload(vec![page(
        1,
        50,
        0.0,
        Some("Raw text\n"),
    )]))]);
    let preview = solstone_core_import_sources::document::preview_size(
        solstone_core_import_sources::document::DocumentPreviewRequest {
            source: &source,
            password: None,
            now: SystemTime::now(),
            zone: solstone_core_journal_config::owner_zone(tree.journal()),
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        tree.journal(),
        false,
        &preview_worker,
    )
    .unwrap();
    assert!(!tree.journal().join("chronicle").exists());
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();

    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert_eq!(preview.recordings.len(), 1);
    let coordinate = preview.recordings.first().unwrap();
    assert_eq!(
        (&coordinate.day, &coordinate.segment),
        (
            &result.segments.as_ref().unwrap()[0].0,
            &result.segments.as_ref().unwrap()[0].1
        )
    );
    assert_eq!(result.entries_written, 1);
    assert!(model.requests().is_empty());
    assert_eq!(
        worker.requests().len(),
        1,
        "pure text needs one worker call"
    );
    assert_eq!(worker.requests()[0].command, PdfCommand::Extract);
    assert!(
        fs::read_to_string(transcript_path(&tree, &result))
            .unwrap()
            .contains("## Page 1\n\nRaw text\n")
    );
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(tree.import_dir().join("content_manifest.jsonl")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["id"], "document-0");
    assert_eq!(manifest["type"], "document");
    assert_eq!(manifest["meta"]["timestamp_source"], "file-mtime");
    assert_eq!(manifest["meta"]["text_layer_pages"], 1);
    assert_eq!(manifest["meta"]["model_calls"], 0);
    assert_eq!(
        manifest["segments"][0]["day"],
        result.segments.as_ref().unwrap()[0].0
    );
    assert_eq!(
        manifest["segments"][0]["key"],
        result.segments.as_ref().unwrap()[0].1
    );
}

#[test]
fn ac2_document_one_image_per_call_for_three_image_only_pages() {
    let tree = TestTree::new();
    let source = tree.pdf("images.pdf", b"source");
    let first = payload(vec![
        page(1, 0, 0.0, None),
        page(2, 0, 0.0, None),
        page(3, 0, 0.0, None),
    ]);
    let rendered = payload(vec![rendered_page(1), rendered_page(2), rendered_page(3)]);
    let worker = FakeWorker::new(vec![Ok(first), Ok(rendered)]);
    let model = FakeModel::generated(["one", "two", "three"]);
    let publication = FakePublication::default();

    run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert_eq!(worker.requests().len(), 2);
    assert_eq!(
        model.requests().len(),
        3,
        "one call for each image-only page"
    );
    for request in model.requests() {
        assert_eq!(request.contents.len(), 2);
        assert_eq!(
            request
                .contents
                .iter()
                .filter(|part| matches!(part, ContentPart::Image { .. }))
                .count(),
            1
        );
        assert_eq!(
            request
                .contents
                .iter()
                .filter(|part| matches!(part, ContentPart::Text { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn ac3_document_model_marker_blockquotes_and_text_layer_is_raw() {
    let tree = TestTree::new();
    let source = tree.pdf("mixed.pdf", b"source");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 50, 0.10, Some("Deterministic text"))])),
        Ok(payload(vec![rendered_page(1)])),
    ]);
    let model = FakeModel::generated(["description line\n\nfinal line"]);
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    let transcript = fs::read_to_string(transcript_path(&tree, &result)).unwrap();
    assert!(transcript.contains("Deterministic text\n\n> [image description — model-generated; original: pages/page-0001.png]\n> description line\n>\n> final line"));
    assert!(!transcript.contains("> Deterministic text"));
}

#[test]
fn ac4_document_rasters_for_unreadable_pages_exist_after_import() {
    let tree = TestTree::new();
    let source = tree.pdf("raster.pdf", b"source");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 0, 0.0, None)])),
        Ok(payload(vec![rendered_page(1)])),
    ]);
    let model = FakeModel::generated(["read text"]);
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert!(
        segment_dir(&tree, &result)
            .join("pages/page-0001.png")
            .is_file()
    );
}

#[test]
fn ac5_document_unmodelable_page_names_existing_raster() {
    let tree = TestTree::new();
    let source = tree.pdf("unmodelable.pdf", b"source");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 0, 0.0, None)])),
        Ok(payload(vec![rendered_page(1)])),
    ]);
    let model = FakeModel::refused(RefusalReason::NoEngineConfigured);
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    let transcript = fs::read_to_string(transcript_path(&tree, &result)).unwrap();
    assert!(transcript.contains("> [page text unavailable — no brain configured; page image preserved at pages/page-0001.png]"));
    assert!(
        segment_dir(&tree, &result)
            .join("pages/page-0001.png")
            .is_file()
    );
    assert!(!transcript.contains("\n\n\n"));
}

#[test]
fn ac6_document_model_call_bound_is_pre_call_and_reaches_header() {
    let tree = TestTree::new();
    let source = tree.pdf("bound.pdf", b"source");
    let first = payload((1..=51).map(|index| page(index, 0, 0.0, None)).collect());
    let second = payload((1..=51).map(rendered_page).collect());
    let worker = FakeWorker::new(vec![Ok(first), Ok(second)]);
    let model = FakeModel::generated(std::iter::repeat_n("text", 50));
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    let transcript = fs::read_to_string(transcript_path(&tree, &result)).unwrap();
    assert_eq!(model.requests().len(), 50);
    assert!(transcript.contains("50 model calls"));
    assert!(transcript.contains("model-call limit reached"));
}

#[test]
fn ac7_document_worker_exit_classes_are_distinct_owner_outcomes() {
    let expected = [
        (1, "PDF worker internal failure"),
        (2, "PDF import configuration error"),
        (3, "encrypted PDF requires a password"),
        (4, "corrupt or unreadable PDF"),
        (5, "PDF render output failed"),
    ];
    let mut actual = Vec::new();
    for (exit_code, wording) in expected {
        let tree = TestTree::new();
        let source = tree.pdf("failure.pdf", b"source");
        let worker = FakeWorker::new(vec![Err(WorkerFailure::Process {
            exit_code: Some(exit_code),
            error: "failure".to_owned(),
            detail: Some("detail".to_owned()),
        })]);
        let model = FakeModel::generated([]);
        let publication = FakePublication::default();
        let result = run_import(
            &tree,
            &source,
            &worker,
            &model,
            &publication,
            SystemTime::now(),
        );
        assert_eq!(result.hard_failures.len(), 1);
        assert!(result.hard_failures[0].contains(wording));
        actual.push(result.hard_failures[0].clone());
    }
    actual.sort();
    actual.dedup();
    assert_eq!(actual.len(), 5);

    let tree = TestTree::new();
    let source = tree.pdf("timeout.pdf", b"source");
    let worker = FakeWorker::new(vec![Err(WorkerFailure::TimedOut {
        timeout: Duration::from_secs(90),
    })]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );
    assert!(result.hard_failures[0].contains("PDF worker timed out after 90s"));

    let tree = TestTree::new();
    let source = tree.pdf("terminated.pdf", b"source");
    let worker = FakeWorker::new(vec![Err(WorkerFailure::Process {
        exit_code: None,
        error: "PDF worker terminated by signal".to_owned(),
        detail: Some("PDF worker terminated by signal 9".to_owned()),
    })]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );
    assert!(result.hard_failures[0].contains("PDF worker internal failure"));
    assert!(result.hard_failures[0].contains("terminated by signal 9"));
    assert!(!result.hard_failures[0].contains("timed out"));
}

#[test]
fn ac8_document_preserves_owner_source_and_installs_private_mtime_copy() {
    let tree = TestTree::new();
    let source = tree.pdf("immutable.pdf", b"byte-identical source");
    let source_mtime = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(source_mtime))
        .unwrap();
    #[cfg(unix)]
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    let before = fs::read(&source).unwrap();
    let source_metadata = fs::metadata(&source).unwrap();
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(1, 50, 0.0, Some("raw"))]))]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();
    let mut result = None;
    let report = observe_source_immutability(tree.sources(), |_| {
        result = Some(run_import(
            &tree,
            &source,
            &worker,
            &model,
            &publication,
            SystemTime::now(),
        ));
    })
    .unwrap();
    let result = result.unwrap();
    let installed = segment_dir(&tree, &result).join("original.pdf");

    assert!(!report.violated());
    assert_eq!(fs::read(&source).unwrap(), before);
    assert_eq!(fs::metadata(&source).unwrap().len(), source_metadata.len());
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        source_mtime
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read(&installed).unwrap(), before);
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&installed).unwrap().modified().unwrap(),
        source_mtime
    );
}

#[test]
fn ac9_document_publication_days_come_from_claim_not_misleading_path() {
    let tree = TestTree::new();
    let source = tree.sources().join("20991231").join("misleading.pdf");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, b"source").unwrap();
    let claimed_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(claimed_time))
        .unwrap();
    let expected_day = jiff::Timestamp::try_from(claimed_time)
        .unwrap()
        .to_zoned_datetime(jiff::tz::TimeZone::system())
        .strftime("%Y%m%d")
        .to_string();
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(1, 50, 0.0, Some("raw"))]))]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert_ne!(expected_day, "20991231");
    assert_eq!(result.segments.as_ref().unwrap()[0].0, expected_day);
    assert_eq!(publication.days(), vec![expected_day.clone(), expected_day]);
}

#[test]
fn document_marker_failure_is_terminal_and_retains_the_publication_record() {
    let tree = TestTree::new();
    let source = tree.pdf("marker-failure.pdf", b"source");
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(1, 50, 0.0, Some("raw"))]))]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::failing_marker("blocked marker");

    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert_eq!(result.entries_written, 1);
    assert_eq!(result.hard_failures.len(), 1);
    assert!(result.hard_failures[0].contains("document publication failed"));
    assert!(result.errors.contains(&result.hard_failures[0]));
    let record: Value = serde_json::from_slice(
        &fs::read(tree.import_dir().join("imported.json")).expect("publication record"),
    )
    .unwrap();
    assert_eq!(record["status"], "failure");
    assert_eq!(record["day_markers"][0]["outcome"]["status"], "failed");
    assert_eq!(
        record["day_markers"][0]["outcome"]["error"],
        "blocked marker"
    );
}

#[test]
fn document_original_is_dirtied_before_a_later_raster_install_failure() {
    let tree = TestTree::new();
    let source = tree.pdf("later-raster-failure.pdf", b"source");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 0, 0.0, None)])),
        Ok(payload(vec![rendered_page(1)])),
    ]);
    let model = FakeModel::generated(["read text"]);
    let publication = FakePublication::sabotage_first_raster();

    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert_eq!(result.entries_written, 0);
    assert_eq!(publication.days().len(), 1);
    let days = publication.days();
    let day = &days[0];
    let stream_dir = tree
        .journal()
        .join("chronicle")
        .join(day)
        .join("import.document");
    let segment_dir = only_segment_directory(&stream_dir);
    assert_eq!(
        fs::read(segment_dir.join("original.pdf")).unwrap(),
        b"source"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("document import failed"))
    );
}

#[test]
fn document_install_marker_failure_is_typed_terminal_and_retains_original() {
    let tree = TestTree::new();
    let source = tree.pdf("install-marker-failure.pdf", b"source");
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(1, 50, 0.0, Some("raw"))]))]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::failing_install_marker("blocked install marker");

    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert_eq!(result.entries_written, 0);
    assert_eq!(result.hard_failures.len(), 1);
    assert!(result.hard_failures[0].contains("original PDF"));
    assert!(result.hard_failures[0].contains("remains installed"));
    assert!(!tree.import_dir().join("imported.json").exists());
    let days = publication.days();
    let day = &days[0];
    let segment_dir = only_segment_directory(
        &tree
            .journal()
            .join("chronicle")
            .join(day)
            .join("import.document"),
    );
    assert_eq!(
        fs::read(segment_dir.join("original.pdf")).unwrap(),
        b"source"
    );
}

#[test]
fn same_bytes_readded_after_failed_install_complete_in_the_original_segment() {
    let tree = TestTree::new();
    tree.write_utc_zone();
    let source = tree.pdf("retry.pdf", b"%PDF-same-source");
    let when = unix_time(1_781_611_200);
    set_mtime(&source, when);
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(
            1,
            50,
            0.0,
            Some("recovered text")
        )]));
        3
    ]);
    let model = FakeModel::generated([]);
    let failed = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &FakePublication::failing_install_marker("blocked install marker"),
        when,
    );
    assert_eq!(failed.entries_written, 0);
    assert_eq!(failed.hard_failures.len(), 1);
    let original_segment = tree
        .journal()
        .join("chronicle/20260616/import.document/120000_0");
    assert_eq!(
        fs::read(original_segment.join("original.pdf")).unwrap(),
        b"%PDF-same-source"
    );
    assert!(!original_segment.join("document_transcript.md").exists());

    let retry = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &FakePublication::default(),
        when,
    );
    assert_eq!(retry.entries_written, 1, "{:?}", retry.errors);
    assert!(retry.errors.is_empty(), "{:?}", retry.errors);
    assert_eq!(segment_dir(&tree, &retry), original_segment);
    assert!(
        fs::read_to_string(transcript_path(&tree, &retry))
            .unwrap()
            .contains("recovered text")
    );
    assert_eq!(fs::read(source).unwrap(), b"%PDF-same-source");
    let published: Value =
        serde_json::from_slice(&fs::read(tree.import_dir().join("imported.json")).unwrap())
            .unwrap();
    assert_eq!(published["status"], "success");

    let again = run_import(
        &tree,
        &tree.sources().join("retry.pdf"),
        &worker,
        &model,
        &FakePublication::default(),
        when,
    );
    assert_eq!(again.entries_written, 0);
    assert!(
        again
            .errors
            .iter()
            .any(|error| error.contains("already imported"))
    );
    assert_eq!(
        only_segment_directory(original_segment.parent().unwrap()),
        original_segment
    );
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_reads_large_stdout_without_timeout() {
    let tree = TestTree::new();
    let source = tree.pdf("large.pdf", b"source");
    let script = write_executable_script(
        &tree,
        "large-worker.sh",
        "#!/bin/sh\nprintf '%s' '{\"schema\":\"sol-pdf/1\",\"engine\":\"test\",\"page_count\":1,\"pages\":[{\"index\":1,\"chars\":262145,\"text\":\"'\nhead -c 262145 /dev/zero | tr '\\000' x\nprintf '%s\\n' '\"}]}'\n",
    );
    let worker = script_worker(script, Duration::from_secs(5));

    let payload = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap();

    assert_eq!(payload.schema, "sol-pdf/1");
    assert_eq!(payload.pages.len(), 1);
    assert_eq!(payload.pages[0].text.as_deref().unwrap().len(), 262_145);
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_accepts_stdout_at_byte_limit() {
    let tree = TestTree::new();
    let source = tree.pdf("stdout-at-limit.pdf", b"source");
    let prefix =
        r#"{"schema":"sol-pdf/1","engine":"test","page_count":1,"pages":[{"index":1,"text":""#;
    let suffix = r#""}]}"#;
    let padding = PDF_WORKER_STDOUT_MAX_BYTES - prefix.len() - suffix.len();
    let script = write_executable_script(
        &tree,
        "stdout-at-limit-worker.sh",
        &format!(
            "#!/bin/sh\nprintf '%s' '{prefix}'\nhead -c {padding} /dev/zero | tr '\\000' x\nprintf '%s' '{suffix}'\n"
        ),
    );
    let worker = script_worker(script, Duration::from_secs(5));

    let payload = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap();

    assert_eq!(payload.pages[0].text.as_deref().unwrap().len(), padding);
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_accepts_interleaved_stderr_at_byte_limit() {
    let tree = TestTree::new();
    let source = tree.pdf("stderr-at-limit.pdf", b"source");
    let first = PDF_WORKER_STDERR_MAX_BYTES / 2;
    let second = PDF_WORKER_STDERR_MAX_BYTES - first;
    let script = write_executable_script(
        &tree,
        "stderr-at-limit-worker.sh",
        &format!(
            "#!/bin/sh\nhead -c {first} /dev/zero | tr '\\000' x >&2\nprintf '%s' '{WORKER_SUCCESS_RESPONSE}'\nhead -c {second} /dev/zero | tr '\\000' x >&2\n"
        ),
    );
    let worker = script_worker(script, Duration::from_secs(5));

    let payload = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap();

    assert_eq!(payload.schema, "sol-pdf/1");
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_rejects_stdout_over_byte_limit_after_exit() {
    let tree = TestTree::new();
    let source = tree.pdf("stdout-over-limit.pdf", b"source");
    let script = write_executable_script(
        &tree,
        "stdout-over-limit-worker.sh",
        &format!(
            "#!/bin/sh\nhead -c {} /dev/zero | tr '\\000' x\n",
            PDF_WORKER_STDOUT_MAX_BYTES + 1
        ),
    );
    let worker = script_worker(script, Duration::from_secs(5));

    let failure = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap_err();

    assert_worker_stream_limit(failure, "stdout", PDF_WORKER_STDOUT_MAX_BYTES);
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_rejects_stderr_over_byte_limit_after_exit() {
    let tree = TestTree::new();
    let source = tree.pdf("stderr-over-limit.pdf", b"source");
    let script = write_executable_script(
        &tree,
        "stderr-over-limit-worker.sh",
        &format!(
            "#!/bin/sh\nprintf '%s' '{WORKER_SUCCESS_RESPONSE}'\nhead -c {} /dev/zero | tr '\\000' x >&2\n",
            PDF_WORKER_STDERR_MAX_BYTES + 1
        ),
    );
    let worker = script_worker(script, Duration::from_secs(5));

    let failure = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap_err();

    assert_worker_stream_limit(failure, "stderr", PDF_WORKER_STDERR_MAX_BYTES);
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_kills_stdout_over_byte_limit_before_timeout() {
    let tree = TestTree::new();
    let source = tree.pdf("stdout-over-limit-live.pdf", b"source");
    let script = write_executable_script(
        &tree,
        "stdout-over-limit-live-worker.sh",
        &format!(
            "#!/bin/sh\nmarker=\"$0.pid\"\nprintf '%s\\n' \"$$\" > \"$marker\"\nhead -c {} /dev/zero | tr '\\000' x\nexec sleep 300\n",
            PDF_WORKER_STDOUT_MAX_BYTES + 1
        ),
    );
    let marker = PathBuf::from(format!("{}.pid", script.display()));
    let worker = script_worker(script, Duration::from_secs(30));

    let started = Instant::now();
    let failure = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(5));
    assert_worker_stream_limit(failure, "stdout", PDF_WORKER_STDOUT_MAX_BYTES);
    let pid = fs::read_to_string(marker).unwrap().trim().parse().unwrap();
    assert!(kill(Pid::from_raw(pid), None).is_err());
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_kills_stderr_over_byte_limit_before_timeout() {
    let tree = TestTree::new();
    let source = tree.pdf("stderr-over-limit-live.pdf", b"source");
    let script = write_executable_script(
        &tree,
        "stderr-over-limit-live-worker.sh",
        &format!(
            "#!/bin/sh\nmarker=\"$0.pid\"\nprintf '%s\\n' \"$$\" > \"$marker\"\nprintf '%s' '{WORKER_SUCCESS_RESPONSE}'\nhead -c {} /dev/zero | tr '\\000' x >&2\nexec sleep 300\n",
            PDF_WORKER_STDERR_MAX_BYTES + 1
        ),
    );
    let marker = PathBuf::from(format!("{}.pid", script.display()));
    let worker = script_worker(script, Duration::from_secs(30));

    let started = Instant::now();
    let failure = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(5));
    assert_worker_stream_limit(failure, "stderr", PDF_WORKER_STDERR_MAX_BYTES);
    let pid = fs::read_to_string(marker).unwrap().trim().parse().unwrap();
    assert!(kill(Pid::from_raw(pid), None).is_err());
}

#[cfg(unix)]
#[test]
fn system_pdf_worker_keeps_in_budget_error_response_exit_code() {
    let tree = TestTree::new();
    let source = tree.pdf("in-budget-error.pdf", b"source");
    let script = write_executable_script(
        &tree,
        "in-budget-error-worker.sh",
        "#!/bin/sh\nprintf '%s\\n' '{\"error\":\"corrupt\",\"detail\":\"bad PDF\"}'\nexit 4\n",
    );
    let worker = script_worker(script, Duration::from_secs(5));

    let failure = worker
        .execute(&PdfWorkerRequest {
            command: PdfCommand::Extract,
            source,
            password: None,
            render: None,
        })
        .unwrap_err();

    assert!(matches!(
        failure,
        WorkerFailure::Process {
            exit_code: Some(4),
            error,
            detail: Some(detail),
        } if error == "corrupt" && detail == "bad PDF"
    ));
}

#[cfg(unix)]
#[test]
fn document_rejects_invalid_worker_protocol_before_artifact_writes() {
    let tree = TestTree::new();
    let source = tree.pdf("invalid.pdf", b"source");
    let script = write_executable_script(&tree, "invalid-worker.sh", "#!/bin/sh\nprintf '{}\\n'\n");
    let worker = script_worker(script, Duration::from_secs(5));
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();

    let result = import(
        DocumentImportRequest {
            source: &source,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "document-test",
            revision: None,
            password: None,
            force: false,
            now: SystemTime::now(),
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &model,
        &publication,
    );

    assert_eq!(result.entries_written, 0);
    assert!(result.hard_failures[0].contains("PDF worker protocol failure"));
    assert!(!tree.journal().join("chronicle").exists());
    assert!(!tree.import_dir().join("content_manifest.jsonl").exists());
}

#[test]
fn document_second_pass_failure_leaves_the_claimed_segment_without_an_original() {
    let tree = TestTree::new();
    let source = tree.pdf("render-failure.pdf", b"source");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 0, 0.0, None)])),
        Err(WorkerFailure::Process {
            exit_code: Some(5),
            error: "render failed".to_owned(),
            detail: Some("output failed".to_owned()),
        }),
    ]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();

    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    assert_eq!(result.entries_written, 0);
    assert!(!result.hard_failures.is_empty());
    let chronicle = tree.journal().join("chronicle");
    assert!(chronicle.is_dir());
    let mut segment_dirs = 0;
    for day in fs::read_dir(&chronicle).unwrap() {
        let stream = day.unwrap().path().join("import.document");
        if !stream.is_dir() {
            continue;
        }
        for segment in fs::read_dir(stream).unwrap() {
            let segment = segment.unwrap().path();
            if segment.is_dir() {
                segment_dirs += 1;
                assert!(!segment.join("original.pdf").exists());
            }
        }
    }
    assert_eq!(segment_dirs, 1);
}

#[test]
fn two_same_time_pdfs_claim_distinct_segments_and_keep_both_originals() {
    let tree = TestTree::new();
    tree.write_utc_zone();
    let when = unix_time(1_781_611_200);
    let first_bytes = b"%PDF-alpha";
    let second_bytes = b"%PDF-beta";
    set_mtime(&tree.pdf("a.pdf", first_bytes), when);
    set_mtime(&tree.pdf("b.pdf", second_bytes), when);
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(
            1,
            50,
            0.0,
            Some("alpha transcript body"),
        )])),
        Ok(payload(vec![page(
            1,
            50,
            0.0,
            Some("beta transcript body"),
        )])),
    ]);
    let model = FakeModel::generated([]);
    let publication = ChainPublication;
    let result = import(
        DocumentImportRequest {
            source: tree.sources(),
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "document-test",
            revision: None,
            password: None,
            force: false,
            now: when,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &model,
        &publication,
    );

    assert_eq!(result.entries_written, 2);
    let segments = result.segments.expect("two segments");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].0, "20260616");
    assert_eq!(segments[1].0, "20260616");
    assert_ne!(segments[0].1, segments[1].1);
    let first = segment_path(&tree, &segments[0].0, &segments[0].1);
    let second = segment_path(&tree, &segments[1].0, &segments[1].1);
    assert_eq!(fs::read(first.join("original.pdf")).unwrap(), first_bytes);
    assert_eq!(fs::read(second.join("original.pdf")).unwrap(), second_bytes);
    assert!(
        fs::read_to_string(first.join("document_transcript.md"))
            .unwrap()
            .contains("alpha transcript body")
    );
    assert!(
        fs::read_to_string(second.join("document_transcript.md"))
            .unwrap()
            .contains("beta transcript body")
    );
    let first_marker: Value =
        serde_json::from_str(&fs::read_to_string(first.join("stream.json")).unwrap()).unwrap();
    let second_marker: Value =
        serde_json::from_str(&fs::read_to_string(second.join("stream.json")).unwrap()).unwrap();
    assert_eq!(first_marker["seq"], 1);
    assert!(first_marker["prev_segment"].is_null());
    assert_eq!(second_marker["seq"], 2);
    assert_eq!(second_marker["prev_segment"], segments[0].1);
    assert_eq!(second_marker["prev_day"], "20260616");
}

#[test]
fn mixed_batch_keeps_the_installed_sibling_when_the_next_claim_crosses_the_day() {
    let tree = TestTree::new();
    tree.write_utc_zone();
    let when = unix_time(1_781_654_399);
    let kept = b"%PDF-kept";
    set_mtime(&tree.pdf("a.pdf", kept), when);
    set_mtime(&tree.pdf("b.pdf", b"%PDF-late"), when);
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(
            1,
            50,
            0.0,
            Some("kept transcript body"),
        )])),
        Ok(payload(vec![page(
            1,
            50,
            0.0,
            Some("late transcript body"),
        )])),
    ]);
    let model = FakeModel::generated([]);
    let publication = ChainPublication;
    let result = import(
        DocumentImportRequest {
            source: tree.sources(),
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "document-test",
            revision: None,
            password: None,
            force: false,
            now: when,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &model,
        &publication,
    );

    assert_eq!(result.entries_written, 1);
    assert!(result.hard_failures.is_empty());
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("document-segment-day-overflow"))
    );
    let segments = result.segments.expect("installed sibling");
    assert_eq!(
        segments,
        vec![("20260616".to_owned(), "235959_0".to_owned())]
    );
    let installed = segment_path(&tree, "20260616", "235959_0");
    assert_eq!(fs::read(installed.join("original.pdf")).unwrap(), kept);
    assert!(
        fs::read_to_string(installed.join("document_transcript.md"))
            .unwrap()
            .contains("kept transcript body")
    );
    assert!(!tree.journal().join("chronicle/20260617").exists());
}

#[test]
fn document_import_refuses_a_full_probe_and_an_occupied_day_boundary() {
    let tree = TestTree::new();
    tree.write_utc_zone();
    let when = unix_time(1_781_611_200);
    let source = tree.pdf("blocked.pdf", b"%PDF-blocked");
    set_mtime(&source, when);
    let stream = tree.journal().join("chronicle/20260616/import.document");
    fs::create_dir_all(&stream).unwrap();
    for offset in 0..60 {
        let total = 12 * 3600 + offset;
        let name = format!(
            "{:02}{:02}{:02}_0",
            total / 3600,
            (total % 3600) / 60,
            total % 60
        );
        let segment = stream.join(name);
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("keep"), b"keep").unwrap();
    }
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(
        1,
        50,
        0.0,
        Some("unused transcript body"),
    )]))]);
    let model = FakeModel::generated([]);
    let publication = ChainPublication;
    let blocked = import(
        DocumentImportRequest {
            source: &source,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "document-test",
            revision: None,
            password: None,
            force: false,
            now: when,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &model,
        &publication,
    );
    assert_eq!(blocked.entries_written, 0);
    assert!(
        blocked
            .hard_failures
            .iter()
            .any(|failure| failure.contains("document-segment-collision"))
    );
    assert!(!stream.join("120100_0").exists());
    assert_eq!(fs::read(stream.join("120000_0/keep")).unwrap(), b"keep");
    assert_eq!(fs::read(stream.join("120059_0/keep")).unwrap(), b"keep");

    let edge = TestTree::new();
    edge.write_utc_zone();
    let end = unix_time(1_781_654_399);
    let late = edge.pdf("late.pdf", b"%PDF-late");
    set_mtime(&late, end);
    let occupied = edge
        .journal()
        .join("chronicle/20260616/import.document/235959_0");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("keep"), b"edge").unwrap();
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(
        1,
        50,
        0.0,
        Some("unused transcript body"),
    )]))]);
    let overflow = import(
        DocumentImportRequest {
            source: &late,
            journal_root: edge.journal(),
            import_dir: edge.import_dir(),
            import_id: "document-test",
            revision: None,
            password: None,
            force: true,
            now: end,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &model,
        &publication,
    );
    assert_eq!(overflow.entries_written, 0);
    assert!(
        overflow
            .hard_failures
            .iter()
            .any(|failure| failure.contains("document-segment-day-overflow"))
    );
    assert_eq!(fs::read(occupied.join("keep")).unwrap(), b"edge");
    assert!(!edge.journal().join("chronicle/20260617").exists());
}

#[test]
fn document_missing_text_layer_emits_raster_backed_unavailable_marker() {
    let tree = TestTree::new();
    let source = tree.pdf("missing-text.pdf", b"source");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 50, 0.10, None)])),
        Ok(payload(vec![rendered_page(1)])),
    ]);
    let model = FakeModel::generated([]);
    let publication = FakePublication::default();
    let result = run_import(
        &tree,
        &source,
        &worker,
        &model,
        &publication,
        SystemTime::now(),
    );

    let transcript = fs::read_to_string(transcript_path(&tree, &result)).unwrap();
    assert!(transcript.contains(
        "> [page text unavailable — page 1: text layer missing; page image preserved at pages/page-0001.png]"
    ));
    assert!(
        segment_dir(&tree, &result)
            .join("pages/page-0001.png")
            .is_file()
    );
    assert!(model.requests().is_empty());
}

fn run_import<'a>(
    tree: &'a TestTree,
    source: &'a Path,
    worker: &'a FakeWorker,
    model: &'a FakeModel,
    publication: &'a FakePublication,
    now: SystemTime,
) -> solstone_core_import::ImportResult {
    import(
        DocumentImportRequest {
            source,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "document-test",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        worker,
        model,
        publication,
    )
}

fn payload(pages: Vec<PdfPage>) -> PdfPayload {
    PdfPayload {
        schema: "sol-pdf/1".to_owned(),
        engine: "oracle-only".to_owned(),
        sha256: String::new(),
        page_count: pages.len(),
        metadata: PdfMetadata::default(),
        pages,
        ..PdfPayload::default()
    }
}

fn page(index: usize, chars: usize, image_area_fraction: f64, text: Option<&str>) -> PdfPage {
    PdfPage {
        index,
        chars,
        image_area_fraction,
        text: text.map(str::to_owned),
        ..PdfPage::default()
    }
}

fn rendered_page(index: usize) -> PdfPage {
    PdfPage {
        index,
        rendered: Some(format!("page-{index:04}.png")),
        ..PdfPage::default()
    }
}

fn transcript_path(tree: &TestTree, result: &solstone_core_import::ImportResult) -> PathBuf {
    segment_dir(tree, result).join("document_transcript.md")
}
fn segment_dir(tree: &TestTree, result: &solstone_core_import::ImportResult) -> PathBuf {
    let (day, segment) = &result.segments.as_ref().unwrap()[0];
    tree.journal()
        .join("chronicle")
        .join(day)
        .join("import.document")
        .join(segment)
}

struct FakeWorker {
    responses: Mutex<VecDeque<Result<PdfPayload, WorkerFailure>>>,
    requests: Mutex<Vec<PdfWorkerRequest>>,
    png: Vec<u8>,
}

impl FakeWorker {
    fn new(responses: Vec<Result<PdfPayload, WorkerFailure>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            png: png_bytes(),
        }
    }
    fn requests(&self) -> Vec<PdfWorkerRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl PdfWorker for FakeWorker {
    fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        self.requests.lock().unwrap().push(request.clone());
        let payload = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected worker call")?;
        if let Some(render) = &request.render {
            fs::create_dir_all(&render.render_dir).unwrap();
            for page in &payload.pages {
                if let Some(name) = &page.rendered {
                    fs::write(render.render_dir.join(name), &self.png).unwrap();
                }
            }
        }
        Ok(payload)
    }
}

struct FakeModel {
    responses: Mutex<VecDeque<GenerateResponse>>,
    requests: Mutex<Vec<GenerateRequest>>,
}

impl FakeModel {
    fn generated(values: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            responses: Mutex::new(values.into_iter().map(generated).collect()),
            requests: Mutex::new(Vec::new()),
        }
    }
    fn refused(reason: RefusalReason) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([GenerateResponse::Refused(
                RefusedResponse {
                    id: None,
                    reason,
                    reason_code: None,
                    retryable: false,
                    blocking: false,
                    reset_at_ms: None,
                    provider: None,
                    detail: String::new(),
                },
            )])),
            requests: Mutex::new(Vec::new()),
        }
    }
    fn requests(&self) -> Vec<GenerateRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl DocumentModelClient for FakeModel {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected model call"))
    }
}

fn generated(text: &'static str) -> GenerateResponse {
    GenerateResponse::Generated(Box::new(GeneratedResponse {
        id: None,
        text: text.to_owned(),
        model: "fake".to_owned(),
        usage: Value::Null,
        finish_reason: "stop".to_owned(),
        thinking: None,
        schema_validation: None,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied: Vec::new(),
    }))
}

#[derive(Default)]
struct FakePublication {
    days: Mutex<Vec<String>>,
    marker_failure: Option<String>,
    marker_failure_at: Option<usize>,
    sabotage_first_raster: bool,
}

impl FakePublication {
    fn days(&self) -> Vec<String> {
        self.days.lock().unwrap().clone()
    }

    fn failing_marker(detail: &str) -> Self {
        Self {
            marker_failure: Some(detail.to_owned()),
            marker_failure_at: Some(2),
            ..Self::default()
        }
    }

    fn failing_install_marker(detail: &str) -> Self {
        Self {
            marker_failure: Some(detail.to_owned()),
            marker_failure_at: Some(1),
            ..Self::default()
        }
    }

    fn sabotage_first_raster() -> Self {
        Self {
            sabotage_first_raster: true,
            ..Self::default()
        }
    }
}

impl PublicationOperations for FakePublication {
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
    fn enqueue_path(&self, _: &Path, _: &Path) -> Result<QueueFileOutcome, String> {
        Ok(QueueFileOutcome::Declined)
    }
    fn touch_stream_health_marker(&self, journal: &Path, day: &str) -> Result<(), String> {
        let call = {
            let mut days = self.days.lock().unwrap();
            days.push(day.to_owned());
            days.len()
        };
        if self.sabotage_first_raster && call == 1 {
            let stream_dir = journal.join("chronicle").join(day).join("import.document");
            let segment_dir = only_segment_directory(&stream_dir);
            fs::create_dir(segment_dir.join("pages/page-0001.png"))
                .map_err(|error| error.to_string())?;
        }
        if self.marker_failure_at == Some(call) {
            Err(self
                .marker_failure
                .clone()
                .expect("configured marker failure has detail"))
        } else {
            Ok(())
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

struct ChainPublication;

impl PublicationOperations for ChainPublication {
    fn advance_stream(
        &self,
        journal: &Path,
        segment: &CreatedSegment,
    ) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
        advance_unbound_stream(
            journal,
            &segment.stream,
            &segment.day,
            &segment.segment,
            segment.hints.clone(),
        )
    }
    fn enqueue_path(&self, _: &Path, _: &Path) -> Result<QueueFileOutcome, String> {
        Ok(QueueFileOutcome::Declined)
    }
    fn touch_stream_health_marker(&self, _: &Path, _: &str) -> Result<(), String> {
        Ok(())
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

fn unix_time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn set_mtime(path: &Path, time: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn segment_path(tree: &TestTree, day: &str, segment: &str) -> PathBuf {
    tree.journal()
        .join("chronicle")
        .join(day)
        .join("import.document")
        .join(segment)
}

fn png_bytes() -> Vec<u8> {
    let image = ImageBuffer::from_pixel(1, 1, Rgba([255_u8, 0, 0, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[cfg(unix)]
fn assert_worker_stream_limit(failure: WorkerFailure, stream: &str, maximum_bytes: usize) {
    match failure {
        WorkerFailure::ProtocolViolation { detail } => {
            assert!(detail.contains(stream));
            assert!(detail.contains(&maximum_bytes.to_string()));
        }
        failure => panic!("expected stream limit violation, got {failure:?}"),
    }
}

struct TestTree {
    root: PathBuf,
    journal: PathBuf,
    imports: PathBuf,
    sources: PathBuf,
}

impl TestTree {
    fn new() -> Self {
        let unique = tempfile::Builder::new()
            .prefix("solstone-document-test-")
            .tempdir()
            .unwrap()
            .keep();
        fs::create_dir_all(unique.join("journal")).unwrap();
        fs::create_dir_all(unique.join("imports/document-test")).unwrap();
        fs::create_dir_all(unique.join("sources")).unwrap();
        Self {
            journal: unique.join("journal"),
            imports: unique.join("imports/document-test"),
            sources: unique.join("sources"),
            root: unique,
        }
    }
    fn journal(&self) -> &Path {
        &self.journal
    }
    fn import_dir(&self) -> &Path {
        &self.imports
    }
    fn sources(&self) -> &Path {
        &self.sources
    }
    #[cfg(unix)]
    fn root(&self) -> &Path {
        &self.root
    }
    fn pdf(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.sources.join(name);
        fs::write(&path, contents).unwrap();
        path
    }
    fn write_utc_zone(&self) {
        let config = self.journal.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
    }
}

/// Runs a worker script this test process has just written. A child that a
/// parallel test forks inherits the script's write descriptor until it execs,
/// so the kernel can briefly refuse to execute the script with ETXTBSY
/// ("Text file busy", os error 26). Only that spawn failure is retried, and
/// only for a bounded time; every other outcome returns unchanged.
#[cfg(unix)]
struct FreshScriptWorker(SystemPdfWorker);

#[cfg(unix)]
impl PdfWorker for FreshScriptWorker {
    fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.0.execute(request) {
                Err(WorkerFailure::Process {
                    detail: Some(detail),
                    ..
                }) if detail.contains("(os error 26)") && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                outcome => return outcome,
            }
        }
    }
}

#[cfg(unix)]
fn script_worker(script: PathBuf, timeout: Duration) -> FreshScriptWorker {
    FreshScriptWorker(SystemPdfWorker::new(script, timeout))
}

#[cfg(unix)]
fn write_executable_script(tree: &TestTree, name: &str, contents: &str) -> PathBuf {
    let path = tree.root().join(name);
    fs::write(&path, contents).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

impl Drop for TestTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn pdf_twins_timestamp_resolution_ladder_and_directory_boundary() {
    let tree = TestTree::new();
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let old_mtime = UNIX_EPOCH + Duration::from_secs(1_704_067_200);
    let pdf_bytes = b"%PDF-1.4 twin test bytes";
    let model = FakeModel::generated([]);

    // 1. Single file, owner_timestamp: Some("20260301_153000"):
    //    preview date range, preview_size recording day, and prepare_document_import timestamp_source
    //    are 20260301 / owner. This wins even when the payload has a different credible creation date.
    let file1 = tree.pdf("owner_override.pdf", pdf_bytes);
    File::options()
        .write(true)
        .open(&file1)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old_mtime))
        .unwrap();
    let mut pl1 = payload(vec![page(1, 50, 0.0, Some("doc text"))]);
    pl1.metadata.creation_date = Some("2026-05-10T14:30:00Z".to_owned());
    let worker1 = FakeWorker::new(vec![Ok(pl1.clone()), Ok(pl1.clone()), Ok(pl1.clone())]);
    let prev1 = preview(
        DocumentPreviewRequest {
            source: &file1,
            password: None,
            now,
            zone: jiff::tz::TimeZone::UTC,
            owner_timestamp: Some("20260301_153000"),
            fallback_timestamp: None,
        },
        &worker1,
    );
    assert_eq!(prev1.date_range.0, "20260301");
    assert_eq!(prev1.date_range.1, "20260301");

    let size1 = preview_size(
        DocumentPreviewRequest {
            source: &file1,
            password: None,
            now,
            zone: jiff::tz::TimeZone::UTC,
            owner_timestamp: Some("20260301_153000"),
            fallback_timestamp: None,
        },
        tree.journal(),
        false,
        &worker1,
    )
    .unwrap();
    assert_eq!(size1.recordings.iter().next().unwrap().day, "20260301");

    let prep1 = prepare_document_import(
        &DocumentImportRequest {
            source: &file1,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "twin-test-1",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: Some("20260301_153000"),
            fallback_timestamp: None,
        },
        &worker1,
        &model,
    );
    assert_eq!(prep1.items[0].timestamp_source, "owner");
    assert!(
        prep1.items[0]
            .segment_dir
            .starts_with(tree.journal().join("chronicle/20260301"))
    );

    // 2. Single file, owner_timestamp: None, filename note_20260402_101500.pdf,
    //    payload creation_date absent or equal to that wall time: source filename, day 20260402.
    let file2 = tree.pdf("note_20260402_101500.pdf", pdf_bytes);
    File::options()
        .write(true)
        .open(&file2)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old_mtime))
        .unwrap();
    let mut pl2 = payload(vec![page(1, 50, 0.0, Some("doc text"))]);
    pl2.metadata.creation_date = None;
    let worker2 = FakeWorker::new(vec![Ok(pl2)]);
    let prep2 = prepare_document_import(
        &DocumentImportRequest {
            source: &file2,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "twin-test-2",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker2,
        &model,
    );
    assert_eq!(prep2.items[0].timestamp_source, "filename");
    assert!(
        prep2.items[0]
            .segment_dir
            .starts_with(tree.journal().join("chronicle/20260402"))
    );

    // 3. The same bytes and payload with a name that has no stamp and owner_timestamp: None:
    //    today's order. If the payload has an in-window mod_date, source is pdf-metadata
    //    and the day is that date, not the file mtime.
    let file3 = tree.pdf("plain_notes.pdf", pdf_bytes);
    File::options()
        .write(true)
        .open(&file3)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old_mtime))
        .unwrap();
    let mut pl3 = payload(vec![page(1, 50, 0.0, Some("doc text"))]);
    pl3.metadata.creation_date = None;
    pl3.metadata.mod_date = Some("2026-05-01T12:00:00Z".to_owned());
    let worker3 = FakeWorker::new(vec![Ok(pl3)]);
    let prep3 = prepare_document_import(
        &DocumentImportRequest {
            source: &file3,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "twin-test-3",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker3,
        &model,
    );
    assert_eq!(prep3.items[0].timestamp_source, "pdf-metadata");
    assert!(
        prep3.items[0]
            .segment_dir
            .starts_with(tree.journal().join("chronicle/20260501"))
    );

    // 4. Filename 20260101_080000.pdf plus a credible in-window creation_date on a
    //    different owner-zone day: source is not filename. The creation date wins through
    //    the existing ladder (pdf-metadata).
    let file4 = tree.pdf("20260101_080000.pdf", pdf_bytes);
    File::options()
        .write(true)
        .open(&file4)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old_mtime))
        .unwrap();
    let mut pl4 = payload(vec![page(1, 50, 0.0, Some("doc text"))]);
    pl4.metadata.creation_date = Some("2026-06-15T09:00:00Z".to_owned());
    let worker4 = FakeWorker::new(vec![Ok(pl4)]);
    let prep4 = prepare_document_import(
        &DocumentImportRequest {
            source: &file4,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "twin-test-4",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker4,
        &model,
    );
    assert_ne!(prep4.items[0].timestamp_source, "filename");
    assert_eq!(prep4.items[0].timestamp_source, "pdf-metadata");
    assert!(
        prep4.items[0]
            .segment_dir
            .starts_with(tree.journal().join("chronicle/20260615"))
    );

    // 5. A directory of two PDFs, request owner_timestamp: Some("20260301_153000"):
    //    that stamp is not applied. Each file keeps its own payload date.
    let dir = tree.sources().join("two_pdfs");
    fs::create_dir_all(&dir).unwrap();
    let file5a = dir.join("doc_a.pdf");
    let file5b = dir.join("doc_b.pdf");
    fs::write(&file5a, pdf_bytes).unwrap();
    fs::write(&file5b, pdf_bytes).unwrap();
    File::options()
        .write(true)
        .open(&file5a)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old_mtime))
        .unwrap();
    File::options()
        .write(true)
        .open(&file5b)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old_mtime))
        .unwrap();

    let mut pl5a = payload(vec![page(1, 50, 0.0, Some("doc a"))]);
    pl5a.metadata.creation_date = Some("2026-07-01T10:00:00Z".to_owned());
    let mut pl5b = payload(vec![page(1, 50, 0.0, Some("doc b"))]);
    pl5b.metadata.creation_date = Some("2026-08-01T10:00:00Z".to_owned());

    let worker5 = FakeWorker::new(vec![Ok(pl5a), Ok(pl5b)]);
    let prep5 = prepare_document_import(
        &DocumentImportRequest {
            source: &dir,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "twin-test-5",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: Some("20260301_153000"),
            fallback_timestamp: None,
        },
        &worker5,
        &model,
    );
    assert_eq!(prep5.items.len(), 2);
    for item in &prep5.items {
        assert_ne!(item.timestamp_source, "owner");
        assert_eq!(item.timestamp_source, "pdf-metadata");
    }
    let has_day_0701 = prep5.items.iter().any(|item| {
        item.segment_dir
            .starts_with(tree.journal().join("chronicle/20260701"))
    });
    let has_day_0801 = prep5.items.iter().any(|item| {
        item.segment_dir
            .starts_with(tree.journal().join("chronicle/20260801"))
    });
    assert!(has_day_0701);
    assert!(has_day_0801);
}

#[test]
fn a_pdf_owner_time_in_a_dst_gap_is_refused_before_extraction_or_writes() {
    let tree = TestTree::new();
    let source = tree.pdf("notes.pdf", b"%PDF-1.4 fixture");
    fs::create_dir_all(tree.journal().join("config")).unwrap();
    fs::write(
        tree.journal().join("config/journal.json"),
        br#"{"identity":{"timezone":"America/New_York"}}"#,
    )
    .unwrap();
    let worker = FakeWorker::new(vec![]);
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let result = preview_size(
        DocumentPreviewRequest {
            source: &source,
            password: None,
            now,
            zone: solstone_core_time::zone("America/New_York").unwrap(),
            owner_timestamp: Some("20260308_023000"),
            fallback_timestamp: None,
        },
        tree.journal(),
        false,
        &worker,
    );
    assert!(result.unwrap_err().contains("does not exist"));
    let prepared = prepare_document_import(
        &DocumentImportRequest {
            source: &source,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "gap",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: Some("20260308_023000"),
            fallback_timestamp: None,
        },
        &worker,
        &FakeModel::generated([]),
    );
    assert!(prepared.items.is_empty());
    assert!(!prepared.hard_failures.is_empty());
    assert!(!tree.journal().join("chronicle").exists());
}

struct FakePageOcr {
    responses: VecDeque<Result<String, PdfPageOcrError>>,
    calls: usize,
}

impl FakePageOcr {
    fn new(responses: impl IntoIterator<Item = Result<String, PdfPageOcrError>>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
            calls: 0,
        }
    }
}

impl PdfPageOcr for FakePageOcr {
    fn recognize_png(&mut self, _png: &[u8]) -> Result<String, PdfPageOcrError> {
        self.calls += 1;
        self.responses
            .pop_front()
            .unwrap_or(Err(PdfPageOcrError::NotInstalled))
    }
}

#[test]
fn scanned_pdf_import_defaults_to_local_ocr_with_zero_model_calls() {
    let tree = TestTree::new();
    let source = tree.pdf("scan.pdf", b"%PDF-1.4 scan");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 0, 1.0, Some(""))])),
        Ok(payload(vec![rendered_page(1)])),
    ]);
    let model = FakeModel::generated([]);
    let mut ocr = FakePageOcr::new([Ok("Invoice total $420.00".to_owned())]);

    let prepared = prepare_document_import_with_ocr(
        &DocumentImportRequest {
            source: &source,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "scan-ocr",
            revision: None,
            password: None,
            force: false,
            now: SystemTime::now(),
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &model,
        &mut ocr,
    );

    assert_eq!(ocr.calls, 1);
    assert!(
        model.requests().is_empty(),
        "local OCR handles scanned pages without model calls"
    );
    assert_eq!(prepared.items.len(), 1);
    let item = &prepared.items[0];
    assert!(item.prepared.extraction_complete());
    assert_eq!(item.prepared.stats.text_layer_pages, 1);
    assert_eq!(item.prepared.stats.model_calls, 0);
    assert!(
        item.prepared
            .transcript
            .contains("## Page 1\n\nInvoice total $420.00\n"),
        "{}",
        item.prepared.transcript
    );
}

#[test]
fn wordless_image_page_falls_back_to_model_when_local_ocr_is_empty() {
    let tree = TestTree::new();
    let source = tree.pdf("diagram.pdf", b"%PDF-1.4 diagram");
    let worker = FakeWorker::new(vec![
        Ok(payload(vec![page(1, 0, 1.0, None)])),
        Ok(payload(vec![rendered_page(1)])),
    ]);
    let model = FakeModel::generated(["Architecture flowchart"]);
    let mut ocr = FakePageOcr::new([Ok(String::new())]);

    let prepared = prepare_document_import_with_ocr(
        &DocumentImportRequest {
            source: &source,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "diagram-fallback",
            revision: None,
            password: None,
            force: false,
            now: SystemTime::now(),
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &model,
        &mut ocr,
    );

    assert_eq!(ocr.calls, 1);
    assert_eq!(model.requests().len(), 1);
    assert_eq!(prepared.items.len(), 1);
    assert!(
        prepared.items[0]
            .prepared
            .transcript
            .contains("Architecture flowchart")
    );
}

#[test]
fn extract_pdf_bytes_with_local_ocr_reads_mixed_typed_and_scanned_pages() {
    let staging = tempfile::tempdir().unwrap();
    let native = PdfPayload {
        schema: "sol-pdf/1".to_owned(),
        engine: "test".to_owned(),
        page_count: 3,
        pages: vec![
            PdfPage {
                index: 1,
                chars: 32,
                width_pt: 612.0,
                height_pt: 792.0,
                image_area_fraction: 0.0,
                text: Some("Native typed page one.".to_owned()),
                ..PdfPage::default()
            },
            PdfPage {
                index: 2,
                chars: 0,
                width_pt: 612.0,
                height_pt: 792.0,
                image_area_fraction: 1.0,
                text: Some(String::new()),
                ..PdfPage::default()
            },
            PdfPage {
                index: 3,
                chars: 6,
                width_pt: 612.0,
                height_pt: 792.0,
                image_area_fraction: 0.95,
                text: Some("Page 3".to_owned()),
                ..PdfPage::default()
            },
        ],
        ..PdfPayload::default()
    };
    let rendered = payload(vec![rendered_page(2), rendered_page(3)]);
    let worker = FakeWorker::new(vec![Ok(native), Ok(rendered)]);
    let mut ocr = FakePageOcr::new([
        Ok("Scanned page two text.".to_owned()),
        Ok("Page 3\nFull scanned receipt body.".to_owned()),
    ]);

    let result = extract_pdf_bytes_with_local_ocr(
        staging.path(),
        b"%PDF-1.4 mixed",
        &worker,
        &mut ocr,
        LocalPdfOcrOptions::default(),
    )
    .unwrap();

    let requests = worker.requests();
    assert_eq!(requests.len(), 2);
    let render_req = requests[1].render.as_ref().expect("sparse pages rendered");
    assert_eq!(render_req.pages, std::collections::BTreeSet::from([2, 3]));
    assert_eq!(result.ocr_pages_read, 2);
    assert_eq!(result.ocr_pages_failed, 0);
    assert!(!result.ocr_not_installed);
    assert_eq!(
        result.payload.pages[0].text.as_deref(),
        Some("Native typed page one.")
    );
    assert_eq!(
        result.payload.pages[1].text.as_deref(),
        Some("Scanned page two text.")
    );
    assert_eq!(
        result.payload.pages[2].text.as_deref(),
        Some("Page 3\nFull scanned receipt body.")
    );
}

fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let mut buf = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, opts).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }
    buf.into_inner()
}

fn build_test_docx(body_xml: &str, title: Option<&str>) -> Vec<u8> {
    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#
                .to_vec(),
        ),
        (
            "_rels/.rels",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"#
                .to_vec(),
        ),
        (
            "word/document.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>{body_xml}</w:body>
</w:document>"#
            )
            .into_bytes(),
        ),
    ];
    if let Some(title) = title {
        entries.push((
            "docProps/core.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
  xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>{title}</dc:title>
</cp:coreProperties>"#
            )
            .into_bytes(),
        ));
    }
    let refs: Vec<(&str, &[u8])> = entries.iter().map(|(k, v)| (*k, v.as_slice())).collect();
    build_test_zip(&refs)
}

fn build_test_encrypted_ooxml() -> Vec<u8> {
    let mut file = vec![0u8; 512 * 3];
    file[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    file[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    file[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    file[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
    file[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
    file[0x44..0x48].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    file[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
    for i in 1..109 {
        file[0x4C + i * 4..0x50 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    file[512..516].copy_from_slice(&0xFFFF_FFFDu32.to_le_bytes());
    file[516..520].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    for i in 2..128 {
        file[512 + i * 4..512 + i * 4 + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    let write_entry = |buf: &mut [u8], name: &str, kind: u8, right: u32, child: u32| {
        let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        for (i, u) in units.iter().enumerate() {
            buf[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        buf[0x40..0x42].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
        buf[0x42] = kind;
        buf[0x44..0x48].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        buf[0x48..0x4C].copy_from_slice(&right.to_le_bytes());
        buf[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
    };
    write_entry(&mut file[1024..1152], "Root Entry", 5, 0xFFFF_FFFF, 1);
    write_entry(&mut file[1152..1280], "EncryptionInfo", 2, 2, 0xFFFF_FFFF);
    write_entry(
        &mut file[1280..1408],
        "EncryptedPackage",
        2,
        0xFFFF_FFFF,
        0xFFFF_FFFF,
    );
    file
}

struct PanickingPdfWorker;
impl PdfWorker for PanickingPdfWorker {
    fn prepare(&self) -> Result<(), String> {
        Err("PDF worker unavailable".to_owned())
    }
    fn execute(&self, _request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        panic!("Office documents must never invoke PdfWorker");
    }
}

struct PanickingModel;
impl DocumentModelClient for PanickingModel {
    fn prepare(&self) -> Result<(), String> {
        Err("Model client unavailable".to_owned())
    }
    fn execute(&self, _request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        panic!("Office documents must never invoke DocumentModelClient");
    }
}

#[test]
fn office_documents_import_without_pdf_or_model_with_parity_dedupe_and_refusals() {
    let tree = TestTree::new();
    let docx_bytes = build_test_docx(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Project Brief</w:t></w:r></w:p>
           <w:p><w:r><w:t>Pure Rust extraction works without PDFium.</w:t></w:r></w:p>"#,
        Some("Project Brief Title"),
    );
    let source = tree.pdf("brief.docx", &docx_bytes);
    // Future-skewed mtime (> 24h ahead of `now`) forces fallback_timestamp path,
    // proving preview_size and prepare_document_import match coordinates even at
    // different `now` instants.
    let now_preview = UNIX_EPOCH + Duration::from_secs(1_776_000_000);
    let now_import = now_preview + Duration::from_secs(120);
    File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(FileTimes::new().set_modified(now_preview + Duration::from_secs(172_800)))
        .unwrap();

    assert!(solstone_core_import_sources::document::detect(&source));
    assert!(!solstone_core_import_sources::document::has_pdf(&source));

    let zone = solstone_core_journal_config::owner_zone(tree.journal());
    let prev = preview(
        DocumentPreviewRequest {
            source: &source,
            password: None,
            now: now_preview,
            zone: zone.clone(),
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_093000"),
        },
        &PanickingPdfWorker,
    );
    assert_eq!(prev.item_count, 1);
    assert_eq!(
        prev.date_range,
        ("20260410".to_owned(), "20260410".to_owned())
    );

    let size = preview_size(
        DocumentPreviewRequest {
            source: &source,
            password: None,
            now: now_preview,
            zone: zone.clone(),
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_093000"),
        },
        tree.journal(),
        false,
        &PanickingPdfWorker,
    )
    .expect("preview_size succeeds without PDFium");
    assert_eq!(size.recordings.len(), 1);
    let planned = size.recordings.first().unwrap();
    assert_eq!(planned.day, "20260410");
    assert_eq!(planned.segment, "093000_0");

    let publication = FakePublication::default();
    let result = import(
        DocumentImportRequest {
            source: &source,
            journal_root: tree.journal(),
            import_dir: &tree.journal().join("imports/20260410_093000"),
            import_id: "20260410_093000",
            revision: None,
            password: None,
            force: false,
            now: now_import,
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_093000"),
        },
        &PanickingPdfWorker,
        &PanickingModel,
        &publication,
    );
    assert_eq!(result.entries_written, 1, "{:?}", result.errors);
    let (day, seg) = &result.segments.as_ref().unwrap()[0];
    assert_eq!(day, &planned.day);
    assert_eq!(seg, &planned.segment);

    let seg_dir = segment_dir(&tree, &result);
    assert_eq!(fs::read(seg_dir.join("original.docx")).unwrap(), docx_bytes);
    // import_trash recognizes original.docx inside the segment just like original.pdf.
    let things =
        solstone_core_import::import_trash::import_things(tree.journal(), "20260410_093000")
            .expect("import_things finds published office import");
    let uploads = solstone_core_import::import_trash::kept_uploads(tree.journal(), &things);
    assert_eq!(
        solstone_core_import::import_trash::kept_originals(tree.journal(), &things) - uploads,
        1
    );
    let transcript = fs::read_to_string(seg_dir.join("document_transcript.md")).unwrap();
    assert!(transcript.contains("# Project Brief Title"), "{transcript}");
    assert!(transcript.contains("**Type:** Document"), "{transcript}");
    assert!(
        transcript.contains("Pure Rust extraction works without PDFium."),
        "{transcript}"
    );
    assert!(!transcript.contains("**Pages:**"), "{transcript}");

    // Re-importing identical bytes (even renamed to .docm) skips as already_imported.
    let renamed = tree.pdf("brief-copy.docm", &docx_bytes);
    File::options()
        .write(true)
        .open(&renamed)
        .unwrap()
        .set_times(FileTimes::new().set_modified(now_preview + Duration::from_secs(172_800)))
        .unwrap();
    let dedup = preview_size(
        DocumentPreviewRequest {
            source: &renamed,
            password: None,
            now: now_import,
            zone: zone.clone(),
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_093000"),
        },
        tree.journal(),
        false,
        &PanickingPdfWorker,
    )
    .unwrap();
    assert_eq!(dedup.recordings.len(), 0);
    assert_eq!(dedup.skipped, 1);

    // keep_original = false writes document_source.json without original.*, and still deduplicates.
    let no_orig_tree = TestTree::new();
    let no_orig_src = no_orig_tree.pdf("no-orig.docx", &docx_bytes);
    File::options()
        .write(true)
        .open(&no_orig_src)
        .unwrap()
        .set_times(FileTimes::new().set_modified(now_preview + Duration::from_secs(172_800)))
        .unwrap();
    let no_orig_import_dir = no_orig_tree.journal().join("imports/20260410_094500");
    fs::create_dir_all(&no_orig_import_dir).unwrap();
    let mut no_orig_meta = solstone_core_import::ImportMetadata::new();
    no_orig_meta.insert("source".to_owned(), serde_json::json!("document"));
    solstone_core_import::originals::record_new_import(&mut no_orig_meta);
    fs::write(
        no_orig_import_dir.join("import.json"),
        serde_json::to_vec(&no_orig_meta).unwrap(),
    )
    .unwrap();
    solstone_core_import::originals::set_preference(no_orig_tree.journal(), "document", false)
        .unwrap();
    solstone_core_import::originals::capture_choice(
        no_orig_tree.journal(),
        "20260410_094500",
        "document",
    )
    .unwrap();
    let no_orig_res = import(
        DocumentImportRequest {
            source: &no_orig_src,
            journal_root: no_orig_tree.journal(),
            import_dir: &no_orig_import_dir,
            import_id: "20260410_094500",
            revision: None,
            password: None,
            force: false,
            now: now_import,
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_094500"),
        },
        &PanickingPdfWorker,
        &PanickingModel,
        &FakePublication::default(),
    );
    assert_eq!(no_orig_res.entries_written, 1, "{:?}", no_orig_res.errors);
    let no_orig_seg = segment_dir(&no_orig_tree, &no_orig_res);
    assert!(!no_orig_seg.join("original.docx").exists());
    assert!(no_orig_seg.join("document_source.json").is_file());
    let no_orig_dedup = preview_size(
        DocumentPreviewRequest {
            source: &no_orig_src,
            password: None,
            now: now_import,
            zone: solstone_core_journal_config::owner_zone(no_orig_tree.journal()),
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_094500"),
        },
        no_orig_tree.journal(),
        false,
        &PanickingPdfWorker,
    )
    .unwrap();
    assert_eq!(no_orig_dedup.recordings.len(), 0);
    assert_eq!(no_orig_dedup.skipped, 1);

    // Encrypted and blank Office docs refuse honestly without invoking PDFium.
    let encrypted = tree.pdf("locked.xlsx", &build_test_encrypted_ooxml());
    let enc_prep = prepare_document_import(
        &DocumentImportRequest {
            source: &encrypted,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "office-enc",
            revision: None,
            password: None,
            force: false,
            now: now_import,
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_093500"),
        },
        &PanickingPdfWorker,
        &PanickingModel,
    );
    assert!(enc_prep.items.is_empty());
    assert!(
        enc_prep
            .hard_failures
            .iter()
            .any(|msg| msg.contains("password-protected document cannot be imported")),
        "{:?}",
        enc_prep.hard_failures
    );

    let blank = tree.pdf("blank.docx", &build_test_docx("", None));
    let blank_prep = prepare_document_import(
        &DocumentImportRequest {
            source: &blank,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "office-blank",
            revision: None,
            password: None,
            force: false,
            now: now_import,
            owner_timestamp: None,
            fallback_timestamp: Some("20260410_093600"),
        },
        &PanickingPdfWorker,
        &PanickingModel,
    );
    assert!(blank_prep.items.is_empty());
    assert!(
        blank_prep
            .hard_failures
            .iter()
            .any(|msg| msg.contains("no extractable text")),
        "{:?}",
        blank_prep.hard_failures
    );

    // Mixed directory (PDF + DOCX): has_pdf is true, PDF uses worker, DOCX uses doc-extract.
    let mixed = tree.root().join("mixed-docs");
    fs::create_dir_all(&mixed).unwrap();
    fs::write(mixed.join("a.pdf"), b"%PDF-1.4 mixed").unwrap();
    fs::write(mixed.join("b.docx"), &docx_bytes).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, mixed.join("c-link.docx")).unwrap();
    assert!(solstone_core_import_sources::document::has_pdf(&mixed));
    let worker = FakeWorker::new(vec![Ok(payload(vec![page(
        1,
        80,
        0.0,
        Some("PDF text inside mixed directory with plenty of characters.\n"),
    )]))]);
    let mixed_prep = prepare_document_import(
        &DocumentImportRequest {
            source: &mixed,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "office-mixed",
            revision: None,
            password: None,
            force: false,
            now: now_import,
            owner_timestamp: None,
            fallback_timestamp: None,
        },
        &worker,
        &PanickingModel,
    );
    // Symlinked c-link.docx is ignored; a.pdf and b.docx both prepare cleanly.
    assert_eq!(mixed_prep.items.len(), 2, "{:?}", mixed_prep.errors);
    assert_eq!(worker.requests().len(), 1);
    assert!(
        mixed_prep
            .items
            .iter()
            .any(|i| i.original_name == "original.pdf")
    );
    assert!(
        mixed_prep
            .items
            .iter()
            .any(|i| i.original_name == "original.docx")
    );
}

#[test]
fn epub_odf_eml_auto_detect_while_strava_and_ambiguous_structured_files_require_explicit_source() {
    let tree = TestTree::new();
    let now = UNIX_EPOCH + Duration::from_secs(1_776_000_000);

    // 1. Strava single CSV, localized Strava CSV, Strava export directory (activities.csv + profile.csv),
    //    Fireflies CSV, and unhinted table.csv / page.html / granola.csv must NOT auto-detect as Document.
    let strava_dir = tree.journal().join("strava-export");
    fs::create_dir_all(&strava_dir).unwrap();
    let strava_en = strava_dir.join("activities.csv");
    fs::write(
        &strava_en,
        "Activity ID,Activity Date,Activity Name,Activity Type\n1,2026-04-01,Morning Run,Run\n",
    )
    .unwrap();
    fs::write(
        strava_dir.join("profile.csv"),
        "Athlete ID,First Name\n1,Alex\n",
    )
    .unwrap();
    assert!(!solstone_core_import_sources::document::detect(&strava_en));
    assert!(!solstone_core_import_sources::document::detect(&strava_dir));

    let strava_fr_dir = tree.journal().join("strava-fr");
    fs::create_dir_all(&strava_fr_dir).unwrap();
    let strava_fr = strava_fr_dir.join("activities.csv");
    fs::write(&strava_fr, "ID de l'activité,Date\n1,2026-04-01\n").unwrap();
    assert!(!solstone_core_import_sources::document::detect(&strava_fr));
    assert!(!solstone_core_import_sources::document::detect(
        &strava_fr_dir
    ));

    for (name, body) in [
        (
            "fireflies.csv",
            "Speaker,Start Time,End Time,Sentence\nMira,00:00:01,00:00:03,Hi\n",
        ),
        ("table.csv", "item,qty\nbolts,12\n"),
        ("table.tsv", "item\tqty\nbolts\t12\n"),
        (
            "page.html",
            "<!DOCTYPE html><html><body><p>Hi</p></body></html>",
        ),
        (
            "granola.csv",
            "id,title,summary,notes,transcript\n1,Sync,S,N,Hi\n",
        ),
    ] {
        let p = tree.pdf(name, body.as_bytes());
        assert!(
            !solstone_core_import_sources::document::detect(&p),
            "{name} must not auto-detect as Document"
        );
    }

    // 2. EPUB, ODT, and EML auto-detect and import cleanly without PDFium or model calls.
    let epub_bytes = build_test_zip(&[
        ("mimetype", b"application/epub+zip"),
        (
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#,
        ),
        (
            "content.opf",
            br#"<?xml version="1.0"?><package><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Book Title</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
        ),
        (
            "c1.xhtml",
            b"<html><body><p>Chapter text inside manual EPUB import.</p></body></html>",
        ),
    ]);
    let epub_path = tree.pdf("book.epub", &epub_bytes);
    assert!(solstone_core_import_sources::document::detect(&epub_path));
    assert!(!solstone_core_import_sources::document::has_pdf(&epub_path));

    let epub_res = import(
        DocumentImportRequest {
            source: &epub_path,
            journal_root: tree.journal(),
            import_dir: tree.import_dir(),
            import_id: "epub-manual",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: Some("20260501_100000"),
            fallback_timestamp: None,
        },
        &PanickingPdfWorker,
        &PanickingModel,
        &FakePublication::default(),
    );
    assert_eq!(epub_res.entries_written, 1, "{:?}", epub_res.errors);
    let epub_seg = segment_dir(&tree, &epub_res);
    assert!(
        fs::read_to_string(epub_seg.join("document_transcript.md"))
            .unwrap()
            .contains("Chapter text inside manual EPUB import.")
    );
    assert!(epub_seg.join("original.epub").is_file());

    // 3. Explicit Document import accepts general .csv cleanly without PDFium/model.
    let csv_path = tree.pdf("budget.csv", b"Category,Amount\nHosting,420\n");
    let csv_tree = TestTree::new();
    let csv_res = import(
        DocumentImportRequest {
            source: &csv_path,
            journal_root: csv_tree.journal(),
            import_dir: csv_tree.import_dir(),
            import_id: "csv-manual",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: Some("20260502_110000"),
            fallback_timestamp: None,
        },
        &PanickingPdfWorker,
        &PanickingModel,
        &FakePublication::default(),
    );
    assert_eq!(csv_res.entries_written, 1, "{:?}", csv_res.errors);
    let csv_seg = segment_dir(&csv_tree, &csv_res);
    assert!(
        fs::read_to_string(csv_seg.join("document_transcript.md"))
            .unwrap()
            .contains("| Hosting | 420 |")
    );

    // 4. RTF auto-detects and imports cleanly without PDFium or model calls.
    let rtf_path = tree.pdf(
        "memo.rtf",
        br"{\rtf1\ansi{\info{\title RTF Note}}Manual RTF body\par}",
    );
    assert!(solstone_core_import_sources::document::detect(&rtf_path));
    assert!(!solstone_core_import_sources::document::has_pdf(&rtf_path));
    let rtf_tree = TestTree::new();
    let rtf_res = import(
        DocumentImportRequest {
            source: &rtf_path,
            journal_root: rtf_tree.journal(),
            import_dir: rtf_tree.import_dir(),
            import_id: "rtf-manual",
            revision: None,
            password: None,
            force: false,
            now,
            owner_timestamp: Some("20260503_120000"),
            fallback_timestamp: None,
        },
        &PanickingPdfWorker,
        &PanickingModel,
        &FakePublication::default(),
    );
    assert_eq!(rtf_res.entries_written, 1, "{:?}", rtf_res.errors);
    let rtf_seg = segment_dir(&rtf_tree, &rtf_res);
    assert!(
        fs::read_to_string(rtf_seg.join("document_transcript.md"))
            .unwrap()
            .contains("Manual RTF body")
    );
    assert!(rtf_seg.join("original.rtf").is_file());
}
