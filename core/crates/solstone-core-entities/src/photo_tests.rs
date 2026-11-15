// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    extract::Extension,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_entity::faceprints as fp;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use tower::ServiceExt;

fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn switch(root: &Path, enabled: bool) {
    write(
        root,
        "config/journal.json",
        json!({"faces":{"enabled":enabled}}).to_string().as_bytes(),
    );
}
fn seed(root: &Path) -> Vec<fp::Face> {
    write(
        root,
        "entities/person/entity.json",
        br#"{"id":"person","name":"Test Person","type":"Person"}"#,
    );
    switch(root, true);
    let faces: Vec<_> = (0..5)
        .map(|i| {
            let mut embedding = vec![0.; 128];
            embedding[0] = 0.8;
            embedding[i + 1] = 0.6;
            let key = fp::FaceKey {
                day: "20261101".into(),
                stream: "screen".into(),
                stream_layout: "named".into(),
                segment_key: format!("{i:06}_300"),
                source_name: "screen.webm".into(),
                frame_id: i as u64,
                face_idx: 0,
            };
            let crop = format!(
                "{}-crops/{i}-0-test.jpg",
                fp::accumulation::source_prefix("screen.webm")
            );
            let path = fp::crop_path(root, &key, &crop).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"synthetic jpeg").unwrap();
            fp::Face {
                embedding,
                metadata: fp::Metadata {
                    schema_version: 1,
                    key,
                    activities: BTreeSet::new(),
                    source: fp::Source::Anchored,
                    label_text: None,
                    quality: i as f32 / 10.,
                    det_score: 1.,
                    face_w: 100.,
                    category: "meeting".into(),
                    crop: Some(crop),
                    added_at: "now".into(),
                    rejected: false,
                },
            }
        })
        .collect();
    fp::reconcile(
        root,
        &json!(["work", "20261101", "meeting"]).to_string(),
        &fp::Encoder {
            id: "synthetic".into(),
            sha256: "0".repeat(64),
            width: 128,
        },
        &BTreeMap::from([("person".into(), faces.clone())]),
    )
    .unwrap();
    faces
}
async fn call(
    root: &Path,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::api_router(root).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    (
        status,
        headers,
        to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
}
const BASE: &str = "/app/entities/api/journal/entity/person";

#[tokio::test]
async fn photos_routes_offer_only_public_metadata_and_images_and_owner_actions_undo() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let faces = seed(root);
    let (status, _, bytes) = call(root, "GET", &format!("{BASE}/faces"), Value::Null).await;
    assert_eq!(status, 200);
    let panel: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(panel["state"], "suggested");
    assert_eq!(panel["faces"].as_array().unwrap().len(), 5);
    assert!(!String::from_utf8(bytes).unwrap().contains("embedding"));
    let crop = panel["faces"][0]["crop_url"].as_str().unwrap();
    assert_eq!(
        call(root, "GET", crop, Value::Null).await.2,
        b"synthetic jpeg"
    );
    let (status, headers, _) = call(root, "GET", &format!("{BASE}/photo"), Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(headers["x-photo-state"], "suggested");
    assert_eq!(headers["cache-control"], "no-store");
    let key = json!({"key":faces[4].metadata.key});
    let (status, _, bytes) = call(root, "POST", &format!("{BASE}/faces/choose"), key.clone()).await;
    assert_eq!(status, 200);
    let action: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        call(root, "GET", &format!("{BASE}/photo"), Value::Null)
            .await
            .1["x-photo-state"],
        "confirmed"
    );
    assert_eq!(
        call(
            root,
            "POST",
            &format!("{BASE}/faces/undo"),
            json!({"token":action["token"]})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(root, "GET", &format!("{BASE}/photo"), Value::Null)
            .await
            .1["x-photo-state"],
        "suggested"
    );
    for (verb, body) in [("reject", key), ("dismiss", json!({}))] {
        let (status, _, bytes) = call(root, "POST", &format!("{BASE}/faces/{verb}"), body).await;
        assert_eq!(status, 200);
        let action: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(action["token"].is_string());
        assert_eq!(
            call(
                root,
                "POST",
                &format!("{BASE}/faces/undo"),
                json!({"token":action["token"]})
            )
            .await
            .0,
            200
        );
    }
    assert_eq!(
        call(
            root,
            "POST",
            "/app/entities/api/journal/faces/clear",
            json!({})
        )
        .await
        .0,
        409
    );
    switch(root, false);
    let (status, _, bytes) = call(root, "GET", &format!("{BASE}/faces"), Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["state"],
        "none"
    );
    assert_eq!(call(root, "GET", crop, Value::Null).await.0, 404);
    assert_eq!(
        call(root, "GET", &format!("{BASE}/photo"), Value::Null)
            .await
            .0,
        404
    );
    for (verb, body) in [
        ("choose", json!({"key":faces[0].metadata.key})),
        ("reject", json!({"key":faces[0].metadata.key})),
        ("dismiss", json!({})),
        ("undo", json!({"token":"old"})),
    ] {
        assert_eq!(
            call(root, "POST", &format!("{BASE}/faces/{verb}"), body)
                .await
                .0,
            409
        );
    }
    assert_eq!(
        call(
            root,
            "POST",
            "/app/entities/api/journal/faces/clear",
            json!({})
        )
        .await
        .0,
        200
    );
    assert!(!root.join("entities/person/faceprints.npz").exists());
}

#[tokio::test]
async fn photos_read_repairs_a_leftover_projection_without_an_owner_choice() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    seed(root);
    fp::dismiss(root, "person").unwrap();
    fs::write(
        root.join("entities").join("person").join(fp::PHOTO),
        b"stale projection",
    )
    .unwrap();
    assert!(crate::photos::has_photo_in_dir(root, "person", true));
    assert_eq!(
        call(root, "GET", &format!("{BASE}/photo"), Value::Null)
            .await
            .0,
        404
    );
    assert!(!crate::photos::has_photo_in_dir(root, "person", true));
}

#[tokio::test]
async fn photos_routes_distinguish_unknown_entity_key_stale_undo_and_busy_clear() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut faces = seed(root);
    for suffix in ["faces", "photo"] {
        assert_eq!(
            call(
                root,
                "GET",
                &format!("/app/entities/api/journal/entity/absent/{suffix}"),
                Value::Null
            )
            .await
            .0,
            404
        );
    }
    faces[0].metadata.key.frame_id = 999;
    for verb in ["choose", "reject"] {
        assert_eq!(
            call(
                root,
                "POST",
                &format!("{BASE}/faces/{verb}"),
                json!({"key":faces[0].metadata.key})
            )
            .await
            .0,
            404
        );
        assert_eq!(
            call(
                root,
                "POST",
                &format!("/app/entities/api/journal/entity/absent/faces/{verb}"),
                json!({"key":faces[0].metadata.key})
            )
            .await
            .0,
            404
        );
    }
    for verb in ["dismiss", "undo"] {
        assert_eq!(
            call(
                root,
                "POST",
                &format!("/app/entities/api/journal/entity/absent/faces/{verb}"),
                json!({"token":"old"})
            )
            .await
            .0,
            404
        );
    }
    assert_eq!(
        call(
            root,
            "POST",
            &format!("{BASE}/faces/undo"),
            json!({"token":"old"})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            root,
            "GET",
            &format!(
                "{BASE}/faces/crop/{}",
                crate::photos::encode_key(&faces[0].metadata.key)
            ),
            Value::Null
        )
        .await
        .0,
        404
    );
    switch(root, false);
    let key = &faces[1].metadata.key;
    let segment = root
        .join("chronicle")
        .join(&key.day)
        .join(&key.stream)
        .join(&key.segment_key);
    fs::write(segment.join("screen.jsonl"), b"{}\n").unwrap();
    let held =
        solstone_core_segment::output_attempt::OutputClaims::discover(&segment, Default::default())
            .unwrap();
    let (status, _, bytes) = call(
        root,
        "POST",
        "/app/entities/api/journal/faces/clear",
        json!({}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["reason_code"],
        "faces_busy"
    );
    drop(held);
    assert_eq!(
        call(
            root,
            "POST",
            "/app/entities/api/journal/faces/clear",
            json!({})
        )
        .await
        .0,
        200
    );
}

#[tokio::test]
async fn photos_routes_require_owner_access() {
    let temp = tempfile::tempdir().unwrap();
    seed(temp.path());
    let app = crate::api_router(temp.path()).layer(Extension(AccessBasis::PairingPeer {
        carrier: solstone_core_convey_http::identity::Carrier::Direct,
    }));
    let response = app
        .oneshot(
            Request::get(format!("{BASE}/faces"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.status(), 200);
}

#[tokio::test]
async fn photos_cross_rules_use_the_durable_id_for_every_read_surface() {
    for flags in [
        fp::panel::CrossEntity {
            contested: true,
            exclusivity_drop: false,
        },
        fp::panel::CrossEntity {
            contested: false,
            exclusivity_drop: true,
        },
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let faces = seed(root);
        // The owner's page uses a folder; derived rules use the durable id.
        fs::rename(
            root.join("entities/person"),
            root.join("entities/person-folder"),
        )
        .unwrap();
        for entity in ["person", "person-folder"] {
            assert_eq!(
                crate::photos::panel(root, entity).unwrap().state,
                crate::photos::State::Suggested
            );
        }
        let mut derived = fp::cross::Derived::default();
        derived.rules.entities.insert("person".into(), flags);
        write(
            root,
            &format!("entities/{}", fp::cross::FILE),
            &serde_json::to_vec(&derived).unwrap(),
        );
        for entity in ["person", "person-folder"] {
            let base = format!("/app/entities/api/journal/entity/{entity}");
            let (status, _, bytes) = call(root, "GET", &format!("{base}/faces"), Value::Null).await;
            assert_eq!(status, 200);
            let panel: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(panel["state"], "none");
            assert_eq!(panel["faces"], json!([]));
            assert!(panel["default"].is_null());
            assert_eq!(
                call(root, "GET", &format!("{base}/photo"), Value::Null)
                    .await
                    .0,
                404
            );
            assert_eq!(
                call(
                    root,
                    "GET",
                    &format!(
                        "{base}/faces/crop/{}",
                        crate::photos::encode_key(&faces[0].metadata.key)
                    ),
                    Value::Null
                )
                .await
                .0,
                404
            );
            // Connected tools use this same context and retain the facet boundary.
            let photo = crate::photos::photo(root, entity, Some("work")).unwrap();
            assert_eq!(photo.state, crate::photos::State::None);
            assert!(photo.jpeg.is_none());
        }
    }
}
