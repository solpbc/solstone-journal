// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};

type NativeObservation = Result<(i32, String, String), String>;

struct ObservedRoute {
    inner: ForegroundImportRoute,
    result: mpsc::Sender<NativeObservation>,
}
struct ObservedAdmission {
    inner: Arc<dyn ImportAdmission>,
    result: mpsc::Sender<NativeObservation>,
}
struct ObservedImport {
    inner: Arc<dyn CapturedImport>,
    result: mpsc::Sender<NativeObservation>,
}
impl RetainedImportRoute for ObservedRoute {
    fn prepare(
        &self,
        stop: WorkStop,
        priority: WorkPriority,
    ) -> Result<Arc<dyn ImportAdmission>, String> {
        Ok(Arc::new(ObservedAdmission {
            inner: self.inner.prepare(stop, priority)?,
            result: self.result.clone(),
        }))
    }
}
impl ImportAdmission for ObservedAdmission {
    fn checkpoint(&self) -> Result<(), String> {
        self.inner.checkpoint()
    }
    fn bind(self: Arc<Self>, args: Vec<String>) -> Result<Arc<dyn CapturedImport>, String> {
        Ok(Arc::new(ObservedImport {
            inner: Arc::clone(&self.inner).bind(args)?,
            result: self.result.clone(),
        }))
    }
}
impl CapturedImport for ObservedImport {
    fn preview(&self) -> Result<CliRun, String> {
        self.inner.preview()
    }
    fn run(&self) -> Result<CliRun, String> {
        let result = self.inner.run();
        let observation = result
            .as_ref()
            .map(|run| (run.exit_code, run.stdout.clone(), run.stderr.clone()))
            .map_err(Clone::clone);
        let _ = self.result.send(observation);
        result
    }
}
fn observe_native_result(
    server: &MobileServing,
    host: &Arc<ForegroundProcessingHost>,
) -> mpsc::Receiver<NativeObservation> {
    let (result, receive) = mpsc::channel();
    server
        .import_route()
        .install(Arc::new(ObservedRoute {
            inner: ForegroundImportRoute::new(Arc::clone(host)),
            result,
        }))
        .unwrap();
    receive
}

fn owner_json(server: &MobileServing, method: &str, uri: &str, body: Value) -> Value {
    let response = server
        .request_owner(method, uri, &body.to_string())
        .unwrap();
    assert_eq!(response.status, 200, "{uri}: {}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

fn recorded_imports(root: &std::path::Path) -> Vec<(String, Option<String>)> {
    let mut records = fs::read_dir(root.join("imports"))
        .unwrap()
        .map(|entry| {
            let id = entry.unwrap().file_name().to_string_lossy().into_owned();
            let attempt = solstone_core_import::read_import_metadata(root, &id)
                .ok()
                .and_then(|metadata| solstone_core_import::get_attempt_facts(&metadata))
                .map(|facts| format!("{:?}:{}", facts.state, facts.generation));
            (id, attempt)
        })
        .collect::<Vec<_>>();
    records.sort();
    records
}

fn assert_native_success(receive: &mpsc::Receiver<NativeObservation>) {
    let (code, stdout, stderr) = receive
        .recv_timeout(Duration::from_secs(10))
        .expect("retained native worker did not return")
        .expect("retained native worker refused the import");
    assert_eq!(code, 0, "stdout: {stdout}; stderr: {stderr}");
}

fn assert_only_saved_record(root: &std::path::Path, id: &str) {
    let records = recorded_imports(root)
        .into_iter()
        .filter(|(id, _)| solstone_core_import::validate_timestamp(id).is_ok())
        .collect::<Vec<_>>();
    assert_eq!(records, vec![(id.to_owned(), Some("Completed:1".into()))]);
}

fn upload_file(
    server: &MobileServing,
    filename: &str,
    source_hint: &str,
    mime: &str,
    bytes: &[u8],
) -> Value {
    unsafe extern "C" {
        fn pipe(fds: *mut i32) -> i32;
        fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
    }
    let mut fds = [-1; 2];
    assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
    let body_read = unsafe { fs::File::from_raw_fd(fds[0]) };
    let mut body_write = unsafe { fs::File::from_raw_fd(fds[1]) };
    let flags = unsafe { fcntl(fds[0], 3, 0) };
    assert!(flags >= 0);
    assert_eq!(unsafe { fcntl(fds[0], 4, flags | 0o4000) }, 0);
    let (mut response_read, response_write) = std::os::unix::net::UnixStream::pair().unwrap();
    response_read
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    response_write.set_nonblocking(true).unwrap();
    let descriptor: OwnedFd = response_write.into();
    let mut multipart = format!("--effect\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nowner-file-effect\r\n--effect\r\nContent-Disposition: form-data; name=\"source_hint\"\r\n\r\n{source_hint}\r\n--effect\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n").into_bytes();
    multipart.extend_from_slice(bytes);
    multipart.extend_from_slice(b"\r\n--effect--\r\n");
    let receiver = server
        .open_owner_upload(
            crate::next_owner_request_id(),
            "POST",
            "/app/import/api/save",
            &[(
                "content-type".into(),
                "multipart/form-data; boundary=effect".into(),
            )],
            body_read,
            fs::File::from(descriptor),
        )
        .unwrap();
    let writer = std::thread::spawn(move || body_write.write_all(&multipart).unwrap());
    let head = receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    let mut response = Vec::new();
    response_read.read_to_end(&mut response).unwrap();
    writer.join().unwrap();
    assert_eq!(head.status, 200, "{}", String::from_utf8_lossy(&response));
    serde_json::from_slice(&response).unwrap()
}

#[test]
fn ordinary_owner_upload_preview_start_replays_and_reads_calendar_effects() {
    if in_isolated_child(
        "owner_effects::ordinary_owner_upload_preview_start_replays_and_reads_calendar_effects",
    ) {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let factory_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&factory_calls);
    let (root, server, host) = fixture(Arc::new(move |context| {
        observed.fetch_add(1, Ordering::AcqRel);
        Ok(PreparedImport::new(
            context.arguments().to_vec(),
            context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker)),
        ))
    }));
    let native_result = observe_native_result(&server, &host);
    let calendar = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:owner-route-calendar\r\nDTSTART:20260809T090000Z\r\nDTEND:20260809T100000Z\r\nSUMMARY:Heliotrope planning\r\nDESCRIPTION:Heliotrope owner route effect\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let saved = upload_file(&server, "calendar.ics", "ics", "text/calendar", calendar);
    let id = saved["timestamp"].as_str().unwrap();
    let input = std::path::Path::new(saved["path"].as_str().unwrap());
    assert_eq!(fs::read(input).unwrap(), calendar);
    let initial = solstone_core_import::read_import_metadata(root.path(), id).unwrap();
    assert!(solstone_core_import::get_attempt_facts(&initial).is_none());
    assert_eq!(factory_calls.load(Ordering::Acquire), 0);
    let request =
        json!({"path": saved["path"], "timestamp": saved["source_timestamp"], "import_id": id});
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    assert!(preview["preview_token"].is_string(), "{preview}");
    let after_preview = solstone_core_import::read_import_metadata(root.path(), id).unwrap();
    assert!(solstone_core_import::get_attempt_facts(&after_preview).is_none());
    let mut start = request;
    start["preview_token"] = preview["preview_token"].clone();
    let accepted = owner_json(&server, "POST", "/app/import/api/start", start.clone());
    assert!(accepted.get("attempt_id").is_none(), "{accepted}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let detail = loop {
        let detail = owner_json(
            &server,
            "GET",
            &format!("/app/import/api/{id}"),
            Value::Null,
        );
        if detail["status"] == "success" {
            break detail;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "import never completed: {detail}; native result: {:?}",
            native_result.try_recv()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(detail["source_type"], "ics", "{detail}");
    assert_eq!(detail["entries_written"], 1, "{detail}");
    assert_eq!(detail["target_day"], "20260809", "{detail}");
    let metadata = solstone_core_import::read_import_metadata(root.path(), id).unwrap();
    let facts = solstone_core_import::get_attempt_facts(&metadata).unwrap();
    assert_eq!(facts.state, solstone_core_import::AttemptState::Completed);
    assert_eq!(facts.generation, 1);
    assert_native_success(&native_result);
    assert_only_saved_record(root.path(), id);
    let completed_factory_calls = factory_calls.load(Ordering::Acquire);
    let replay = owner_json(&server, "POST", "/app/import/api/start", start);
    assert_eq!(replay["replay"], true, "{replay}");
    assert_eq!(
        factory_calls.load(Ordering::Acquire),
        completed_factory_calls
    );
    assert_eq!(
        solstone_core_import::get_attempt_facts(
            &solstone_core_import::read_import_metadata(root.path(), id).unwrap()
        )
        .unwrap()
        .generation,
        1
    );
    assert_eq!(fs::read(input).unwrap(), calendar);
    let read = owner_json(
        &server,
        "GET",
        "/app/transcripts/api/read/20260809?transcripts=1",
        Value::Null,
    );
    assert!(read.to_string().contains("Heliotrope"), "{read}");
    server.index().unwrap();
    let search = owner_json(
        &server,
        "GET",
        "/app/search/api/search?q=Heliotrope",
        Value::Null,
    );
    assert!(
        search["total"].as_u64().is_some_and(|count| count > 0),
        "{search}"
    );
    let hits = search["days"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|day| day["results"].as_array().unwrap());
    assert!(
        hits.into_iter().any(|hit| {
            hit["day"] == "20260809"
                && hit["path"]
                    .as_str()
                    .is_some_and(|path| path.starts_with("20260809/import.ics/"))
                && hit["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Heliotrope"))
        }),
        "{search}"
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

fn file_family(source: &str, filename: &str, bytes: &[u8], stream: &str, sentinel: &str) {
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (root, server, host) = fixture(Arc::new(|context| {
        Ok(PreparedImport::new(
            context.arguments().to_vec(),
            context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker)),
        ))
    }));
    let native_result = observe_native_result(&server, &host);
    let saved = upload_file(&server, filename, source, "application/zip", bytes);
    if source == "chatgpt" {
        // Start in a later wall-clock second than the upload's saved identity.
        std::thread::sleep(Duration::from_millis(1100));
    }
    let id = saved["timestamp"].as_str().unwrap();
    let original = std::path::Path::new(saved["path"].as_str().unwrap());
    assert_eq!(fs::read(original).unwrap(), bytes);
    let request =
        json!({"path":saved["path"], "timestamp":saved["source_timestamp"], "import_id":id});
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    assert!(preview["preview_token"].is_string(), "{source}: {preview}");
    let preview_metadata = solstone_core_import::read_import_metadata(root.path(), id).unwrap();
    assert!(solstone_core_import::get_attempt_facts(&preview_metadata).is_none());
    let staged_directory = root.path().join("imports").join(id);
    let preview_copy =
        staged_directory.join(preview_metadata["preview"]["relative"].as_str().unwrap());
    assert_eq!(fs::read(&preview_copy).unwrap(), bytes);
    let preview_tree = preview_copy.parent().unwrap();
    assert!(preview_tree.starts_with(&staged_directory) && preview_tree != staged_directory);
    let mut request = request;
    request["preview_token"] = preview["preview_token"].clone();
    owner_json(&server, "POST", "/app/import/api/start", request.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let detail = loop {
        let detail = owner_json(
            &server,
            "GET",
            &format!("/app/import/api/{id}"),
            Value::Null,
        );
        if detail["status"] == "success" {
            break detail;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{source} did not finish: {detail}; native result: {:?}; records: {:?}",
            native_result.try_recv(),
            recorded_imports(root.path())
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(detail["source_type"], source, "{detail}");
    let facts = solstone_core_import::get_attempt_facts(
        &solstone_core_import::read_import_metadata(root.path(), id).unwrap(),
    )
    .unwrap();
    assert_eq!(facts.state, solstone_core_import::AttemptState::Completed);
    assert_eq!(facts.generation, 1);
    assert_native_success(&native_result);
    assert_only_saved_record(root.path(), id);
    let replay = owner_json(&server, "POST", "/app/import/api/start", request);
    assert_eq!(replay["replay"], true, "{source}: {replay}");
    assert_eq!(
        solstone_core_import::get_attempt_facts(
            &solstone_core_import::read_import_metadata(root.path(), id).unwrap()
        )
        .unwrap()
        .generation,
        1
    );
    if source == "strava" {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let absent = |path| {
            matches!(fs::symlink_metadata(path), Err(error)
            if error.kind() == std::io::ErrorKind::NotFound)
        };
        loop {
            if absent(original) && absent(preview_tree) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "completed Strava upload or preview tree was retained"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    } else {
        assert_eq!(fs::read(original).unwrap(), bytes);
    }
    let read = owner_json(
        &server,
        "GET",
        "/app/transcripts/api/read/20261014?transcripts=1",
        Value::Null,
    );
    assert!(
        read["markdown"]
            .as_str()
            .is_some_and(|text| text.contains(sentinel)),
        "{source}: {read}"
    );
    server.index().unwrap();
    let search = owner_json(
        &server,
        "GET",
        "/app/search/api/search?q=cobalt",
        Value::Null,
    );
    let hits = search["days"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|day| day["results"].as_array().unwrap());
    assert!(
        hits.into_iter().any(|hit| hit["day"] == "20261014"
            && hit["path"]
                .as_str()
                .is_some_and(|path| path.starts_with(&format!("20261014/{stream}/")))
            && (source == "journal_archive" || hit["stream"] == stream)
            && hit["text"]
                .as_str()
                .is_some_and(|text| text.contains("cobalt"))),
        "{source}: {search}"
    );
    if source == "journal_archive" {
        assert_eq!(
            fs::read(root.path().join("imports/archive-fixture/source.txt")).unwrap(),
            b"cobalt harbor portable journal\n"
        );
    }
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[test]
fn ordinary_owner_claude_export_reads_and_searches_its_saved_effect() {
    if in_isolated_child(
        "owner_effects::ordinary_owner_claude_export_reads_and_searches_its_saved_effect",
    ) {
        return;
    }
    file_family(
        "claude",
        "claude.zip",
        include_bytes!("../../../core/fixtures/mobile_owner_import/claude.zip"),
        "import.claude",
        "cobalt harbor dialogue",
    );
}

#[test]
fn ordinary_owner_chatgpt_export_reads_and_searches_its_saved_effect() {
    if in_isolated_child(
        "owner_effects::ordinary_owner_chatgpt_export_reads_and_searches_its_saved_effect",
    ) {
        return;
    }
    file_family(
        "chatgpt",
        "chatgpt.zip",
        include_bytes!("../../../core/fixtures/mobile_owner_import/chatgpt.zip"),
        "import.chatgpt",
        "cobalt harbor conversation",
    );
}

#[test]
fn ordinary_owner_gemini_export_reads_and_searches_its_saved_effect() {
    if in_isolated_child(
        "owner_effects::ordinary_owner_gemini_export_reads_and_searches_its_saved_effect",
    ) {
        return;
    }
    file_family(
        "gemini",
        "gemini.zip",
        include_bytes!("../../../core/fixtures/mobile_owner_import/gemini.zip"),
        "import.gemini",
        "cobalt harbor activity",
    );
}

#[test]
fn ordinary_owner_strava_export_reads_and_searches_its_saved_effect() {
    if in_isolated_child(
        "owner_effects::ordinary_owner_strava_export_reads_and_searches_its_saved_effect",
    ) {
        return;
    }
    file_family(
        "strava",
        "strava.zip",
        include_bytes!("../../../core/fixtures/mobile_owner_import/strava.zip"),
        "import.strava",
        "cobalt harbor workout",
    );
}

#[test]
fn ordinary_owner_portable_journal_reads_and_searches_its_saved_effect() {
    if in_isolated_child(
        "owner_effects::ordinary_owner_portable_journal_reads_and_searches_its_saved_effect",
    ) {
        return;
    }
    file_family(
        "journal_archive",
        "journal.zip",
        include_bytes!("../../../core/fixtures/mobile_owner_import/journal.zip"),
        "import.text",
        "cobalt harbor portable journal",
    );
}
