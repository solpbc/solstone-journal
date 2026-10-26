// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};

fn owner_json(server: &MobileServing, method: &str, uri: &str, body: Value) -> Value {
    let response = server
        .request_owner(method, uri, &body.to_string())
        .unwrap();
    assert_eq!(response.status, 200, "{uri}: {}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

fn upload_calendar(server: &MobileServing, bytes: &[u8]) -> Value {
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
    let mut multipart = b"--effect\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nowner-calendar-effect\r\n--effect\r\nContent-Disposition: form-data; name=\"source_hint\"\r\n\r\nics\r\n--effect\r\nContent-Disposition: form-data; name=\"file\"; filename=\"calendar.ics\"\r\nContent-Type: text/calendar\r\n\r\n".to_vec();
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
    let calendar = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:owner-route-calendar\r\nDTSTART:20260809T090000Z\r\nDTEND:20260809T100000Z\r\nSUMMARY:Heliotrope planning\r\nDESCRIPTION:Heliotrope owner route effect\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let saved = upload_calendar(&server, calendar);
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
            "import never completed: {detail}"
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
