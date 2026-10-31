// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::foreground::PreparedImport;
use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use solstone_core_import::{read_import_metadata, write_import_metadata};
use solstone_core_import_sources::document::{
    PdfCommand, PdfMetadata, PdfPage, PdfPayload, PdfWorker, PdfWorkerRequest, WorkerFailure,
};
use std::fs;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;
use tower::ServiceExt;

const SAVED: &str = "20200102_030405";

struct Pdf {
    date: Option<&'static str>,
    failure: bool,
    extracted: Arc<AtomicUsize>,
}

impl PdfWorker for Pdf {
    fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        if request.command == PdfCommand::Extract {
            self.extracted.fetch_add(1, Ordering::SeqCst);
            if self.failure {
                return Err(WorkerFailure::Cancelled);
            }
        }
        let text = "a saved document stays connected to its publication. ".repeat(30);
        Ok(PdfPayload {
            schema: "sol-pdf/1".into(),
            engine: "fixture".into(),
            page_count: 1,
            metadata: PdfMetadata {
                creation_date: self.date.map(str::to_owned),
                ..Default::default()
            },
            pages: vec![PdfPage {
                index: 1,
                chars: text.len(),
                text: Some(text),
                ..Default::default()
            }],
            ..Default::default()
        })
    }
}

struct Finished(mpsc::Sender<()>);
impl Drop for Finished {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

struct NoReindex;
impl solstone_core_import_sources::archive::FullReindexRequester for NoReindex {
    fn request_full_reindex(&self) -> Result<bool, String> {
        panic!("a PDF import must not request archive reindexing")
    }
}

async fn request(router: &Router, method: &str, path: &str, body: Value) -> (u16, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn saved_pdf_start_keeps_execution_publication_and_progress_on_its_record() {
    if super::tests::in_isolated_child_exact(
        "import_route::pdf_identity_tests::saved_pdf_start_keeps_execution_publication_and_progress_on_its_record",
    ) {
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // Undated PDF, PDF metadata date, explicit owner date, and cancellation.
    for (date, owner, failure) in [
        (None, None, false),
        (Some("2024-06-15T09:10:11Z"), None, false),
        (Some("2024-06-15T09:10:11Z"), Some("20230504_050607"), false),
        (None, None, true),
    ] {
        let extracted = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&extracted);
        let (finished, done) = mpsc::channel();
        let (root, server, host) = super::tests::fixture(Arc::new(move |context| {
            let mut seams = context.seams_with_pdf(Box::new(Pdf {
                date,
                failure: failure && !context.arguments().iter().any(|arg| arg == "--dry-run"),
                extracted: Arc::clone(&observed),
            }));
            seams.model = Box::new(solstone_core_import_sources::NullDocumentModelClient);
            seams.reindex = Box::new(NoReindex);
            let prepared = PreparedImport::new(context.arguments().to_vec(), seams);
            Ok(
                if context.arguments().iter().any(|arg| arg == "--dry-run") {
                    prepared
                } else {
                    prepared.with_resources(Finished(finished.clone()))
                },
            )
        }));
        let (status, detail, records) = runtime.block_on(async {
            let slot = server.import_route();
            let router = solstone_core_import_web::routes_with_embedded(
                root.path().to_path_buf(), Arc::clone(&slot),
            );
            let upload = "--pdf\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nsaved-pdf\r\n--pdf\r\nContent-Disposition: form-data; name=\"file\"; filename=\"ordinary.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.1\nfixture\r\n--pdf--\r\n";
            let response = router.clone().oneshot(Request::post("/app/import/api/save")
                .header("content-type", "multipart/form-data; boundary=pdf")
                .body(Body::from(upload)).unwrap()).await.unwrap();
            assert_eq!(response.status(), 200);
            let saved: Value = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
            let generated = saved["timestamp"].as_str().unwrap();
            // Fix the fixture's upload clock before Preview, without waiting for a second tick.
            let mut metadata = read_import_metadata(root.path(), generated).unwrap();
            solstone_core_import::relocate_import(root.path(), generated, SAVED).unwrap();
            let path = root.path().join("imports").join(SAVED).join("ordinary.pdf");
            metadata.insert("file_path".into(), json!(path));
            for key in ["import_id", "source_timestamp", "user_timestamp"] {
                metadata.insert(key.into(), json!(SAVED));
            }
            metadata.remove(solstone_core_import::saved_start::SAVED_START_KEY);
            solstone_core_import::saved_start::initialize_saved_start(&mut metadata, SAVED).unwrap();
            // Seed the fixture clock and witness together. The public metadata writer
            // deliberately preserves the original witness and cannot re-date a saved row.
            fs::write(root.path().join("imports").join(SAVED).join("import.json"),
                serde_json::to_vec(&metadata).unwrap()).unwrap();
            let mut data = json!({"path": path, "timestamp": owner.unwrap_or(SAVED), "import_id": SAVED});
            if owner.is_some() { data["timestamp_override"] = json!(true); }
            let (status, preview) = request(&router, "POST", "/app/import/api/preview", data.clone()).await;
            assert_eq!(status, 200, "{preview}");
            let preview_extractions = extracted.load(Ordering::SeqCst);
            data["preview_token"] = preview["preview_token"].clone();
            data["first_start_only"] = json!(true);

            // A stale preview is refused without consuming the saved permission or extracting.
            let before = read_import_metadata(root.path(), SAVED).unwrap();
            let mut changed = before.clone();
            changed.insert("setting".into(), json!("changed after preview"));
            write_import_metadata(root.path(), SAVED, &changed).unwrap();
            let (status, refusal) = request(&router, "POST", "/app/import/api/start", data.clone()).await;
            assert_eq!(status, 409, "{refusal}");
            assert_eq!(extracted.load(Ordering::SeqCst), preview_extractions);
            assert_eq!(solstone_core_import::saved_start::saved_start_state(
                &read_import_metadata(root.path(), SAVED).unwrap(), SAVED),
                solstone_core_import::saved_start::SavedStartState::Eligible);
            write_import_metadata(root.path(), SAVED, &before).unwrap();
            while done.try_recv().is_ok() {}

            let (status, started) = request(&router, "POST", "/app/import/api/start", data.clone()).await;
            assert_eq!(status, 200, "{started}");
            assert_eq!(started["import_id"], owner.unwrap_or(SAVED));
            done.recv_timeout(Duration::from_secs(10)).unwrap();
            let executed = extracted.load(Ordering::SeqCst);
            // Refuse the consumed first Start while the route is still available.
            let (status, refusal) = request(&router, "POST", "/app/import/api/start", data).await;
            assert_eq!(status, 400, "{refusal}");
            assert_eq!(refusal["reason_code"], "invalid_operation_for_state");
            assert_eq!(extracted.load(Ordering::SeqCst), executed);
            slot.retire().unwrap(); // Join the retained execution after its resource has dropped.
            let id = owner.unwrap_or(SAVED);
            let (status, detail) = request(&router, "GET", &format!("/app/import/api/{id}"), Value::Null).await;
            let records: Vec<_> = fs::read_dir(root.path().join("imports")).unwrap()
                .map(Result::unwrap).filter(|entry| solstone_core_import::validate_timestamp(&entry.file_name().to_string_lossy()).is_ok())
                .map(|entry| entry.file_name()).collect();
            for record in &records {
                let projection = solstone_core_import::project_import_result(root.path(), &record.to_string_lossy());
                eprintln!("record={record:?} status={:?} publication={:?}", projection.status, projection.raw_publication);
            }
            eprintln!("PDF identity control: date={date:?} owner={owner:?} failure={failure} extracted={} records={records:?} detail={detail}", extracted.load(Ordering::SeqCst));
            assert!(extracted.load(Ordering::SeqCst) > preview_extractions,
                "execution must reach the PDF worker after Preview");
            (status, detail, records)
            });
        host.shutdown().unwrap();
        server.shutdown_for_retirement().unwrap();
        assert!(
            extracted.load(Ordering::SeqCst) > 0,
            "execution must reach the PDF worker"
        );
        assert_eq!(status, 200);
        assert_eq!(detail["import_json"]["client_item_id"], "saved-pdf");
        assert_eq!(detail["saved_start"]["state"], "consumed");
        assert_eq!(records.len(), 1, "execution must stay on the saved record");
        if failure {
            assert_eq!(detail["status"], "failed", "{detail}");
        } else {
            assert_eq!(detail["status"], "success", "{detail}");
            assert_eq!(detail["thinking"]["status"], "current");
            assert_eq!(detail["thinking"]["recordings"], 1);
            if let Some(day) = owner.map(|stamp| &stamp[..8]).or(date.map(|_| "20240615")) {
                assert_eq!(detail["imported_json"]["segments"][0]["day"], day);
            }
        }
    }
}
