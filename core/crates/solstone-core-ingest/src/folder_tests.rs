// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_import_sources::document::{PdfMetadata, PdfPage, PdfPayload};
use solstone_core_ingest_resolve::{IngestNotice, IngestNotifier};
use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
use tower::ServiceExt;

use crate::folder::{FolderPdfReader, PdfReadError};
use crate::router::api_router_with_pdf;
use crate::validation::PROTOCOL_HEADER;

const LAPTOP: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

type RecordedNotice = (String, String, String, Vec<String>);

#[derive(Default)]
struct RecordingNotifier {
    notices: Mutex<Vec<RecordedNotice>>,
    fail_next: std::sync::atomic::AtomicBool,
    block_next_baseline: Mutex<Option<std::path::PathBuf>>,
}

impl RecordingNotifier {
    fn count(&self) -> usize {
        self.notices.lock().unwrap().len()
    }

    fn take(&self) -> Vec<RecordedNotice> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }
}

impl IngestNotifier for RecordingNotifier {
    fn notify(
        &self,
        notice: &IngestNotice<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self
            .fail_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err("synthetic one-time notification interruption".into());
        }
        if let Some(target) = self.block_next_baseline.lock().unwrap().take() {
            fs::create_dir_all(target)?;
        }
        self.notices.lock().unwrap().push((
            notice.source.to_owned(),
            notice.day.to_owned(),
            notice.segment.to_owned(),
            notice.files.to_vec(),
        ));
        Ok(())
    }
}

struct FakePdfReader {
    result: Mutex<Result<PdfPayload, PdfReadError>>,
}

impl FakePdfReader {
    fn ok(text: &str) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(Ok(PdfPayload {
                schema: "sol-pdf/1".into(),
                engine: "fake".into(),
                sha256: String::new(),
                page_count: 1,
                encrypted: false,
                metadata: PdfMetadata {
                    title: Some("Sample Report".into()),
                    ..PdfMetadata::default()
                },
                pages: vec![PdfPage {
                    index: 1,
                    chars: text.len(),
                    text: Some(text.to_owned()),
                    ..PdfPage::default()
                }],
                render: None,
                warnings: Vec::new(),
            })),
        })
    }

    fn encrypted() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(Ok(PdfPayload {
                page_count: 1,
                encrypted: true,
                ..PdfPayload::default()
            })),
        })
    }

    fn unavailable() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(Err(PdfReadError::Unavailable("missing binary".into()))),
        })
    }
}

impl FolderPdfReader for FakePdfReader {
    fn extract(&self, _journal_root: &Path, _bytes: &[u8]) -> Result<PdfPayload, PdfReadError> {
        self.result.lock().unwrap().clone()
    }
}

fn journal() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    let mut ledger = AuthorizationLedger::new(dir.path());
    ledger
        .add(ClientEntry::new(
            LAPTOP,
            "laptop",
            "2026-11-01T00:00:00Z",
            "instance-1",
            ClientRole::Roleless,
        ))
        .unwrap();
    for (slug, title, muted) in [
        ("work", "Work", false),
        ("personal", "Personal", false),
        ("hidden", "Hidden", true),
    ] {
        solstone_core_facets::create_facet(dir.path(), slug, title, "", "#2f6fdd", "", None)
            .unwrap();
        if muted {
            solstone_core_facets::set_facet_muted(dir.path(), slug, true).unwrap();
        }
    }
    dir
}

fn fixed_now_ms() -> Arc<dyn Fn() -> i64 + Send + Sync> {
    // 2026-11-27T22:00:00Z -> 20261127 in UTC and Mountain Time.
    let ts = solstone_core_time::parse_rfc3339("2026-11-27T22:00:00Z")
        .unwrap()
        .timestamp()
        .as_millisecond();
    Arc::new(move || ts)
}

fn test_app(
    root: &Path,
    notifier: Arc<RecordingNotifier>,
    pdf_reader: Arc<dyn FolderPdfReader>,
) -> Router {
    api_router_with_pdf(root, notifier, fixed_now_ms(), pdf_reader)
}

fn png_bytes(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb([shade, 10, 20]))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn build_multipart(envelope: &Value, parts: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "----FolderBoundaryTest123456";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"envelope\"\r\n\r\n");
    body.extend_from_slice(serde_json::to_string(envelope).unwrap().as_bytes());
    body.extend_from_slice(b"\r\n");
    for (filename, bytes) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn linked_basis() -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(LAPTOP).unwrap(),
        leaf_spki: vec![0x30, 0x00],
    }
}

async fn post_folder(
    app: &Router,
    day: &str,
    segment: &str,
    folder_meta: Value,
    lines: &[Value],
    companions: &[(&str, &[u8])],
) -> (StatusCode, Value) {
    let mut activity = String::new();
    for line in lines {
        activity.push_str(&serde_json::to_string(line).unwrap());
        activity.push('\n');
    }
    let activity_bytes = activity.into_bytes();
    let activity_name = format!("folder_{}.jsonl", sha256(&activity_bytes));
    let mut file_entries = vec![json!({ "submitted": activity_name })];
    let mut all_parts: Vec<(&str, &[u8])> = vec![(&activity_name, &activity_bytes)];
    for (name, bytes) in companions {
        file_entries.push(json!({ "submitted": name }));
        all_parts.push((name, bytes));
    }
    let envelope = json!({
        "day": day,
        "segment": segment,
        "source": "folder",
        "files": file_entries,
        "meta": { "folder": folder_meta }
    });
    let (content_type, body) = build_multipart(&envelope, &all_parts);
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/app/devices/ingest")
        .header(PROTOCOL_HEADER, "3")
        .header("content-type", content_type)
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(linked_basis());
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let raw = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let parsed: Value =
        serde_json::from_slice(&raw).unwrap_or(json!({ "raw": String::from_utf8_lossy(&raw) }));
    (status, parsed)
}

async fn call_json(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let raw_body = body
        .map(|v| serde_json::to_vec(&v).unwrap())
        .unwrap_or_default();
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(PROTOCOL_HEADER, "3");
    if !raw_body.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let mut req = builder.body(Body::from(raw_body)).unwrap();
    req.extensions_mut().insert(linked_basis());
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn find_segment_dir(day_dir: &Path, segment: &str) -> std::path::PathBuf {
    fs::read_dir(day_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(segment))
        .find(|path| path.is_dir())
        .unwrap_or_else(|| {
            panic!(
                "expected segment directory {segment} under {}",
                day_dir.display()
            )
        })
}

#[tokio::test]
async fn activity_only_upload_indexes_screen_text_with_zero_notifications_and_respects_owner_facet()
{
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    let app = test_app(dir.path(), notifier.clone(), FakePdfReader::ok("unused"));

    let (status, body) = post_folder(
        &app,
        "20261127",
        "140000_300",
        json!({
            "v": 1,
            "id": "downloads-1",
            "name": "Downloads",
            "facet": "work",
            "kinds": []
        }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "installer.dmg",
            "at": "2026-11-27T14:02:10-07:00",
            "observed_at": "2026-11-27T14:02:12-07:00",
            "file_type": "dmg",
            "size": 10_485_760,
            "platform": {
                "origin_url": "https://releases.example/app.dmg?token=supersecretvalue",
                "downloaded_by": "Firefox"
            }
        })],
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert!(body.get("refused_files").is_none());
    assert_eq!(
        notifier.count(),
        0,
        "activity-only must emit zero observe notices"
    );

    // Find the created segment directory.
    let day_dir = dir.path().join("chronicle/20261127");
    let seg_dir = find_segment_dir(&day_dir, "140000_300");
    assert!(solstone_core_segment::is_activity_only_folder_dir(&seg_dir));

    let screen_text = fs::read_to_string(seg_dir.join("talents/screen_text.md")).unwrap();
    assert!(
        screen_text.contains("## 02:10 · Folder: Downloads"),
        "{screen_text}"
    );
    assert!(
        screen_text.contains("Added installer\\.dmg"),
        "{screen_text}"
    );
    assert!(screen_text.contains("via Firefox"), "{screen_text}");
    assert!(
        !screen_text.contains("supersecretvalue"),
        "secret redacted: {screen_text}"
    );

    let facets: Value =
        serde_json::from_slice(&fs::read(seg_dir.join("talents/facets.json")).unwrap()).unwrap();
    assert_eq!(facets, json!([{ "facet": "work", "level": "high" }]));

    // Resend returns duplicate; erasing the segment does NOT resurrect it.
    fs::remove_dir_all(&seg_dir).unwrap();
    let (status2, body2) = post_folder(
        &app,
        "20261127",
        "140000_300",
        json!({ "v": 1, "id": "downloads-1", "name": "Downloads", "facet": "work", "kinds": [] }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "installer.dmg",
            "at": "2026-11-27T14:02:10-07:00",
            "observed_at": "2026-11-27T14:02:12-07:00",
            "file_type": "dmg",
            "size": 10_485_760,
            "platform": {
                "origin_url": "https://releases.example/app.dmg?token=supersecretvalue",
                "downloaded_by": "Firefox"
            }
        })],
        &[],
    )
    .await;
    assert_eq!(status2, StatusCode::OK, "{body2}");
    assert_eq!(body2["status"], "duplicate");
    assert!(
        !seg_dir.exists(),
        "erased segment must stay erased on resend"
    );
}

#[tokio::test]
async fn per_file_checking_refuses_broken_file_by_name_while_rest_of_window_lands() {
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    let app = test_app(dir.path(), notifier.clone(), FakePdfReader::ok("unused"));

    let note_bytes = b"# Quick Note\n\nMet about folder source.\n";
    let broken_png = b"not a real png at all";
    let (status, body) = post_folder(
        &app,
        "20261127",
        "140500_300",
        json!({
            "v": 1,
            "id": "desktop-1",
            "name": "Desktop",
            "facet": null,
            "kinds": ["notes", "screenshots"]
        }),
        &[
            json!({
                "v": 1,
                "event": "added",
                "path": "notes/quick.md",
                "at": "2026-11-27T14:05:10-07:00",
                "observed_at": "2026-11-27T14:05:12-07:00",
                "file_type": "md",
                "size": note_bytes.len(),
                "content_hash": sha256(note_bytes),
                "part": "c_01.md",
                "kind_hint": null
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "broken.png",
                "at": "2026-11-27T14:06:00-07:00",
                "observed_at": "2026-11-27T14:06:02-07:00",
                "file_type": "png",
                "size": broken_png.len(),
                "content_hash": sha256(broken_png),
                "part": "c_02.png",
                "kind_hint": "screenshots"
            }),
        ],
        &[("c_01.md", note_bytes), ("c_02.png", broken_png)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    let refused = body["refused_files"]
        .as_array()
        .expect("refused_files present");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["path"], "broken.png");
    assert_eq!(refused[0]["part"], "c_02.png");
    assert_eq!(refused[0]["reason_code"], "folder_file_invalid");
    assert_eq!(notifier.count(), 1);

    let day_dir = dir.path().join("chronicle/20261127");
    let seg_dir = find_segment_dir(&day_dir, "140500_300");
    let note_md = fs::read_to_string(seg_dir.join("note_transcript.md")).unwrap();
    assert!(note_md.contains("Met about folder source."), "{note_md}");
    assert!(
        !seg_dir.join("talents/facets.json").exists(),
        "no owner facet -> whole-journal only"
    );

    // Upload containing ONLY a broken file returns 400 folder_file_invalid.
    let (status_bad, body_bad) = post_folder(
        &app,
        "20261127",
        "141000_300",
        json!({ "v": 1, "id": "desktop-1", "name": "Desktop", "kinds": ["screenshots"] }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "corrupt.png",
            "at": "2026-11-27T14:10:10-07:00",
            "observed_at": "2026-11-27T14:10:12-07:00",
            "content_hash": sha256(broken_png),
            "part": "c_01.png",
            "kind_hint": "screenshots"
        })],
        &[("c_01.png", broken_png)],
    )
    .await;
    assert_eq!(status_bad, StatusCode::BAD_REQUEST, "{body_bad}");
    assert_eq!(body_bad["reason_code"], "folder_file_invalid");
    assert_eq!(body_bad["refused_files"][0]["path"], "corrupt.png");
}

#[tokio::test]
async fn kind_hint_routes_screenshots_photos_transcripts_and_notes_net_changes_and_documents_revisions()
 {
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    let app = test_app(
        dir.path(),
        notifier.clone(),
        FakePdfReader::ok("Page one body text."),
    );

    // 1. Screenshot + Photo + PDF + Transcript in one window.
    let shot1 = png_bytes(16, 9, 10);
    let shot2 = png_bytes(16, 9, 20);
    let photo = png_bytes(16, 9, 30);
    let pdf1 = b"%PDF-1.4\n1 0 obj<<>>endobj\n%%EOF\n";
    let transcript = b"[00:00:05] Alice: Welcome to the call.\n[00:00:12] Bob: Glad to be here.\n";

    let (status, body) = post_folder(
        &app,
        "20261127",
        "100000_300",
        json!({
            "v": 1,
            "id": "mix-1",
            "name": "Mixed",
            "facet": "work",
            "kinds": ["notes", "documents", "transcripts", "screenshots", "photos"]
        }),
        &[
            json!({
                "v": 1,
                "event": "added",
                "path": "shot1.png",
                "at": "2026-11-27T10:00:10-07:00",
                "observed_at": "2026-11-27T10:00:11-07:00",
                "content_hash": sha256(&shot1),
                "part": "c_01.png",
                "kind_hint": "screenshots"
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "shot2.png",
                "at": "2026-11-27T10:00:20-07:00",
                "observed_at": "2026-11-27T10:00:21-07:00",
                "content_hash": sha256(&shot2),
                "part": "c_02.png",
                "kind_hint": "screenshots"
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "cam.png",
                "at": "2026-11-27T10:00:30-07:00",
                "observed_at": "2026-11-27T10:00:31-07:00",
                "content_hash": sha256(&photo),
                "part": "c_03.png",
                "kind_hint": "photos"
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "report.pdf",
                "at": "2026-11-27T10:01:00-07:00",
                "observed_at": "2026-11-27T10:01:01-07:00",
                "content_hash": sha256(pdf1),
                "part": "c_04.pdf",
                "kind_hint": "documents"
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "call.txt",
                "at": "2026-11-27T10:01:30-07:00",
                "observed_at": "2026-11-27T10:01:31-07:00",
                "content_hash": sha256(transcript),
                "part": "c_05.txt",
                "kind_hint": "unknown_future_hint"
            }),
        ],
        &[
            ("c_01.png", &shot1),
            ("c_02.png", &shot2),
            ("c_03.png", &photo),
            ("c_04.pdf", pdf1),
            ("c_05.txt", transcript),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("refused_files").is_none(), "{body}");

    let day_dir = dir.path().join("chronicle/20261127");
    let seg_dir = find_segment_dir(&day_dir, "100000_300");
    let stream_dir = seg_dir.parent().unwrap().to_path_buf();
    assert!(seg_dir.join("screenshot_screen.png").is_file());
    assert!(
        seg_dir
            .join(format!("screenshot_{}_screen.png", &sha256(&shot2)[..12]))
            .is_file()
    );
    assert!(
        seg_dir
            .join(format!("photo_{}.png", &sha256(&photo)[..12]))
            .is_file()
    );
    let doc_md = fs::read_to_string(seg_dir.join("document_transcript.md")).unwrap();
    assert!(doc_md.contains("Page one body text."), "{doc_md}");
    let tr_jsonl = fs::read_to_string(seg_dir.join("folder_transcript.jsonl")).unwrap();
    assert!(tr_jsonl.contains("Welcome to the call."), "{tr_jsonl}");

    // 2. Document revision in a later window is kept separately; same bytes is duplicate.
    let pdf2 = b"%PDF-1.4\n2 0 obj<<>>endobj\n%%EOF\n";
    let (status_rev, body_rev) = post_folder(
        &app,
        "20261127",
        "110500_300",
        json!({ "v": 1, "id": "mix-1", "name": "Mixed", "facet": "work", "kinds": ["documents"] }),
        &[json!({
            "v": 1,
            "event": "changed",
            "path": "report.pdf",
            "at": "2026-11-27T11:05:10-07:00",
            "observed_at": "2026-11-27T11:05:12-07:00",
            "content_hash": sha256(pdf2),
            "part": "c_01.pdf",
            "kind_hint": "documents"
        })],
        &[("c_01.pdf", pdf2)],
    )
    .await;
    assert_eq!(status_rev, StatusCode::OK, "{body_rev}");
    assert_eq!(body_rev["status"], "ok");
    assert!(
        stream_dir
            .join("110500_300/document_transcript.md")
            .is_file()
    );

    // 3. Note first-sight vs net-change edit in next window.
    let note_v1 = b"Line one.\nLine two.\n";
    let note_v2 = b"Line one.\nLine two.\nAdded third line.\n";
    let (s1, _) = post_folder(
        &app,
        "20261127",
        "120000_300",
        json!({ "v": 1, "id": "mix-1", "name": "Mixed", "kinds": ["notes"] }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "todo.md",
            "at": "2026-11-27T12:00:10-07:00",
            "observed_at": "2026-11-27T12:00:11-07:00",
            "content_hash": sha256(note_v1),
            "part": "c_01.md",
            "kind_hint": "notes"
        })],
        &[("c_01.md", note_v1)],
    )
    .await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, _) = post_folder(
        &app,
        "20261127",
        "120500_300",
        json!({ "v": 1, "id": "mix-1", "name": "Mixed", "kinds": ["notes"] }),
        &[json!({
            "v": 1,
            "event": "changed",
            "path": "todo.md",
            "at": "2026-11-27T12:05:10-07:00",
            "observed_at": "2026-11-27T12:05:11-07:00",
            "content_hash": sha256(note_v2),
            "part": "c_01.md",
            "kind_hint": "notes"
        })],
        &[("c_01.md", note_v2)],
    )
    .await;
    assert_eq!(s2, StatusCode::OK);
    let edit_md = fs::read_to_string(stream_dir.join("120500_300/note_transcript.md")).unwrap();
    assert!(edit_md.contains("Added third line."), "{edit_md}");
    assert!(
        !edit_md.contains("Line one."),
        "net-change must not repeat unchanged lines: {edit_md}"
    );
}

#[tokio::test]
async fn encrypted_pdf_refused_and_unavailable_worker_returns_retryable() {
    let dir = journal();
    let pdf = b"%PDF-1.4\n%%EOF\n";

    let app_enc = test_app(
        dir.path(),
        Arc::new(RecordingNotifier::default()),
        FakePdfReader::encrypted(),
    );
    let (s_enc, b_enc) = post_folder(
        &app_enc,
        "20261127",
        "100000_300",
        json!({ "v": 1, "id": "pdf-1", "name": "Docs", "kinds": ["documents"] }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "locked.pdf",
            "at": "2026-11-27T10:00:10-07:00",
            "observed_at": "2026-11-27T10:00:11-07:00",
            "content_hash": sha256(pdf),
            "part": "c_01.pdf",
            "kind_hint": "documents"
        })],
        &[("c_01.pdf", pdf)],
    )
    .await;
    assert_eq!(s_enc, StatusCode::BAD_REQUEST, "{b_enc}");
    assert_eq!(b_enc["reason_code"], "folder_file_invalid");

    let app_down = test_app(
        dir.path(),
        Arc::new(RecordingNotifier::default()),
        FakePdfReader::unavailable(),
    );
    let (s_down, b_down) = post_folder(
        &app_down,
        "20261127",
        "100000_300",
        json!({ "v": 1, "id": "pdf-1", "name": "Docs", "kinds": ["documents"] }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "locked.pdf",
            "at": "2026-11-27T10:00:10-07:00",
            "observed_at": "2026-11-27T10:00:11-07:00",
            "content_hash": sha256(pdf),
            "part": "c_01.pdf",
            "kind_hint": "documents"
        })],
        &[("c_01.pdf", pdf)],
    )
    .await;
    assert_eq!(s_down, StatusCode::SERVICE_UNAVAILABLE, "{b_down}");
    assert_eq!(b_down["status"], "retryable");
}

#[tokio::test]
async fn supported_facets_estimate_segments_listing_and_future_day_guard() {
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    let app = test_app(dir.path(), notifier.clone(), FakePdfReader::ok("ok"));

    // 1. Supported list.
    let (s_sup, sup) = call_json(
        &app,
        Method::GET,
        "/app/devices/ingest/folder/supported",
        None,
    )
    .await;
    assert_eq!(s_sup, StatusCode::OK);
    assert_eq!(
        sup["ruleset_version"],
        solstone_core_import::triage::RULESET_VERSION
    );
    assert_eq!(sup["window_seconds"], 300);
    let doc_exts: Vec<&str> = sup["kinds"]["documents"]["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(doc_exts.contains(&"pdf"));
    for ext in solstone_core_doc_extract::SUPPORTED_EXTENSIONS {
        assert!(doc_exts.contains(ext), "missing Office ext {ext}");
    }
    let tr_exts: Vec<&str> = sup["kinds"]["transcripts"]["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(tr_exts.contains(&"vtt"));
    assert!(tr_exts.contains(&"srt"));
    assert_eq!(sup["kinds"]["other"]["mode"], "metadata_only");

    // 2. Facets list omits muted facet.
    let (s_fac, fac) =
        call_json(&app, Method::GET, "/app/devices/ingest/folder/facets", None).await;
    assert_eq!(s_fac, StatusCode::OK);
    let names: Vec<&str> = fac["facets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    assert_eq!(names, vec!["personal", "work"]);

    // 3. Estimate: activity-only items cost 0 thoughts; content + screenshots cost thoughts.
    let (s_est, est) = call_json(
        &app,
        Method::POST,
        "/app/devices/ingest/folder/estimate",
        Some(json!({
            "folder_id": "dl-1",
            "items": [
                { "path": "a.dmg", "at": "2026-11-20T09:01:00-07:00", "file_type": "dmg", "has_content": false },
                { "path": "b.md", "at": "2026-11-21T10:02:00-07:00", "file_type": "md", "kind_hint": "notes", "has_content": true },
                { "path": "s.png", "at": "2026-11-21T10:03:00-07:00", "file_type": "png", "kind_hint": "screenshots", "has_content": true }
            ]
        })),
    )
    .await;
    assert_eq!(s_est, StatusCode::OK, "{est}");
    assert_eq!(est["items"], 3);
    assert_eq!(est["content_items"], 2);
    assert_eq!(est["activity_only_items"], 1);
    assert_eq!(est["segments"], 2);
    assert_eq!(est["content_segments"], 1);
    assert_eq!(est["activity_only_segments"], 1);
    assert_eq!(est["days"], 2);
    assert_eq!(est["thoughts"], 2); // 1 content segment (~1 thought) + 1 screenshot vision pass
    assert_eq!(est["backfill_paused"], false);

    // 4. Two distinct folders in the same 5-minute window both land and list under ?source=folder.
    for (id, name) in [("folder-a", "Downloads"), ("folder-b", "Desktop")] {
        let (st, b) = post_folder(
            &app,
            "20261127",
            "150000_300",
            json!({ "v": 1, "id": id, "name": name, "kinds": [] }),
            &[json!({
                "v": 1,
                "event": "added",
                "path": format!("{id}.bin"),
                "at": "2026-11-27T15:01:00-07:00",
                "observed_at": "2026-11-27T15:01:05-07:00"
            })],
            &[],
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{b}");
    }
    let (s_seg, segs) = call_json(
        &app,
        Method::GET,
        "/app/devices/ingest/segments/20261127?source=folder",
        None,
    )
    .await;
    assert_eq!(s_seg, StatusCode::OK, "{segs}");
    assert_eq!(segs["total"], 2, "{segs}");

    // 5. Known sources surfaces both watched folders keyed by folder_id,
    //    and joins declared folder state with observed last_synced_at.
    let snap = crate::known_sources::snapshot(dir.path(), Some(LAPTOP.to_owned())).unwrap();
    let snap_val = serde_json::to_value(&snap).unwrap();
    let sources = snap_val["devices"][0]["sources"].as_array().unwrap();
    assert!(
        sources
            .iter()
            .any(|s| s["source"] == "folder" && s["detail"] == "folder-a")
    );
    assert!(
        sources
            .iter()
            .any(|s| s["source"] == "folder" && s["detail"] == "folder-b")
    );
    solstone_core_sol_link::source_declarations::replace(
        dir.path(),
        LAPTOP,
        vec![
            solstone_core_sol_link::source_declarations::Source {
                source: "folder".to_owned(),
                state: solstone_core_sol_link::source_declarations::State::Paused,
                detail: Some("folder-a".to_owned()),
            },
            solstone_core_sol_link::source_declarations::Source {
                source: "folder".to_owned(),
                state: solstone_core_sol_link::source_declarations::State::NeedsAttention,
                detail: Some("folder-b".to_owned()),
            },
        ],
        "2026-11-27T15:05:00Z".to_owned(),
    )
    .unwrap();
    let declared_snap =
        crate::known_sources::snapshot(dir.path(), Some(LAPTOP.to_owned())).unwrap();
    let declared_val = serde_json::to_value(&declared_snap).unwrap();
    let declared_sources = declared_val["devices"][0]["sources"].as_array().unwrap();
    let row_a = declared_sources
        .iter()
        .find(|s| s["source"] == "folder" && s["detail"] == "folder-a")
        .expect("declared folder-a");
    assert_eq!(row_a["origin"], "declared");
    assert_eq!(row_a["state"], "paused");
    assert!(row_a["last_synced_at"].is_string());
    let row_b = declared_sources
        .iter()
        .find(|s| s["source"] == "folder" && s["detail"] == "folder-b")
        .expect("declared folder-b");
    assert_eq!(row_b["origin"], "declared");
    assert_eq!(row_b["state"], "needs_attention");
    assert!(row_b["last_synced_at"].is_string());

    // 6. Future day refused with 409 folder_ahead.
    let (s_ahead, b_ahead) = post_folder(
        &app,
        "20261129",
        "090000_300",
        json!({ "v": 1, "id": "folder-a", "name": "Downloads", "kinds": [] }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "future.txt",
            "at": "2026-11-29T09:01:00-07:00",
            "observed_at": "2026-11-29T09:01:01-07:00"
        })],
        &[],
    )
    .await;
    assert_eq!(s_ahead, StatusCode::CONFLICT, "{b_ahead}");
    assert_eq!(b_ahead["reason_code"], "folder_ahead");
    assert_eq!(notifier.take().len(), 0);
}

#[tokio::test]
async fn each_published_folder_golden_envelope_is_admitted_as_sent() {
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    let app = test_app(dir.path(), notifier, FakePdfReader::ok("unused"));

    let note_bytes: &[u8] = b"# Meeting notes\n\nFollow up with Sam tomorrow.\n";
    let png_bytes: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x10,
        0x50, 0x30, 0x00, 0x00, 0x00, 0xa4, 0x00, 0x61, 0x34, 0x66, 0x7d, 0x72, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let transcript_bytes: &[u8] =
        b"[00:00:05] Alice: Let's review the roadmap.\n[00:00:12] Bob: Sounds good.\n";

    type GoldenPayload<'a> = (&'a [Value], Vec<(&'a str, &'a [u8])>);
    let payloads: Vec<GoldenPayload<'_>> = vec![
        (
            Box::leak(Box::new([json!({
                "at": "2026-11-27T14:02:15-07:00",
                "content_hash": null,
                "created_at": "2026-11-27T14:02:14-07:00",
                "event": "added",
                "file_type": "dmg",
                "kind_hint": null,
                "modified_at": "2026-11-27T14:02:15-07:00",
                "observed_at": "2026-11-27T14:02:18-07:00",
                "part": null,
                "path": "archive/installer.dmg",
                "platform": {
                    "downloaded_by": "Firefox",
                    "origin_url": "https://releases.example/app.dmg",
                    "referrer_url": "https://releases.example/downloads"
                },
                "previous_path": null,
                "size": 52_428_800,
                "v": 1
            })])),
            vec![],
        ),
        (
            Box::leak(Box::new([
                json!({
                    "at": "2026-11-27T14:05:10-07:00",
                    "content_hash": sha256(note_bytes),
                    "event": "added",
                    "file_type": "md",
                    "kind_hint": null,
                    "observed_at": "2026-11-27T14:05:12-07:00",
                    "part": "c_01.md",
                    "path": "notes/meeting.md",
                    "previous_path": null,
                    "size": note_bytes.len(),
                    "v": 1
                }),
                json!({
                    "at": "2026-11-27T14:06:22-07:00",
                    "content_hash": sha256(png_bytes),
                    "event": "added",
                    "file_type": "png",
                    "kind_hint": "screenshots",
                    "observed_at": "2026-11-27T14:06:24-07:00",
                    "part": "c_02.png",
                    "path": "Screenshots/screen-2026-11-27.png",
                    "platform": { "screenshot_provenance": "xdg_screenshots" },
                    "previous_path": null,
                    "size": png_bytes.len(),
                    "v": 1
                }),
            ])),
            vec![("c_01.md", note_bytes), ("c_02.png", png_bytes)],
        ),
        (
            Box::leak(Box::new([
                json!({
                    "at": "2026-11-27T14:10:05-07:00",
                    "content_hash": sha256(transcript_bytes),
                    "event": "added",
                    "file_type": "txt",
                    "kind_hint": "transcripts",
                    "observed_at": "2026-11-27T14:10:08-07:00",
                    "part": "c_01.txt",
                    "path": "calls/roadmap.txt",
                    "previous_path": null,
                    "size": transcript_bytes.len(),
                    "v": 1
                }),
                json!({
                    "at": "2026-11-27T14:12:00-07:00",
                    "content_hash": null,
                    "event": "renamed",
                    "file_type": "zip",
                    "kind_hint": null,
                    "observed_at": "2026-11-27T14:12:02-07:00",
                    "part": null,
                    "path": "archive/bundle-v2.zip",
                    "previous_path": "bundle.zip",
                    "size": 1_048_576,
                    "v": 1
                }),
                json!({
                    "at": "2026-11-27T14:14:30-07:00",
                    "content_hash": null,
                    "event": "removed",
                    "file_type": null,
                    "kind_hint": null,
                    "observed_at": "2026-11-27T14:14:32-07:00",
                    "part": null,
                    "path": "scratch.tmp",
                    "previous_path": null,
                    "size": null,
                    "v": 1
                }),
            ])),
            vec![("c_01.txt", transcript_bytes)],
        ),
    ];

    let golden = include_str!(
        "../../../../docs/openapi/client-ingest-contract/fixtures/folder-golden.jsonl"
    );
    let golden_lines: Vec<&str> = golden.lines().collect();
    assert_eq!(golden_lines.len(), payloads.len());

    for (line, (activity_lines, companions)) in golden_lines.into_iter().zip(payloads) {
        let envelope: Value = serde_json::from_str(line).unwrap();
        let mut activity_text = String::new();
        for ev in activity_lines {
            activity_text.push_str(&serde_json::to_string(ev).unwrap());
            activity_text.push('\n');
        }
        let activity_bytes = activity_text.into_bytes();
        let expected_name = format!("folder_{}.jsonl", sha256(&activity_bytes));
        assert_eq!(
            envelope["files"][0]["submitted"].as_str(),
            Some(expected_name.as_str()),
            "golden filename must equal activity SHA-256"
        );
        let mut all_parts: Vec<(&str, &[u8])> = vec![(&expected_name, &activity_bytes)];
        all_parts.extend(companions);
        let (content_type, body) = build_multipart(&envelope, &all_parts);
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/app/devices/ingest")
            .header(PROTOCOL_HEADER, "3")
            .header("content-type", content_type)
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut().insert(linked_basis());
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let raw = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(status, StatusCode::OK, "{line} -> {parsed}");
        assert_eq!(parsed["status"], "ok", "{parsed}");
        assert_eq!(parsed["segment"], envelope["segment"]);
        assert!(parsed.get("refused_files").is_none(), "{parsed}");
    }
}

#[tokio::test]
async fn scanned_and_mixed_folder_pdfs_read_text_via_local_ocr_and_retry_on_ocr_failure() {
    use solstone_core_import_sources::document::{
        PdfPageOcr, PdfPageOcrError, PdfWorker, PdfWorkerRequest, WorkerFailure,
    };

    struct MixedScanWorker;
    impl PdfWorker for MixedScanWorker {
        fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
            let bytes = fs::read(&request.source).unwrap();
            let is_pure_scan = bytes.windows(9).any(|w| w == b"PURE_SCAN");
            if let Some(render) = &request.render {
                let mut pages = Vec::new();
                for &idx in &render.pages {
                    let name = format!("page-{idx:04}.png");
                    fs::write(render.render_dir.join(&name), format!("png-{idx}")).unwrap();
                    pages.push(PdfPage {
                        index: idx,
                        rendered: Some(name),
                        ..PdfPage::default()
                    });
                }
                return Ok(PdfPayload {
                    schema: "sol-pdf/1".to_owned(),
                    engine: "test".to_owned(),
                    page_count: if is_pure_scan { 1 } else { 2 },
                    pages,
                    ..PdfPayload::default()
                });
            }
            if is_pure_scan {
                Ok(PdfPayload {
                    schema: "sol-pdf/1".to_owned(),
                    engine: "test".to_owned(),
                    page_count: 1,
                    pages: vec![PdfPage {
                        index: 1,
                        chars: 0,
                        width_pt: 612.0,
                        height_pt: 792.0,
                        image_area_fraction: 1.0,
                        text: Some(String::new()),
                        ..PdfPage::default()
                    }],
                    ..PdfPayload::default()
                })
            } else {
                Ok(PdfPayload {
                    schema: "sol-pdf/1".to_owned(),
                    engine: "test".to_owned(),
                    page_count: 2,
                    pages: vec![
                        PdfPage {
                            index: 1,
                            chars: 28,
                            width_pt: 612.0,
                            height_pt: 792.0,
                            image_area_fraction: 0.0,
                            text: Some("Typed contract cover page.".to_owned()),
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
                    ],
                    ..PdfPayload::default()
                })
            }
        }
    }

    struct StubOcr {
        fail: bool,
    }
    impl PdfPageOcr for StubOcr {
        fn recognize_png(&mut self, png: &[u8]) -> Result<String, PdfPageOcrError> {
            if self.fail {
                return Err(PdfPageOcrError::Failed("helper timeout".to_owned()));
            }
            Ok(format!("OCR text from {}", String::from_utf8_lossy(png)))
        }
    }

    struct LocalOcrFolderReader {
        fail_ocr: bool,
    }
    impl FolderPdfReader for LocalOcrFolderReader {
        fn extract(&self, journal_root: &Path, bytes: &[u8]) -> Result<PdfPayload, PdfReadError> {
            let mut ocr = StubOcr {
                fail: self.fail_ocr,
            };
            crate::folder::extract_folder_pdf_with(journal_root, bytes, &MixedScanWorker, &mut ocr)
        }
    }

    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    let app = test_app(
        dir.path(),
        notifier,
        Arc::new(LocalOcrFolderReader { fail_ocr: false }),
    );

    let mixed_pdf = b"%PDF-1.4 MIXED_DOC\n%%EOF\n";
    let pure_scan_pdf = b"%PDF-1.4 PURE_SCAN\n%%EOF\n";
    let (status, body) = post_folder(
        &app,
        "20261127",
        "150000_300",
        json!({ "v": 1, "id": "scans-1", "name": "Scans", "kinds": ["documents"] }),
        &[
            json!({
                "v": 1,
                "event": "added",
                "path": "mixed.pdf",
                "at": "2026-11-27T15:00:10-07:00",
                "observed_at": "2026-11-27T15:00:11-07:00",
                "content_hash": sha256(mixed_pdf),
                "part": "c_01.pdf",
                "kind_hint": "documents"
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "receipt_scan.pdf",
                "at": "2026-11-27T15:01:00-07:00",
                "observed_at": "2026-11-27T15:01:01-07:00",
                "content_hash": sha256(pure_scan_pdf),
                "part": "c_02.pdf",
                "kind_hint": "documents"
            }),
        ],
        &[("c_01.pdf", mixed_pdf), ("c_02.pdf", pure_scan_pdf)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("refused_files").is_none(), "{body}");

    let seg_dir = find_segment_dir(&dir.path().join("chronicle/20261127"), "150000_300");
    let doc_md = fs::read_to_string(seg_dir.join("document_transcript.md")).unwrap();
    assert!(doc_md.contains("Typed contract cover page."), "{doc_md}");
    assert!(doc_md.contains("OCR text from png-2"), "{doc_md}");
    assert!(doc_md.contains("OCR text from png-1"), "{doc_md}");
    assert!(
        !doc_md.contains("[No selectable text on this page]"),
        "{doc_md}"
    );

    // An installed OCR worker that fails on a pure-scan PDF surfaces retryable 503.
    let failing_app = test_app(
        dir.path(),
        Arc::new(RecordingNotifier::default()),
        Arc::new(LocalOcrFolderReader { fail_ocr: true }),
    );
    let pure_scan_v2 = b"%PDF-1.4 PURE_SCAN_V2\n%%EOF\n";
    let (fail_status, fail_body) = post_folder(
        &failing_app,
        "20261127",
        "151000_300",
        json!({ "v": 1, "id": "scans-1", "name": "Scans", "kinds": ["documents"] }),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "scan_fail.pdf",
            "at": "2026-11-27T15:10:10-07:00",
            "observed_at": "2026-11-27T15:10:11-07:00",
            "content_hash": sha256(pure_scan_v2),
            "part": "c_01.pdf",
            "kind_hint": "documents"
        })],
        &[("c_01.pdf", pure_scan_v2)],
    )
    .await;
    assert_eq!(fail_status, StatusCode::SERVICE_UNAVAILABLE, "{fail_body}");
    assert_eq!(fail_body["status"], "retryable", "{fail_body}");
}

fn stored_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut local = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = local.len() as u32;
        let name_bytes = name.as_bytes();
        let size = data.len() as u32;
        // Minimal CRC32 (IEEE 802.3) so zip crate accepts stored entries.
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in *data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if (crc & 1) != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        crc = !crc;
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name_bytes);
        local.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let cd_offset = local.len() as u32;
    let cd_size = central.len() as u32;
    let count = entries.len() as u16;
    let mut out = local;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn synthetic_docx(body_xml: &str, title: Option<&str>) -> Vec<u8> {
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
    stored_zip_bytes(&refs)
}

fn synthetic_encrypted_ooxml() -> Vec<u8> {
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

#[tokio::test]
async fn office_documents_land_in_folder_ingest_dedupe_estimate_and_isolate_failures() {
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    // Unavailable PDF reader proves Office folder ingest never invokes PDFium.
    let app = test_app(dir.path(), notifier.clone(), FakePdfReader::unavailable());

    let valid_docx = synthetic_docx(
        r#"<w:p><w:r><w:t>Board meeting agenda and quarterly budget.</w:t></w:r></w:p>"#,
        Some("Q4 Agenda"),
    );
    let empty_docx = synthetic_docx("", None);
    let encrypted_xlsx = synthetic_encrypted_ooxml();

    let folder_meta = json!({
        "v": 1,
        "id": "office-folder",
        "name": "Work Docs",
        "facet": "work",
        "kinds": ["documents"]
    });

    // 1. Mixed window (unhinted): valid DOCX lands while encrypted XLSX and empty DOCX
    //    are refused per-file without failing the upload or returning 503.
    let (status, body) = post_folder(
        &app,
        "20261127",
        "100000_300",
        folder_meta.clone(),
        &[
            json!({
                "v": 1,
                "event": "added",
                "path": "reports/agenda.docx",
                "at": "2026-11-27T10:01:00-07:00",
                "observed_at": "2026-11-27T10:01:02-07:00",
                "content_hash": sha256(&valid_docx),
                "part": "c_01.docx"
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "reports/locked.xlsx",
                "at": "2026-11-27T10:02:00-07:00",
                "observed_at": "2026-11-27T10:02:02-07:00",
                "content_hash": sha256(&encrypted_xlsx),
                "part": "c_02.xlsx"
            }),
            json!({
                "v": 1,
                "event": "added",
                "path": "reports/blank.docx",
                "at": "2026-11-27T10:03:00-07:00",
                "observed_at": "2026-11-27T10:03:02-07:00",
                "content_hash": sha256(&empty_docx),
                "part": "c_03.docx"
            }),
        ],
        &[
            ("c_01.docx", valid_docx.as_slice()),
            ("c_02.xlsx", encrypted_xlsx.as_slice()),
            ("c_03.docx", empty_docx.as_slice()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok", "{body}");
    let refused = body["refused_files"].as_array().expect("refused_files");
    assert_eq!(refused.len(), 2, "{body}");
    assert!(refused.iter().any(|r| {
        r["path"] == "reports/locked.xlsx"
            && r["detail"]
                .as_str()
                .is_some_and(|d| d.contains("password-protected"))
    }));
    assert!(refused.iter().any(|r| {
        r["path"] == "reports/blank.docx"
            && r["detail"]
                .as_str()
                .is_some_and(|d| d.contains("no extractable text"))
    }));

    let seg_dir = find_segment_dir(&dir.path().join("chronicle/20261127"), "100000_300");
    let transcript = fs::read_to_string(seg_dir.join("document_transcript.md"))
        .expect("document_transcript.md written");
    assert!(transcript.contains("# Q4 Agenda"), "{transcript}");
    assert!(transcript.contains("**Format:** docx"), "{transcript}");
    assert!(
        transcript.contains("Board meeting agenda and quarterly budget."),
        "{transcript}"
    );

    // 2. Duplicate valid DOCX upload returns duplicate.
    let (s_dup, b_dup) = post_folder(
        &app,
        "20261127",
        "100000_300",
        folder_meta.clone(),
        &[json!({
            "v": 1,
            "event": "added",
            "path": "reports/agenda.docx",
            "at": "2026-11-27T10:01:00-07:00",
            "observed_at": "2026-11-27T10:01:02-07:00",
            "content_hash": sha256(&valid_docx),
            "part": "c_01.docx"
        })],
        &[("c_01.docx", valid_docx.as_slice())],
    )
    .await;
    assert_eq!(s_dup, StatusCode::OK, "{b_dup}");
    assert_eq!(b_dup["status"], "duplicate", "{b_dup}");

    // 3. Estimate counts Office docs without kind_hint as documents and deduplicates
    //    the already-received DOCX via folder_id + content_hash.
    let (s_est, est) = call_json(
        &app,
        Method::POST,
        "/app/devices/ingest/folder/estimate",
        Some(json!({
            "folder_id": "office-folder",
            "items": [
                {
                    "path": "reports/agenda.docx",
                    "at": "2026-11-27T10:01:00-07:00",
                    "file_type": "docx",
                    "content_hash": sha256(&valid_docx),
                    "has_content": true
                },
                {
                    "path": "reports/new-deck.pptx",
                    "at": "2026-11-27T11:00:00-07:00",
                    "file_type": "pptx",
                    "has_content": true
                }
            ]
        })),
    )
    .await;
    assert_eq!(s_est, StatusCode::OK, "{est}");
    assert_eq!(est["already_in_journal"], 1, "{est}");
    assert_eq!(est["counts_by_kind"]["documents"], 1, "{est}");

    // 4. Refused-only window returns 400 BAD_REQUEST with folder_file_invalid (never 503).
    let (s_bad, b_bad) = post_folder(
        &app,
        "20261127",
        "100500_300",
        folder_meta,
        &[json!({
            "v": 1,
            "event": "added",
            "path": "reports/only-locked.xlsx",
            "at": "2026-11-27T10:06:00-07:00",
            "observed_at": "2026-11-27T10:06:02-07:00",
            "content_hash": sha256(&encrypted_xlsx),
            "part": "c_01.xlsx"
        })],
        &[("c_01.xlsx", encrypted_xlsx.as_slice())],
    )
    .await;
    assert_eq!(s_bad, StatusCode::BAD_REQUEST, "{b_bad}");
    assert_eq!(b_bad["reason_code"], "folder_file_invalid", "{b_bad}");
}

#[tokio::test]
async fn folder_text_decodes_utf16_before_checking_nuls_and_stamps_delivery_on_arrival() {
    let dir = journal();
    let app = test_app(dir.path(), Arc::default(), FakePdfReader::ok("unused"));
    let bytes: Vec<_> = [0xff, 0xfe]
        .into_iter()
        .chain("café".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    for (id, kind, filename) in [
        ("one", "notes", "note_transcript.md"),
        ("two", "documents", "document_transcript.md"),
    ] {
        let (status, result) = post_folder(&app, "20261127", "120000_300", json!({"v":1,"id":id,"name":id,"kinds":[kind]}),
            &[json!({"v":1,"event":"added","path":"letter.txt","at":"2026-11-27T12:01:00Z","observed_at":"2026-11-27T12:02:00Z",
                   "content_hash":sha256(&bytes),"kind_hint":kind,"part":"content.txt"})], &[("content.txt", &bytes)]).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        let folders = crate::folder::list_synced_folders(dir.path()).unwrap();
        let synced = &folders.iter().find(|f| f.id == id).unwrap().devices[LAPTOP];
        assert_eq!(
            synced.parse::<jiff::Timestamp>().unwrap().as_millisecond(),
            fixed_now_ms()()
        );
        let file = fs::read_dir(dir.path().join("chronicle/20261127"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("120000_300").join(filename))
            .find(|p| p.is_file())
            .unwrap();
        assert!(fs::read_to_string(file).unwrap().contains("café"));
    }
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn folder_erasure_follows_version_across_folder_ids_while_other_copies_and_later_edits_land()
{
    let dir = journal();
    let app = test_app(dir.path(), Arc::default(), FakePdfReader::ok("unused"));
    let contents = b"the first version";
    let meta = |id: &str| json!({"v":1,"id":id,"name":id,"kinds":["notes"]});
    let events = |bytes: &[u8]| {
        [
            json!({"v":1,"event":"added","path":"plan.md","at":"2026-11-27T12:01:00Z",
               "observed_at":"2026-11-27T12:02:00Z","content_hash":sha256(bytes),"kind_hint":"notes","part":"content.md"}),
            json!({"v":1,"event":"added","path":"download.bin","at":"2026-11-27T12:01:00Z","observed_at":"2026-11-27T12:02:00Z","size":10,"content_hash":sha256(b"stale hash")}),
        ]
    };
    let day = "20261127";
    let key = "120000_300";
    let day_dir = dir.path().join("chronicle").join(day);
    assert_eq!(
        post_folder(
            &app,
            day,
            key,
            meta("first"),
            &events(contents),
            &[("content.md", contents)]
        )
        .await
        .1["status"],
        "ok"
    );
    let original = find_segment_dir(&day_dir, key);
    let persisted = original.join("talents/folder_activity.json");
    let mut legacy_activity: Value =
        serde_json::from_slice(&fs::read(&persisted).unwrap()).unwrap();
    let details = legacy_activity["events"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["path"] == "download.bin")
        .unwrap();
    assert!(details["content_hash"].is_null());
    // Exercise an older journal that persisted a client-supplied hash without bytes.
    details["content_hash"] = json!(sha256(b"stale hash"));
    fs::write(persisted, serde_json::to_vec(&legacy_activity).unwrap()).unwrap();
    let stream = original
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        post_folder(
            &app,
            day,
            key,
            meta("independent"),
            &events(contents),
            &[("content.md", contents)]
        )
        .await
        .1["status"],
        "ok"
    );
    let baseline = dir
        .path()
        .join(solstone_core_import::folder_erasure::note_relative_path(
            "first", "plan.md",
        ));
    let independent = dir
        .path()
        .join(solstone_core_import::folder_erasure::note_relative_path(
            "independent",
            "plan.md",
        ));
    assert!(baseline.is_file());
    let owner = solstone_core_transcripts_web::router(
        dir.path().into(),
        solstone_core_transcripts_web::Clock::system(),
        || axum::response::Response::new(Body::empty()),
    );
    let (status, result) = call_json(
        &owner,
        Method::DELETE,
        &format!("/app/transcripts/api/segment/{day}/{stream}/{key}"),
        Some(json!({"erase":true,"backups":false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["erased"], true, "{result}");
    assert!(!baseline.exists(), "erased note comparison text survived");
    assert!(
        independent.is_file(),
        "another folder still owns its own copy"
    );
    let mut replay = events(contents);
    replay[1]["event"] = json!("changed");
    let retry = post_folder(
        &app,
        day,
        key,
        meta("reinstalled"),
        &replay,
        &[("content.md", contents)],
    )
    .await;
    assert_eq!(retry.1["status"], "duplicate", "{}", retry.1);
    let mut names_only = replay[0].clone();
    names_only.as_object_mut().unwrap().remove("part");
    names_only.as_object_mut().unwrap().remove("content_hash");
    let retry = post_folder(&app, day, key, meta("names-only"), &[names_only], &[]).await;
    assert_eq!(retry.1["status"], "duplicate", "{}", retry.1);
    let mut prior_details = replay[1].clone();
    prior_details["kind_hint"] = json!("documents");
    prior_details["part"] = json!("later.txt");
    prior_details["content_hash"] = json!(sha256(b"1234567890"));
    let identical_metadata = post_folder(
        &app,
        day,
        key,
        meta("formerly-names-only"),
        &[prior_details],
        &[("later.txt", b"1234567890")],
    )
    .await;
    assert_eq!(
        identical_metadata.1["status"], "duplicate",
        "{}",
        identical_metadata.1
    );
    let changed = b"the second version";
    let next = post_folder(
        &app,
        day,
        key,
        meta("reinstalled"),
        &events(changed),
        &[("content.md", changed)],
    )
    .await;
    assert_eq!(next.1["status"], "ok", "{}", next.1);
}

#[tokio::test]
async fn unreadable_folder_document_is_preserved_when_another_arrives_in_its_window() {
    let dir = journal();
    let app = test_app(dir.path(), Arc::default(), FakePdfReader::ok("unused"));
    let meta = json!({"v":1, "id":"letters", "name":"letters", "kinds":["documents"]});
    let line = |name: &str, bytes: &[u8]| {
        json!({
            "v":1, "event":"added", "path":name, "at":"2026-11-27T12:00:55Z",
            "observed_at":"2026-11-27T12:01:00Z", "kind_hint":"documents",
            "part":"doc.txt", "content_hash":sha256(bytes)
        })
    };
    let first = b"first letter";
    let later = b"another letter";
    let (status, _) = post_folder(
        &app,
        "20261127",
        "120000_300",
        meta.clone(),
        &[line("a.txt", first)],
        &[("doc.txt", first)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let segment = find_segment_dir(&dir.path().join("chronicle/20261127"), "120000_300");
    let doc = segment.join("document_transcript.md");
    fs::write(&doc, [0xff, 0xff]).unwrap();
    let (status, result) = post_folder(
        &app,
        "20261127",
        "120000_300",
        meta,
        &[line("b.txt", later)],
        &[("doc.txt", later)],
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
    assert_eq!(fs::read(doc).unwrap(), [0xff, 0xff]);
}

#[tokio::test]
async fn folder_retry_after_notification_failure_preserves_interleaved_edits_once() {
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    notifier
        .fail_next
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let app = test_app(dir.path(), notifier, FakePdfReader::ok("unused"));
    let meta = json!({"v":1,"id":"letters","name":"letters","kinds":["documents"]});
    let line = |name: &str, bytes: &[u8]| {
        json!({
            "v":1,"event":"added","path":name,"at":"2026-11-27T12:01:00Z","observed_at":"2026-11-27T12:01:01Z",
            "kind_hint":"documents","content_hash":sha256(bytes),"part":"content.txt"
        })
    };
    for (name, bytes, expected) in [
        (
            "first.txt",
            &b"first paragraph"[..],
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        ("second.txt", &b"second paragraph"[..], StatusCode::OK),
        ("first.txt", &b"first paragraph"[..], StatusCode::OK),
    ] {
        let (status, body) = post_folder(
            &app,
            "20261127",
            "120000_300",
            meta.clone(),
            &[line(name, bytes)],
            &[("content.txt", bytes)],
        )
        .await;
        assert_eq!(status, expected, "{body}");
    }
    let segment = find_segment_dir(&dir.path().join("chronicle/20261127"), "120000_300");
    let doc = fs::read_to_string(segment.join("document_transcript.md")).unwrap();
    assert_eq!(doc.matches("first paragraph").count(), 1, "{doc}");
    assert_eq!(doc.matches("second paragraph").count(), 1, "{doc}");
}

#[tokio::test]
async fn folder_declared_transcript_uses_its_observed_layout_and_refuses_unrelated_tables() {
    let dir = journal();
    let app = test_app(dir.path(), Arc::default(), FakePdfReader::ok("unused"));
    let spoken = b"start\tend\ttext\n0\t120\tAlmost done.\n121\t480\tReady now.\n";
    let srt = b"1\n00:00:05,400 --> 00:00:08,500\nA timed subtitle.\n\n2\n00:00:09,000 --> 00:00:11,000\nAnother line.\n";
    let table = b"fruit\tprice\napple\t1\n";
    let line = |name: &str, bytes: &[u8]| {
        json!({"v":1,"event":"added","path":name,
        "at":"2026-11-27T12:01:00Z","observed_at":"2026-11-27T12:01:00Z",
        "content_hash":sha256(bytes),"kind_hint":"transcripts","part":name})
    };
    let (status, result) = post_folder(
        &app,
        "20261127",
        "120000_300",
        json!({"v":1,"id":"captions","name":"captions","kinds":["transcripts"]}),
        &[
            line("talk.tsv", spoken),
            line("prices.tsv", table),
            line("cues.srt", srt),
        ],
        &[
            ("talk.tsv", spoken),
            ("prices.tsv", table),
            ("cues.srt", srt),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let refused = result["refused_files"].as_array().unwrap();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["path"], "prices.tsv");
    let segment = find_segment_dir(&dir.path().join("chronicle/20261127"), "120000_300");
    let body = fs::read_to_string(segment.join("folder_transcript.jsonl")).unwrap();
    assert!(body.contains("Ready now."));
    let subtitle = body
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row["text"] == "A timed subtitle.")
        .expect("timed subtitle row");
    assert_eq!(subtitle["cue_start"], "00:00:05.400", "{subtitle}");
}

#[tokio::test]
async fn folder_note_baseline_write_failure_can_be_retried_without_losing_the_next_diff() {
    let dir = journal();
    let notifier = Arc::new(RecordingNotifier::default());
    let baseline = dir
        .path()
        .join(solstone_core_import::folder_erasure::note_relative_path(
            "retry", "note.md",
        ));
    *notifier.block_next_baseline.lock().unwrap() = Some(baseline.clone());
    let app = test_app(dir.path(), notifier, FakePdfReader::ok("unused"));
    let raw = b"first revision";
    let meta = json!({"v":1,"id":"retry","name":"retry","kinds":["notes"]});
    let line = json!({"v":1,"event":"added","path":"note.md","at":"2026-11-27T12:01:00Z","observed_at":"2026-11-27T12:01:00Z",
        "kind_hint":"notes","content_hash":sha256(raw),"part":"note.md"});
    let lines = [line];
    let parts = [("note.md", &raw[..])];
    let first = post_folder(&app, "20261127", "120000_300", meta.clone(), &lines, &parts).await;
    assert_eq!(first.0, StatusCode::INTERNAL_SERVER_ERROR, "{}", first.1);
    fs::remove_dir(&baseline).unwrap();
    let retry = post_folder(&app, "20261127", "120000_300", meta, &lines, &parts).await;
    assert_eq!(retry.1["status"], "ok");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(baseline).unwrap()).unwrap()["content_hash"],
        sha256(raw)
    );
    let dir = find_segment_dir(&dir.path().join("chronicle/20261127"), "120000_300");
    assert_eq!(
        fs::read_to_string(dir.join("note_transcript.md"))
            .unwrap()
            .matches("first revision")
            .count(),
        1
    );
}

#[tokio::test]
async fn folder_retry_screenshot_preserves_the_primary_taken_by_another_arrival() {
    let dir = journal();
    let app = test_app(dir.path(), Arc::default(), FakePdfReader::ok("unused"));
    let meta = json!({"v":1,"id":"pics","name":"pics","kinds":["screenshots","other"]});
    let line = |path: &str| json!({"v":1,"event":"added","path":path,"at":"2026-11-27T12:01:00Z","observed_at":"2026-11-27T12:01:00Z"});
    let seed = post_folder(
        &app,
        "20261127",
        "120000_300",
        meta.clone(),
        &[line("a.bin")],
        &[],
    )
    .await;
    assert_eq!(seed.0, StatusCode::OK, "{}", seed.1);
    let segment = find_segment_dir(&dir.path().join("chronicle/20261127"), "120000_300");
    let primary = segment.join("screenshot_screen.png");
    fs::create_dir(&primary).unwrap();
    let one = png_bytes(8, 8, 1);
    let two = png_bytes(8, 8, 2);
    for (idx, (name, raw, status)) in [
        ("a.png", &one, StatusCode::INTERNAL_SERVER_ERROR),
        ("b.png", &two, StatusCode::OK),
        ("a.png", &one, StatusCode::OK),
    ]
    .into_iter()
    .enumerate()
    {
        let mut event = line(name);
        event["part"] = json!(name);
        event["content_hash"] = json!(sha256(raw));
        event["kind_hint"] = json!("screenshots");
        let response = post_folder(
            &app,
            "20261127",
            "120000_300",
            meta.clone(),
            &[event],
            &[(name, raw)],
        )
        .await;
        assert_eq!(response.0, status, "{}", response.1);
        if idx == 0 {
            fs::remove_dir(&primary).unwrap();
        }
    }
    assert_eq!(fs::read(primary).unwrap(), two);
    assert_eq!(
        fs::read(segment.join(format!("screenshot_{}_screen.png", &sha256(&one)[..12]))).unwrap(),
        one
    );
}
