// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::{Value, json};
use solstone_core_journal_config::{StoredBodyFacet, parse_body_facet, read_journal_config};
use tempfile::TempDir;
use tower::ServiceExt;

use super::*;

fn journal(facets: &[&str]) -> TempDir {
    let root = TempDir::new().unwrap();
    for facet in facets {
        solstone_core_facets::create_facet(root.path(), facet, facet, "", "", "", None).unwrap();
    }
    root
}

async fn call(root: &Path, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = crate::routes(root.to_path_buf())
        .oneshot(
            request
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn offer_of(root: &Path) -> (String, Value) {
    let (status, body) = call(root, "GET", "/app/import/api/body-facet", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["offer"]["state"].as_str().unwrap().to_owned(),
        body["offer"]["facet"].clone(),
    )
}

/// Answer the question the card shows now.
async fn answer_with(root: &Path, answer: &str) -> (StatusCode, Value) {
    let (_, shown) = call(root, "GET", "/app/import/api/body-facet", None).await;
    answer_shown(root, answer, &shown["offer"]).await
}

/// Answer `shown`, the question as the card showed it.
async fn answer_shown(root: &Path, answer: &str, shown: &Value) -> (StatusCode, Value) {
    call(
        root,
        "POST",
        "/app/import/api/body-facet",
        Some(json!({ "answer": answer, "offer": shown })),
    )
    .await
}

fn stored(root: &Path) -> StoredBodyFacet {
    parse_body_facet(
        &read_journal_config(root)
            .unwrap()
            .config
            .unwrap_or_default(),
    )
}

fn remembered(root: &Path, choice: &BodyFacetChoice) {
    solstone_core_journal_config_write::record_body_facet(root, choice).unwrap();
}

fn enabled(root: &Path) -> Vec<String> {
    solstone_core_facets::observe_declared_facet_inventory(root)
        .unwrap()
        .enabled
}

#[tokio::test]
async fn the_card_reads_every_offer_state() {
    let none = journal(&["work"]);
    assert_eq!(
        offer_of(none.path()).await,
        ("ask_make_body".into(), json!("body"))
    );

    let health = journal(&["work", "health"]);
    assert_eq!(
        offer_of(health.path()).await,
        ("ask_existing".into(), json!("health"))
    );

    let both = journal(&["work", "health", "body"]);
    assert_eq!(
        offer_of(both.path()).await,
        ("ask_existing".into(), json!("body"))
    );

    let muted = journal(&["work", "body"]);
    solstone_core_facets::set_facet_muted(muted.path(), "body", true).unwrap();
    assert_eq!(
        offer_of(muted.path()).await,
        ("ask_unmute".into(), json!("body"))
    );

    let chosen = journal(&["work", "body"]);
    remembered(chosen.path(), &BodyFacetChoice::Facet("body".into()));
    assert_eq!(
        offer_of(chosen.path()).await,
        ("chosen".into(), json!("body"))
    );

    let declined = journal(&["work"]);
    remembered(declined.path(), &BodyFacetChoice::None);
    assert_eq!(
        offer_of(declined.path()).await,
        ("declined".into(), Value::Null)
    );

    // A remembered facet the owner has since muted asks again.
    let muted_default = journal(&["work", "body", "health"]);
    remembered(muted_default.path(), &BodyFacetChoice::Facet("body".into()));
    solstone_core_facets::set_facet_muted(muted_default.path(), "body", true).unwrap();
    assert_eq!(
        offer_of(muted_default.path()).await,
        ("ask_existing".into(), json!("health"))
    );

    // Every state carries the facets the owner can pick for one import, and
    // the words the card shows.
    let (_, body) = call(chosen.path(), "GET", "/app/import/api/body-facet", None).await;
    let names = body["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|facet| facet["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["body", "work"]);
    assert_eq!(body["copy"].as_object().unwrap().len(), COPY.len());
}

#[tokio::test]
async fn a_yes_makes_body_and_remembers_it() {
    let root = journal(&["work"]);
    let (status, body) = answer_with(root.path(), "yes").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offer"]["state"], "chosen");
    assert_eq!(enabled(root.path()), ["body", "work"]);
    assert_eq!(
        stored(root.path()),
        StoredBodyFacet::Answered(BodyFacetChoice::Facet("body".into()))
    );
}

#[tokio::test]
async fn a_yes_to_a_muted_body_turns_it_back_on() {
    let root = journal(&["work", "body"]);
    solstone_core_facets::set_facet_muted(root.path(), "body", true).unwrap();
    let (status, body) = answer_with(root.path(), "yes").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offer"]["state"], "chosen");
    assert_eq!(enabled(root.path()), ["body", "work"]);
}

#[tokio::test]
async fn a_yes_to_an_existing_facet_remembers_it_and_makes_nothing() {
    let root = journal(&["work", "health"]);
    answer_with(root.path(), "yes").await;
    assert_eq!(
        stored(root.path()),
        StoredBodyFacet::Answered(BodyFacetChoice::Facet("health".into()))
    );
    assert_eq!(enabled(root.path()), ["health", "work"]);
}

#[tokio::test]
async fn a_no_is_remembered_and_not_asked_again() {
    let root = journal(&["work"]);
    let (status, body) = answer_with(root.path(), "no").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offer"]["state"], "declined");
    assert_eq!(
        stored(root.path()),
        StoredBodyFacet::Answered(BodyFacetChoice::None)
    );
    assert_eq!(enabled(root.path()), ["work"]);
    // Answering again changes nothing, whichever question the page still shows.
    answer_with(root.path(), "yes").await;
    answer_shown(
        root.path(),
        "yes",
        &json!({"state": "ask_make_body", "facet": "body"}),
    )
    .await;
    assert_eq!(offer_of(root.path()).await.0, "declined");
    assert_eq!(enabled(root.path()), ["work"]);
    let (status, _) = answer_with(root.path(), "maybe").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// An answer to a question that has changed since the card showed it is
/// refused with nothing written, and the page is told to show it again; an
/// answer to the question asked now is taken.
#[tokio::test]
async fn an_answer_to_a_question_no_longer_asked_is_refused_and_writes_nothing() {
    let root = journal(&["work", "health"]);
    let (_, shown) = call(root.path(), "GET", "/app/import/api/body-facet", None).await;
    assert_eq!(
        shown["offer"],
        json!({"state": "ask_existing", "facet": "health"})
    );
    // health is muted before the owner clicks yes.
    solstone_core_facets::set_facet_muted(root.path(), "health", true).unwrap();
    let before = (
        stored(root.path()),
        solstone_core_facets::observe_declared_facet_inventory(root.path()).unwrap(),
    );

    let (status, body) = answer_shown(root.path(), "yes", &shown["offer"]).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason_code"], "body_facet_offer_changed");
    assert_eq!(body["error"], text("offer_changed"));
    assert_eq!(
        (
            stored(root.path()),
            solstone_core_facets::observe_declared_facet_inventory(root.path()).unwrap()
        ),
        before
    );
    assert!(!root.path().join("facets/body").exists());
    // A page that sends no question is refused the same way.
    let (status, _) = call(
        root.path(),
        "POST",
        "/app/import/api/body-facet",
        Some(json!({"answer": "no"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stored(root.path()), before.0);

    // The question the card shows now is answered.
    let (status, body) = answer_with(root.path(), "yes").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        stored(root.path()),
        StoredBodyFacet::Answered(BodyFacetChoice::Facet("body".into()))
    );
}

/// A Strava download staged on its card, as the upload leaves it.
fn staged_strava(root: &Path) -> (String, String) {
    let id = "20261010_120000";
    let path = root.join("imports").join(id).join("activities.csv");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "Activity ID\n").unwrap();
    let mut metadata = solstone_core_import::ImportMetadata::new();
    metadata.insert("file_path".into(), json!(path.to_str().unwrap()));
    metadata.insert("source_hint".into(), json!("strava"));
    metadata.insert("user_timestamp".into(), json!(id));
    solstone_core_import::write_import_metadata(root, id, &metadata).unwrap();
    (id.to_owned(), path.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn one_imports_facet_never_changes_the_remembered_one() {
    let root = journal(&["work", "body"]);
    remembered(root.path(), &BodyFacetChoice::Facet("body".into()));
    let (id, path) = staged_strava(root.path());
    for (choice, arguments) in [
        (
            json!({"choice": "facet", "facet": "work"}),
            vec!["--facet", "work"],
        ),
        (json!({"choice": "none"}), vec!["--no-facet"]),
    ] {
        let (status, body) = call(
            root.path(),
            "POST",
            "/app/import/api/meta",
            Some(json!({"path": path, "import_id": id, "body_facet": choice})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let metadata = solstone_core_import::read_import_metadata(root.path(), &id).unwrap();
        let command = crate::lifecycle::command(&path, &id, &metadata, false);
        assert!(
            command
                .windows(arguments.len())
                .any(|window| window == arguments.as_slice()),
            "{command:?}"
        );
        assert_eq!(
            stored(root.path()),
            StoredBodyFacet::Answered(BodyFacetChoice::Facet("body".into()))
        );
    }
    let (status, _) = call(
        root.path(),
        "POST",
        "/app/import/api/meta",
        Some(json!({"path": path, "import_id": id, "body_facet": {"choice": "maybe"}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn a_page_that_sends_no_facet_gives_the_importer_none() {
    // The mobile serving path and an older cached page send no facet field:
    // the importer gets no facet argument, so it uses the remembered default
    // while enabled, and never makes one.
    assert!(command_arguments(None).is_empty());
    assert!(command_arguments(Some(&Value::Null)).is_empty());
}
