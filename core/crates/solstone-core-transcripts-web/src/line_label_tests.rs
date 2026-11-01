// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_speaker_resolve::owner_line_labels::{
    LineSpeaker, OwnerLine, current_owner_line_entries, set_owner_line_labels,
};
use tower::ServiceExt;

use crate::Clock;

const DAY: &str = "20260510";
const KEY: &str = "120000_300";

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn segment_dir(root: &Path, stream: &str) -> PathBuf {
    root.join("chronicle").join(DAY).join(stream).join(KEY)
}

fn journal() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (id, principal) in [("owner", true), ("ada", false), ("bo", false)] {
        write(
            &root.path().join("entities").join(id).join("entity.json"),
            &json!({"id":id,"name":id,"type":"Person","is_principal":principal}).to_string(),
        );
    }
    drop(solstone_core_entity::hold_entity_trust_lock(root.path()).unwrap());
    for stream in ["mic", "phone"] {
        let rows = [
            json!({"raw":"audio.flac"}),
            json!({"start":"00:00:01","text":"hello","sentence_id":1}),
        ]
        .map(|row| row.to_string() + "\n")
        .concat();
        write(&segment_dir(root.path(), stream).join("audio.jsonl"), &rows);
    }
    for (facet, id, stream) in [
        ("work", "on_mic", "mic"),
        ("home", "also_on_mic", "mic"),
        ("home", "on_phone", "phone"),
    ] {
        if !root.path().join("facets").join(facet).exists() {
            solstone_core_facets::create_facet(root.path(), facet, facet, "", "", "", None)
                .unwrap();
        }
        let row = json!({"id":id,"activity":"meeting","stream":stream,"segments":[KEY]});
        let _ = solstone_core_facets::append_activity_record(
            root.path(),
            facet,
            DAY,
            row.as_object().unwrap().clone(),
        )
        .unwrap();
    }
    root
}

async fn post(root: &Path, path: &str, owner: bool, body: Value) -> (StatusCode, Value) {
    let app = crate::router(root.to_path_buf(), Clock::system(), || {
        axum::response::Response::new(Body::empty())
    });
    let mut request = Request::builder()
        .method("POST")
        .uri(format!(
            "/app/transcripts/api/line-label/{DAY}/mic/{KEY}{path}"
        ))
        .header(header::CONTENT_TYPE, "application/json");
    if owner {
        request = request.header("x-solstone-actor", "owner");
    }
    let response = app
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn answer(root: &Path) -> Option<Option<String>> {
    current_owner_line_entries(&segment_dir(root, "mic"))
        .unwrap()
        .into_iter()
        .find(|entry| entry.key() == ("audio", 1))
        .map(|entry| entry.speaker)
}

fn asks(root: &Path) -> Vec<solstone_core_system::owner_ask::OwnerAsk> {
    solstone_core_system::owner_ask::read(root)
        .unwrap()
        .into_values()
        .collect()
}

#[tokio::test]
async fn the_owner_answers_a_line_and_its_streams_activities_are_asked_again() {
    let root = journal();
    let (status, body) = post(
        root.path(),
        "",
        true,
        json!({"source":"audio","sentence_id":1,"speaker":{"kind":"person","id":"ada"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"ok":true,"asked":true}));
    assert_eq!(answer(root.path()), Some(Some("ada".into())));
    let asks = asks(root.path());
    assert_eq!(asks.len(), 1);
    assert!(asks[0].thaw);
    let named = asks[0]
        .to_seed
        .iter()
        .map(|asked| asked.activity.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(named, BTreeSet::from(["also_on_mic", "on_mic"]));
}

#[tokio::test]
async fn back_to_the_journals_guess_removes_whatever_answer_the_line_has() {
    let root = journal();
    // An answer given from an activity.
    set_owner_line_labels(
        root.path(),
        &[OwnerLine {
            segment_dir: segment_dir(root.path(), "mic"),
            source: "audio".into(),
            sentence_id: 1,
        }],
        &LineSpeaker::Person("bo".into()),
    )
    .unwrap();
    let line = json!({"source":"audio","sentence_id":1});
    let (status, body) = post(root.path(), "/remove", true, line.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["asked"], true);
    assert_eq!(answer(root.path()), None);
    // Nothing left to remove: nothing is written or asked.
    let (status, body) = post(root.path(), "/remove", true, line).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok":true,"asked":false}));
    assert_eq!(asks(root.path()).len(), 1);
}

#[tokio::test]
async fn refusals_change_nothing() {
    let root = journal();
    let owner_file =
        || fs::read(segment_dir(root.path(), "mic").join("talents/owner_line_labels.json")).ok();
    let ada = json!({"kind":"person","id":"ada"});
    for (path, owner, body, status, reason) in [
        (
            "",
            false,
            json!({"source":"audio","sentence_id":1,"speaker":ada}),
            StatusCode::FORBIDDEN,
            "owner_line_owner_only",
        ),
        (
            "/remove",
            false,
            json!({"source":"audio","sentence_id":1}),
            StatusCode::FORBIDDEN,
            "owner_line_owner_only",
        ),
        (
            "",
            true,
            json!({"sentence_id":1,"speaker":ada}),
            StatusCode::BAD_REQUEST,
            "owner_line_invalid",
        ),
        (
            "",
            true,
            json!({"source":"audio","sentence_id":1,"speaker":{"kind":"person"}}),
            StatusCode::BAD_REQUEST,
            "owner_line_speaker_invalid",
        ),
        (
            "",
            true,
            json!({"source":"audio","sentence_id":1,"speaker":{"kind":"person","id":"zed"}}),
            StatusCode::CONFLICT,
            "owner_line_person_unavailable",
        ),
        (
            "",
            true,
            json!({"source":"audio","sentence_id":7,"speaker":ada}),
            StatusCode::CONFLICT,
            "owner_line_not_found",
        ),
    ] {
        let (got, reply) = post(root.path(), path, owner, body.clone()).await;
        assert_eq!(got, status, "{body}");
        assert_eq!(reply["reason_code"], reason, "{body}");
        assert_eq!(owner_file(), None, "{body}");
        assert!(asks(root.path()).is_empty(), "{body}");
    }
}
