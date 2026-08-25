// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::fs;
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
use solstone_core_import::{CreatedSegment, PublicationOperations, observe_source_immutability};
use solstone_core_import_sources::document::{
    DocumentImportRequest, DocumentModelClient, PdfCommand, PdfMetadata, PdfPage, PdfPayload,
    PdfWorker, PdfWorkerRequest, SystemPdfWorker, WorkerFailure, import,
};
use solstone_core_indexer_store::scan::RescanFileStatus;
use solstone_core_segment::{StreamAdvance, UnboundStreamAdvanceError};

#[cfg(unix)]
const PDF_WORKER_STDOUT_MAX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(unix)]
const PDF_WORKER_STDERR_MAX_BYTES: usize = 64 * 1024;
#[cfg(unix)]
const WORKER_SUCCESS_RESPONSE: &str =
    r#"{"schema":"sol-pdf/1","engine":"test","page_count":1,"pages":[{"index":1,"text":"text"}]}"#;

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
    fs::File::open(&source)
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
    fs::File::open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(claimed_time))
        .unwrap();
    let expected_day = chrono::DateTime::<chrono::Local>::from(claimed_time)
        .format("%Y%m%d")
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
    let segment_dir = fs::read_dir(stream_dir)
        .unwrap()
        .next()
        .expect("partial document segment")
        .unwrap()
        .path();
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
    let segment_dir = fs::read_dir(
        tree.journal()
            .join("chronicle")
            .join(day)
            .join("import.document"),
    )
    .unwrap()
    .next()
    .expect("partial document segment")
    .unwrap()
    .path();
    assert_eq!(
        fs::read(segment_dir.join("original.pdf")).unwrap(),
        b"source"
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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(5));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(5));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(5));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(5));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(5));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(30));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(30));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(5));

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
    let worker = SystemPdfWorker::new(script, Duration::from_secs(5));
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
fn document_second_pass_failure_creates_no_segment_directory() {
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
    assert!(!tree.journal().join("chronicle").exists());
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
    fn rescan_file(&self, _: &Path, _: &Path) -> Result<RescanFileStatus, String> {
        Ok(RescanFileStatus::Declined)
    }
    fn touch_stream_health_marker(&self, journal: &Path, day: &str) -> Result<(), String> {
        let call = {
            let mut days = self.days.lock().unwrap();
            days.push(day.to_owned());
            days.len()
        };
        if self.sabotage_first_raster && call == 1 {
            let stream_dir = journal.join("chronicle").join(day).join("import.document");
            let segment_dir = fs::read_dir(stream_dir)
                .map_err(|error| error.to_string())?
                .next()
                .ok_or_else(|| "missing document segment".to_owned())?
                .map_err(|error| error.to_string())?
                .path();
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
        let root =
            std::env::temp_dir().join(format!("solstone-document-test-{}", std::process::id()));
        let unique = root.with_extension(format!(
            "{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
