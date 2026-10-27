// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::owner_effects::{
    assert_native_success, assert_only_saved_record, observe_native_result, owner_json, upload_file,
};
use super::*;
use solstone_core_generate::{
    ClientError, GenerateExecutor, GenerateRequest, GenerateResponse, GeneratedResponse,
};
use std::path::Path;

#[derive(Default)]
struct RecordedGenerate {
    calls: Mutex<Vec<String>>,
}
impl GenerateExecutor for RecordedGenerate {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.calls.lock().unwrap().push(request.context.clone());
        let text = if request.context == "observe.detect.topics" {
            serde_json::json!({"topics":"cobalt owner context","setting":"synthetic conversation"})
                .to_string()
        } else {
            "cobalt ordinary model description".to_owned()
        };
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text,
            model: "deterministic-owner-effect".into(),
            usage: serde_json::json!({}),
            finish_reason: "stop".into(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        })))
    }
}

fn model_fixture() -> (
    tempfile::TempDir,
    Arc<MobileServing>,
    Arc<ForegroundProcessingHost>,
    Arc<RecordedGenerate>,
) {
    let factory: Arc<crate::foreground::ImportSeamFactory> = Arc::new(|context| {
        Ok(PreparedImport::new(
            context.arguments().to_vec(),
            context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker)),
        ))
    });
    model_fixture_with_factory(factory)
}

fn model_fixture_with_factory(
    factory: Arc<crate::foreground::ImportSeamFactory>,
) -> (
    tempfile::TempDir,
    Arc<MobileServing>,
    Arc<ForegroundProcessingHost>,
    Arc<RecordedGenerate>,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/journal.json"),r#"{"setup":{"completed_at":1},"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true}}"#).unwrap();
    let payload = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload");
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
        unreachable!()
    };
    drop(probe);
    let server = Arc::new(
        MobileServing::start_with_payload(root.path().to_path_buf(), address, payload.clone())
            .unwrap(),
    );
    let generator = Arc::new(RecordedGenerate::default());
    let host = Arc::new(
        ForegroundProcessingHost::open_with_imports_and_executor_for_test(
            Arc::clone(&server.root),
            &payload,
            Arc::new(|| false),
            Arc::new(AtomicBool::new(false)),
            server.backup_dependencies(),
            factory,
            generator.clone(),
            false,
            false,
        )
        .unwrap(),
    );
    host.stop_discovery_for_test();
    server
        .import_route()
        .install(Arc::new(ForegroundImportRoute::new(Arc::clone(&host))))
        .unwrap();
    (root, server, host, generator)
}

#[allow(
    clippy::too_many_arguments,
    reason = "Owner fixture names the ordinary upload and expected readable effect."
)]
fn complete_owner_file(
    server: &MobileServing,
    host: &Arc<ForegroundProcessingHost>,
    root: &Path,
    filename: &str,
    source: &str,
    mime: &str,
    bytes: &[u8],
    expected: &str,
) {
    let native = observe_native_result(server, host);
    let saved = upload_file(server, filename, source, mime, bytes);
    let id = saved["timestamp"].as_str().unwrap();
    let original = std::path::PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(fs::read(&original).unwrap(), bytes);
    let mut request = serde_json::json!({"path":saved["path"],"timestamp":saved["source_timestamp"],"import_id":id});
    let preview = owner_json(server, "POST", "/app/import/api/preview", request.clone());
    assert!(preview["preview_token"].is_string(), "{preview}");
    assert!(
        solstone_core_import::get_attempt_facts(
            &solstone_core_import::read_import_metadata(root, id).unwrap()
        )
        .is_none()
    );
    request["preview_token"] = preview["preview_token"].clone();
    owner_json(server, "POST", "/app/import/api/start", request.clone());
    assert_native_success(&native);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let status = owner_json(
            server,
            "GET",
            &format!("/app/import/api/{id}"),
            serde_json::Value::Null,
        );
        if status["status"] == "success" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{status}");
        std::thread::park_timeout(Duration::from_millis(10));
    }
    assert_only_saved_record(root, id);
    assert_eq!(
        owner_json(server, "POST", "/app/import/api/start", request)["replay"],
        true
    );
    assert_eq!(fs::read(&original).unwrap(), bytes);
    let day = &saved["source_timestamp"].as_str().unwrap()[..8];
    let read = owner_json(
        server,
        "GET",
        &format!("/app/transcripts/api/read/{day}?transcripts=1"),
        serde_json::Value::Null,
    );
    assert!(
        read["markdown"].as_str().unwrap().contains(expected),
        "{read}"
    );
    server.index().unwrap(); // Explicit indexing; automatic discovery is not proved.
    let search = owner_json(
        server,
        "GET",
        "/app/search/api/search?q=cobalt",
        serde_json::Value::Null,
    );
    assert!(
        search["total"].as_u64().is_some_and(|count| count > 0),
        "{search}"
    );
    assert!(
        search["days"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|day| day["results"].as_array().unwrap())
            .any(|hit| hit["day"] == day
                && hit["text"].as_str().is_some_and(|text| text
                    .replace("<strong>", "")
                    .replace("</strong>", "")
                    .contains(expected))),
        "{search}"
    );
}

#[test]
fn ordinary_text_uses_same_host_generate_and_preserves_source_words() {
    if in_isolated_child(
        "model_effects::ordinary_text_uses_same_host_generate_and_preserves_source_words",
    ) {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (root, server, host, generator) = model_fixture();
    complete_owner_file(
        &server,
        &host,
        root.path(),
        "conversation.txt",
        "",
        "text/plain",
        b"## 00:00:00\n**Ana Lima:** cobalt ordinary source words\n",
        "cobalt ordinary source words",
    );
    assert_eq!(
        *generator.calls.lock().unwrap(),
        vec!["observe.detect.topics"]
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

fn fixture_png() -> Vec<u8> {
    vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 2,
        0, 0, 0, 75, 109, 41, 220, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 112, 12, 42, 198,
        138, 24, 134, 150, 4, 0, 139, 191, 65, 129, 149, 1, 30, 27, 0, 0, 0, 0, 73, 69, 78, 68,
        174, 66, 96, 130,
    ]
}

#[cfg(feature = "in-process-pdf")]
fn fixture_image_pdf() -> Vec<u8> {
    vec![
        37, 80, 68, 70, 45, 49, 46, 52, 10, 49, 32, 48, 32, 111, 98, 106, 10, 60, 60, 32, 47, 84,
        121, 112, 101, 32, 47, 67, 97, 116, 97, 108, 111, 103, 32, 47, 80, 97, 103, 101, 115, 32,
        50, 32, 48, 32, 82, 32, 62, 62, 10, 101, 110, 100, 111, 98, 106, 10, 50, 32, 48, 32, 111,
        98, 106, 10, 60, 60, 32, 47, 84, 121, 112, 101, 32, 47, 80, 97, 103, 101, 115, 32, 47, 75,
        105, 100, 115, 32, 91, 51, 32, 48, 32, 82, 93, 32, 47, 67, 111, 117, 110, 116, 32, 49, 32,
        62, 62, 10, 101, 110, 100, 111, 98, 106, 10, 51, 32, 48, 32, 111, 98, 106, 10, 60, 60, 32,
        47, 84, 121, 112, 101, 32, 47, 80, 97, 103, 101, 32, 47, 80, 97, 114, 101, 110, 116, 32,
        50, 32, 48, 32, 82, 32, 47, 77, 101, 100, 105, 97, 66, 111, 120, 32, 91, 48, 32, 48, 32,
        49, 48, 48, 32, 49, 48, 48, 93, 32, 47, 82, 101, 115, 111, 117, 114, 99, 101, 115, 32, 60,
        60, 32, 47, 88, 79, 98, 106, 101, 99, 116, 32, 60, 60, 32, 47, 73, 109, 48, 32, 52, 32, 48,
        32, 82, 32, 62, 62, 32, 62, 62, 32, 47, 67, 111, 110, 116, 101, 110, 116, 115, 32, 53, 32,
        48, 32, 82, 32, 62, 62, 10, 101, 110, 100, 111, 98, 106, 10, 52, 32, 48, 32, 111, 98, 106,
        10, 60, 60, 32, 47, 84, 121, 112, 101, 32, 47, 88, 79, 98, 106, 101, 99, 116, 32, 47, 83,
        117, 98, 116, 121, 112, 101, 32, 47, 73, 109, 97, 103, 101, 32, 47, 87, 105, 100, 116, 104,
        32, 52, 32, 47, 72, 101, 105, 103, 104, 116, 32, 52, 32, 47, 67, 111, 108, 111, 114, 83,
        112, 97, 99, 101, 32, 47, 68, 101, 118, 105, 99, 101, 82, 71, 66, 32, 47, 66, 105, 116,
        115, 80, 101, 114, 67, 111, 109, 112, 111, 110, 101, 110, 116, 32, 56, 32, 47, 70, 105,
        108, 116, 101, 114, 32, 47, 70, 108, 97, 116, 101, 68, 101, 99, 111, 100, 101, 32, 47, 76,
        101, 110, 103, 116, 104, 32, 49, 52, 32, 62, 62, 10, 115, 116, 114, 101, 97, 109, 10, 120,
        156, 115, 12, 42, 118, 36, 5, 1, 0, 142, 79, 16, 97, 10, 101, 110, 100, 115, 116, 114, 101,
        97, 109, 10, 101, 110, 100, 111, 98, 106, 10, 53, 32, 48, 32, 111, 98, 106, 10, 60, 60, 32,
        47, 76, 101, 110, 103, 116, 104, 32, 51, 49, 32, 62, 62, 10, 115, 116, 114, 101, 97, 109,
        10, 113, 10, 49, 48, 48, 32, 48, 32, 48, 32, 49, 48, 48, 32, 48, 32, 48, 32, 99, 109, 10,
        47, 73, 109, 48, 32, 68, 111, 10, 81, 10, 101, 110, 100, 115, 116, 114, 101, 97, 109, 10,
        101, 110, 100, 111, 98, 106, 10, 120, 114, 101, 102, 10, 48, 32, 54, 10, 48, 48, 48, 48,
        48, 48, 48, 48, 48, 48, 32, 54, 53, 53, 51, 53, 32, 102, 32, 10, 48, 48, 48, 48, 48, 48,
        48, 48, 48, 57, 32, 48, 48, 48, 48, 48, 32, 110, 32, 10, 48, 48, 48, 48, 48, 48, 48, 48,
        53, 56, 32, 48, 48, 48, 48, 48, 32, 110, 32, 10, 48, 48, 48, 48, 48, 48, 48, 49, 49, 53,
        32, 48, 48, 48, 48, 48, 32, 110, 32, 10, 48, 48, 48, 48, 48, 48, 48, 50, 52, 53, 32, 48,
        48, 48, 48, 48, 32, 110, 32, 10, 48, 48, 48, 48, 48, 48, 48, 52, 50, 51, 32, 48, 48, 48,
        48, 48, 32, 110, 32, 10, 116, 114, 97, 105, 108, 101, 114, 10, 60, 60, 32, 47, 83, 105,
        122, 101, 32, 54, 32, 47, 82, 111, 111, 116, 32, 49, 32, 48, 32, 82, 32, 62, 62, 10, 115,
        116, 97, 114, 116, 120, 114, 101, 102, 10, 53, 48, 51, 10, 37, 37, 69, 79, 70, 10,
    ]
}

#[test]
fn ordinary_image_uses_same_host_generate_and_has_readable_search_output() {
    if in_isolated_child(
        "model_effects::ordinary_image_uses_same_host_generate_and_has_readable_search_output",
    ) {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (root, server, host, generator) = model_fixture();
    complete_owner_file(
        &server,
        &host,
        root.path(),
        "image.png",
        "image",
        "image/png",
        &fixture_png(),
        "cobalt ordinary model description",
    );
    assert_eq!(
        *generator.calls.lock().unwrap(),
        vec!["import.image.vision"]
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[cfg(feature = "in-process-pdf")]
#[test]
#[ignore = "requires the retained pinned host PDFium library; explicit qualification only"]
fn ordinary_pdf_uses_actual_pdfium_and_same_host_generate_before_read_search() {
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let library = std::path::PathBuf::from(
        std::env::var("SOLSTONE_NATIVE_FOLDER_TEST_PDFIUM").expect("retained PDFium required"),
    );
    let (root, server, host, generator) =
        model_fixture_with_factory(packaged_import_factory(library));
    complete_owner_file(
        &server,
        &host,
        root.path(),
        "scan.pdf",
        "document",
        "application/pdf",
        &fixture_image_pdf(),
        "cobalt ordinary model description",
    );
    assert_eq!(
        *generator.calls.lock().unwrap(),
        vec!["import.document.vision"]
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[cfg(feature = "audio-native")]
#[test]
#[ignore = "requires the retained speech fixture and pinned native model/assets; explicit qualification only"]
fn ordinary_audio_upload_uses_shared_sense_and_pinned_speech_before_read_with_raw_search_excluded()
{
    use sha2::{Digest, Sha256};
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (root, server, host, generator) = model_fixture();
    fs::write(root.path().join("config/journal.json"), serde_json::to_vec(&serde_json::json!({
        "setup":{"completed_at":1},"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true},
        "transcribe":{"backend":"parakeet","confidential_audio":false,"preserve_all":true}
    })).unwrap()).unwrap();
    let fixture = std::path::PathBuf::from(
        std::env::var("FC2_MEDIA_SPEECH_FIXTURE").expect("retained synthetic speech required"),
    );
    let bytes = fs::read(&fixture).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "69692b6c8c4374f3c1f806b80840ee7f08555d35f42d8c0d64118176c95198f4"
    );
    host.enable_audio(
        std::path::PathBuf::from(
            std::env::var("FC2_MEDIA_MODELS").expect("retained media assets required"),
        ),
        std::path::PathBuf::from(
            std::env::var("FC2_MEDIA_PARAKEET_MODEL").expect("retained pinned Parakeet required"),
        ),
        root.path().join("native-scratch"),
    )
    .unwrap();
    let native = observe_native_result(&server, &host);
    let saved = upload_file(&server, "speech.wav", "", "audio/wav", &bytes);
    let id = saved["timestamp"].as_str().unwrap();
    let original = std::path::PathBuf::from(saved["path"].as_str().unwrap());
    let mut request = serde_json::json!({"path":saved["path"],"timestamp":saved["source_timestamp"],"import_id":id});
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    assert!(preview["preview_token"].is_string(), "{preview}");
    request["preview_token"] = preview["preview_token"].clone();
    owner_json(&server, "POST", "/app/import/api/start", request.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let staged = loop {
        if let Some(record) =
            solstone_core_import_host::audio::read_audio_import_record(root.path(), id).unwrap()
            && !record.created_segments.is_empty()
        {
            break record;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "generic audio did not publish its durable input"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    };
    assert_eq!(staged.created_segments.len(), 1);
    assert_eq!(staged.created_segments[0].start_offset_seconds, 0.0);
    let input = staged.created_segments[0].file_path.clone();
    assert!(
        !input.with_extension("jsonl").exists(),
        "saved raw was incorrectly presented as processed"
    );
    host.run_sense_pass().unwrap(); // The production shared Sense/AudioWorker, not a second recognizer.
    eprintln!(
        "audio worker after Sense: {}; processing policy: {:?}",
        host.audio_status(),
        solstone_core_system::processing::policy_at(root.path(), chrono::Utc::now())
    );
    let result = native
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|error| {
            panic!("native completion: {error}; audio: {}", host.audio_status())
        })
        .unwrap();
    assert_eq!(
        result.0,
        0,
        "{}{}; audio status: {}",
        result.1,
        result.2,
        host.audio_status()
    );
    assert_only_saved_record(root.path(), id);
    assert_eq!(
        owner_json(
            &server,
            "GET",
            &format!("/app/import/api/{id}"),
            serde_json::Value::Null
        )["status"],
        "success"
    );
    let transcript = fs::read_to_string(input.with_extension("jsonl")).unwrap();
    assert!(
        transcript.contains("The quick brown fox jumps over the lazy dog."),
        "{transcript}"
    );
    assert!(input.with_extension("npz").is_file());
    assert!(input.is_file());
    assert_eq!(fs::read(&original).unwrap(), bytes);
    let read = owner_json(
        &server,
        "GET",
        &format!(
            "/app/transcripts/api/read/{}?transcripts=1",
            staged.created_segments[0].day
        ),
        serde_json::Value::Null,
    );
    assert!(
        read["markdown"]
            .as_str()
            .unwrap()
            .contains("quick brown fox"),
        "{read}"
    );
    // Raw speech is deliberately unindexed on desktop and mobile alike. Saved
    // analysis has its separate publication authority; do not bypass it here.
    use solstone_core_format::content::{
        ContentResolution, RawPerceptFamily, resolve_content_shape,
    };
    let transcript_path = input.with_extension("jsonl");
    assert_eq!(
        resolve_content_shape(
            &transcript_path,
            transcript_path
                .strip_prefix(root.path().join("chronicle"))
                .unwrap()
                .to_str()
                .unwrap()
        ),
        ContentResolution::Unindexed(RawPerceptFamily::Audio)
    );
    let indexed = server.index().unwrap(); // Explicit index; no automatic-indexing claim.
    let search = server
        .request_owner("GET", "/app/search/api/search?q=fox", "")
        .unwrap();
    assert_eq!(search.status, 503, "{}", search.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&search.body).unwrap()["reason_code"],
        "empty_index"
    );
    eprintln!("ordinary speech transcript: {transcript}; explicit index: {indexed}");
    assert_eq!(
        owner_json(&server, "POST", "/app/import/api/start", request)["replay"],
        true
    );
    assert!(
        generator.calls.lock().unwrap().is_empty(),
        "local speech cannot introduce a second Generate path"
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}
