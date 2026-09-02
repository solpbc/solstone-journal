// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use solstone_core_clients_web::router as clients_router;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_ingest::api_router as ingest_router;
use solstone_core_journal_io::{LockOptions, hold_lock, lock_is_held};
use tempfile::TempDir;
use tokio::runtime::Runtime;
use tower::ServiceExt;

const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DAY: &str = "20260805";
const SEGMENT: &str = "070000_17";

fn linked_basis(cid: &str) -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(cid).unwrap(),
    }
}

fn ingest_request(cid: &str, segment: &str) -> Request<Body> {
    let boundary = "location-mutation-ordering";
    let envelope = json!({
        "day": DAY,
        "segment": segment,
        "source": "location",
        "files": [{"submitted": "location.jsonl"}],
    })
    .to_string();
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n{envelope}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"location.jsonl\"\r\n\r\n{{}}\n\r\n\
         --{boundary}--\r\n"
    );
    let mut request = Request::builder()
        .method("POST")
        .uri("/app/devices/ingest")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("X-Solstone-Protocol-Version", "3")
        .body(Body::from(body))
        .unwrap();
    request.extensions_mut().insert(linked_basis(cid));
    request
}

fn delete_request(cid: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("DELETE")
        .uri("/app/devices/source/location")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(linked_basis(cid));
    request
}

fn run_ingest(router: Router, cid: &str, segment: &str) -> (StatusCode, Value) {
    Runtime::new().unwrap().block_on(async move {
        let response = router.oneshot(ingest_request(cid, segment)).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    })
}

fn run_delete(router: Router, cid: &str) -> (StatusCode, Value) {
    Runtime::new().unwrap().block_on(async move {
        let response = router.oneshot(delete_request(cid)).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    })
}

fn source_lock_target(journal: &Path) -> PathBuf {
    journal.join("streams/.source-location.mutation")
}

fn write_pairing_identities(journal: &Path) {
    let path = journal.join("link/authorized_clients.json");
    fs::create_dir_all(path.parent().unwrap()).expect("link directory");
    fs::write(
        path,
        json!([
            {
                "fingerprint": CID_A,
                "device_label": "fixture A",
                "paired_at": "2026-01-01T00:00:00Z",
                "instance_id": "fixture-instance",
                "role": "",
                "kind": "cert",
            },
            {
                "fingerprint": CID_B,
                "device_label": "fixture B",
                "paired_at": "2026-01-01T00:00:00Z",
                "instance_id": "fixture-instance",
                "role": "",
                "kind": "cert",
            },
        ])
        .to_string(),
    )
    .expect("pairing identities");
}

fn wait_until_source_lock_is_held(journal: &Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if lock_is_held(source_lock_target(journal)).unwrap() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("location source lock was not acquired before the deadline");
}

fn one_location_segment(journal: &Path) -> PathBuf {
    let day = journal.join("chronicle").join(DAY);
    for stream in fs::read_dir(day).unwrap() {
        let stream = stream.unwrap().path();
        for segment in fs::read_dir(stream).unwrap() {
            let segment = segment.unwrap().path();
            if segment.join("location.jsonl").is_file() || segment.join("tombstone.json").is_file()
            {
                return segment;
            }
        }
    }
    panic!("expected a live location segment");
}

fn one_live_location_segment(journal: &Path) -> PathBuf {
    let day = journal.join("chronicle").join(DAY);
    for stream in fs::read_dir(day).unwrap() {
        let stream = stream.unwrap().path();
        for segment in fs::read_dir(stream).unwrap() {
            let segment = segment.unwrap().path();
            if segment.join("location.jsonl").is_file() {
                return segment;
            }
        }
    }
    panic!("expected a live location segment");
}

#[test]
fn location_ingest_and_deletion_share_one_outer_mutation_order() {
    let temporary = TempDir::new_in("/var/tmp").unwrap();
    let journal = temporary.path().to_path_buf();
    write_pairing_identities(&journal);
    let callosum_runtime = Runtime::new().unwrap();
    let socket = journal.join("health/callosum.sock");
    let server = callosum_runtime
        .block_on(CallosumSocketServer::bind(&socket))
        .unwrap();
    let mut peer = CallosumSocketConnection::new(&socket, Map::new());
    callosum_runtime.block_on(async {
        peer.start();
        for _ in 0..50 {
            if server.client_count() >= 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("callosum peer did not connect");
    });

    // Ingest first: an inner registry lock stops the request after it acquires
    // the outer source lock and before its first stream reservation. Deletion
    // cannot scan until that request has completed its durable work.
    let registry_lock = hold_lock(journal.join("streams/.registry"), LockOptions::default())
        .expect("hold the ingest registry lock");
    let (ingest_done_sender, ingest_done) = mpsc::channel();
    let ingest_journal = journal.clone();
    let ingest_first = thread::spawn(move || {
        let status = run_ingest(ingest_router(&ingest_journal), CID_A, SEGMENT);
        ingest_done_sender.send(status).unwrap();
    });
    wait_until_source_lock_is_held(&journal);

    let (delete_done_sender, delete_done) = mpsc::channel();
    let delete_journal = journal.clone();
    let delete_after_ingest = thread::spawn(move || {
        let response = run_delete(clients_router(delete_journal), CID_B);
        delete_done_sender.send(response).unwrap();
    });
    assert!(matches!(
        delete_done.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));

    drop(registry_lock);
    let (status, body) = ingest_done.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    ingest_first.join().unwrap();
    let (status, receipt) = delete_done.recv_timeout(Duration::from_secs(2)).unwrap();
    delete_after_ingest.join().unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["removed"]["segments"], 1);
    assert!(
        one_location_segment(&journal)
            .join("tombstone.json")
            .is_file()
    );

    // Delete first: holding its selected segment lock keeps delete inside the
    // outer source lock. A later ingest cannot rebind until delete's decisive
    // rescan has unlinked the prior stream record and released that outer lock.
    let (status, body) = run_ingest(ingest_router(&journal), CID_A, "080000_17");
    assert_eq!(status, StatusCode::OK, "{body}");
    let existing = one_live_location_segment(&journal);
    let segment_lock = hold_lock(&existing, LockOptions::default()).unwrap();
    let (delete_done_sender, delete_done) = mpsc::channel();
    let delete_journal = journal.clone();
    let delete_first = thread::spawn(move || {
        let response = run_delete(clients_router(delete_journal), CID_B);
        delete_done_sender.send(response).unwrap();
    });
    wait_until_source_lock_is_held(&journal);

    let (ingest_done_sender, ingest_done) = mpsc::channel();
    let ingest_journal = journal.clone();
    let ingest_after_delete = thread::spawn(move || {
        let status = run_ingest(ingest_router(&ingest_journal), CID_A, "090000_17");
        ingest_done_sender.send(status).unwrap();
    });
    assert!(matches!(
        ingest_done.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));

    drop(segment_lock);
    let (status, receipt) = delete_done.recv_timeout(Duration::from_secs(2)).unwrap();
    delete_first.join().unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(receipt["removed"]["segments"].as_u64().unwrap() >= 1);
    let (status, body) = ingest_done.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    ingest_after_delete.join().unwrap();
    assert!(
        one_live_location_segment(&journal)
            .join("location.jsonl")
            .is_file()
    );
    callosum_runtime.block_on(peer.stop());
    callosum_runtime.block_on(server.stop());
}
