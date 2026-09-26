// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;

use axum::http::{Request, StatusCode};
use serde_json::json;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use tempfile::TempDir;
use tower::ServiceExt;

const CID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SEGMENT_DIR: &str = "chronicle/20260805/location/070000_17";
const STREAM_RECORD: &str = "streams/location.json";

fn setup_bed() -> (TempDir, BTreeSet<String>, Vec<(String, Vec<u8>)>, Vec<u8>) {
    let bed = TempDir::new_in("/var/tmp").expect("journal");
    let journal = bed.path();

    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        r#"{"setup":{"completed_at":1},"jid":"journal-jid-fixture"}"#,
    )
    .unwrap();

    let seg_path = journal.join(SEGMENT_DIR);
    fs::create_dir_all(&seg_path).unwrap();
    let loc_bytes = b"{\"fix\":\"loc\"}\n";
    let stream_json_bytes = b"{}";
    fs::write(seg_path.join("location.jsonl"), loc_bytes).unwrap();
    fs::write(seg_path.join("stream.json"), stream_json_bytes).unwrap();

    let streams_path = journal.join("streams");
    fs::create_dir_all(&streams_path).unwrap();
    let stream_rec_bytes = json!({
        "name": "location",
        "kind": "unknown",
        "host": null,
        "platform": null,
        "created_at": 1,
        "last_day": null,
        "last_segment": null,
        "seq": 0,
        "cid": CID,
        "source": "location",
    })
    .to_string()
    .into_bytes();
    fs::write(journal.join(STREAM_RECORD), &stream_rec_bytes).unwrap();

    let mut listing = BTreeSet::new();
    for entry in fs::read_dir(&seg_path).unwrap() {
        listing.insert(entry.unwrap().file_name().to_string_lossy().into_owned());
    }

    let file_bytes = vec![
        ("location.jsonl".to_owned(), loc_bytes.to_vec()),
        ("stream.json".to_owned(), stream_json_bytes.to_vec()),
    ];

    (bed, listing, file_bytes, stream_rec_bytes)
}

#[tokio::test]
async fn devices_source_delete_is_retired_and_preserves_disk_state() {
    let (bed, initial_listing, initial_files, initial_stream_record) = setup_bed();
    let app = crate::router(bed.path().to_path_buf());

    let mut request = Request::builder()
        .method("DELETE")
        .uri("/app/devices/source/location")
        .body(axum::body::Body::empty())
        .unwrap();

    request.extensions_mut().insert(AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(CID).unwrap(),
    });

    let response = app.oneshot(request).await.expect("response");
    let status = response.status();

    let seg_path = bed.path().join(SEGMENT_DIR);
    let mut current_listing = BTreeSet::new();
    if seg_path.is_dir() {
        for entry in fs::read_dir(&seg_path).unwrap() {
            current_listing.insert(entry.unwrap().file_name().to_string_lossy().into_owned());
        }
    }

    assert_eq!(
        current_listing, initial_listing,
        "segment erased or tombstone.json written"
    );

    for (filename, expected_bytes) in initial_files {
        let actual_bytes = fs::read(seg_path.join(&filename)).unwrap_or_default();
        assert_eq!(
            actual_bytes, expected_bytes,
            "file bytes modified: {filename}"
        );
    }

    let actual_stream_record = fs::read(bed.path().join(STREAM_RECORD)).unwrap_or_default();
    assert_eq!(
        actual_stream_record, initial_stream_record,
        "stream-record bytes modified"
    );

    assert_eq!(status, StatusCode::NOT_FOUND);
}
