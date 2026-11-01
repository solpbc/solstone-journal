// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use solstone_core_speaker_resolve::owner_line_labels::{
    LineSpeaker, OwnerLine, current_owner_line_entries, set_owner_line_labels,
};
use tower::ServiceExt;

use super::super::{ACTOR_HEADER, ACTOR_OWNER, routes};
use super::set_lines_checked;
use crate::test_support::{fixed_clock, phase_root, write};

const DAY: &str = "20260510";
const A: &str = "120000_300";
const B: &str = "120500_300";

fn person(root: &Path, id: &str, principal: bool) {
    write(
        &root.join("entities").join(id).join("entity.json"),
        &json!({"id":id,"name":id,"type":"Person","is_principal":principal}).to_string(),
    );
}

fn segment(root: &Path, stream: &str, key: &str) -> PathBuf {
    let dir = super::segment_dir(root, DAY, stream, key);
    let mut rows = vec![json!({"raw":"audio.flac"}).to_string()];
    rows.extend((1..=3).map(|id| {
        json!({"start":format!("00:00:0{id}"),"text":format!("line {id}"),"sentence_id":id})
            .to_string()
    }));
    write(&dir.join("audio.jsonl"), &(rows.join("\n") + "\n"));
    // Voice state the owner's answers must never touch.
    write(
        &dir.join("talents/speaker_labels.json"),
        r#"{"labels":[{"sentence_id":1,"speaker":"bo"}]}"#,
    );
    write(&dir.join("talents/speaker_corrections.json"), "[]");
    dir
}

fn activity(root: &Path, facet: &str, id: &str, stream: &str, segments: &[&str]) {
    let row = json!({"id":id,"activity":"meeting","title":id,"stream":stream,"segments":segments,"edits":[]});
    let _ = solstone_core_facets::append_activity_record(
        root,
        facet,
        DAY,
        row.as_object().unwrap().clone(),
    )
    .unwrap();
}

fn journal() -> tempfile::TempDir {
    let root = phase_root("established_empty");
    for facet in ["work", "home"] {
        solstone_core_facets::create_facet(root.path(), facet, facet, "", "", "", None).unwrap();
    }
    person(root.path(), "owner", true);
    for id in ["ada", "bo", "sam"] {
        person(root.path(), id, false);
    }
    write(
        &root.path().join("entities/owner/voiceprints.npz"),
        "voiceprints",
    );
    write(
        &root.path().join("awareness/current.json"),
        r#"{"voiceprint":{"state":"confirmed"}}"#,
    );
    drop(solstone_core_entity::hold_entity_trust_lock(root.path()).unwrap());
    for key in [A, B] {
        segment(root.path(), "mic", key);
    }
    activity(root.path(), "work", "rec", "mic", &[A, B]);
    root
}

fn line(root: &Path, key: &str, sentence_id: i64) -> OwnerLine {
    OwnerLine {
        segment_dir: super::segment_dir(root, DAY, "mic", key),
        source: "audio".to_owned(),
        sentence_id,
    }
}

fn lines(keys: &[(&str, i64)]) -> Value {
    keys.iter()
        .map(|(key, id)| json!({"stream":"mic","segment":key,"source":"audio","sentence_id":id}))
        .collect()
}

async fn post(root: &Path, path: &str, actor: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!(
            "/app/activities/api/day/{DAY}/record/rec/{path}?facet=work"
        ))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(actor) = actor {
        request = request.header(ACTOR_HEADER, actor);
    }
    let router: Router = solstone_core_convey_shell::session_gate::apply_layer(
        routes(root.to_path_buf(), fixed_clock()),
        root.to_path_buf(),
    );
    let response = router
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

async fn answer(root: &Path, keys: &[(&str, i64)], speaker: Value) -> Value {
    let (status, body) = post(
        root,
        "lines",
        Some(ACTOR_OWNER),
        json!({"lines":lines(keys),"speaker":speaker}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn remove(root: &Path, position: usize) -> (StatusCode, Value) {
    let record = solstone_core_facets::get_activity_record(root, "work", DAY, "rec")
        .unwrap()
        .unwrap();
    let edit = &record["edits"][position];
    post(
        root,
        "edits/remove",
        Some(ACTOR_OWNER),
        json!({"position":position,"timestamp":edit["timestamp"],"actor":edit["actor"]}),
    )
    .await
}

/// The owner's answer for a line: `None` when there is none.
fn speaker(root: &Path, key: &str, sentence_id: i64) -> Option<Option<String>> {
    entry(root, key, sentence_id).map(|entry| entry.0)
}

fn entry(root: &Path, key: &str, sentence_id: i64) -> Option<(Option<String>, String)> {
    current_owner_line_entries(&super::segment_dir(root, DAY, "mic", key))
        .unwrap()
        .into_iter()
        .find(|entry| entry.key() == ("audio", sentence_id))
        .map(|entry| (entry.speaker, entry.set_at))
}

/// Answer a line the way the transcript page does.
fn transcript_answer(root: &Path, key: &str, sentence_id: i64, id: &str) -> String {
    set_owner_line_labels(
        root,
        &[line(root, key, sentence_id)],
        &LineSpeaker::Person(id.to_owned()),
    )
    .unwrap()[0]
        .set_at
        .clone()
}

fn owner_file(root: &Path, key: &str) -> Option<Vec<u8>> {
    std::fs::read(super::segment_dir(root, DAY, "mic", key).join("talents/owner_line_labels.json"))
        .ok()
}

fn day_bytes(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(format!("facets/work/activities/{DAY}.jsonl"))).unwrap()
}

fn asks(root: &Path) -> Vec<solstone_core_system::owner_ask::OwnerAsk> {
    solstone_core_system::owner_ask::read(root)
        .unwrap()
        .into_values()
        .collect()
}

fn ada() -> Value {
    json!({"kind":"person","id":"ada"})
}

fn merge(root: &Path, source: &str, target: &str) {
    solstone_core_entity::commit_entity_merge(
        root,
        source,
        target,
        solstone_core_entity::EntityMergeOptions::default(),
        &solstone_core_entity::EncoderIdentity {
            id: "test".into(),
            sha256: "a".repeat(64),
            width: 256,
        },
    )
    .unwrap();
}

#[tokio::test]
async fn lines_across_two_segments_are_answered_with_one_edit_and_ask_their_stream_only() {
    let root = journal();
    // The same segment key on another stream, and another facet on this one.
    segment(root.path(), "phone", A);
    activity(root.path(), "home", "phone_rec", "phone", &[A]);
    activity(root.path(), "home", "home_rec", "mic", &[A]);
    let body = answer(root.path(), &[(A, 1), (B, 2)], ada()).await;
    assert_eq!(body["asked"], true);
    let edits = body["record"]["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["actor"], "owner:lines");
    assert_eq!(edits[0]["speaker"], ada());
    let recorded = edits[0]["lines"].as_array().unwrap();
    assert_eq!(recorded.len(), 2);
    for (recorded, (key, id)) in recorded.iter().zip([(A, 1), (B, 2)]) {
        assert_eq!(recorded["segment"], key);
        assert_eq!(recorded["sentence_id"], id);
        assert_eq!(recorded["prior"], Value::Null);
        assert_eq!(
            entry(root.path(), key, id),
            Some((
                Some("ada".into()),
                recorded["set_at"].as_str().unwrap().into()
            ))
        );
    }
    let asks = asks(root.path());
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].day, DAY);
    assert!(asks[0].thaw);
    let named = asks[0]
        .to_seed
        .iter()
        .map(|asked| (asked.facet.as_str(), asked.activity.as_str()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        named,
        BTreeSet::from([("home", "home_rec"), ("work", "rec")])
    );
}

#[tokio::test]
async fn a_refused_segment_writes_no_answer_and_no_edit() {
    let root = journal();
    transcript_answer(root.path(), A, 1, "bo");
    // The second segment's transcript views no longer agree on one source.
    write(
        &super::segment_dir(root.path(), DAY, "mic", B).join("mic_audio.jsonl"),
        "{}\n",
    );
    let before = (owner_file(root.path(), A), owner_file(root.path(), B));
    let day = day_bytes(root.path());
    let (status, body) = post(
        root.path(),
        "lines",
        Some(ACTOR_OWNER),
        json!({"lines":lines(&[(A, 1), (B, 1)]),"speaker":ada()}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason_code"], "owner_line_source_unsupported");
    assert_eq!(
        (owner_file(root.path(), A), owner_file(root.path(), B)),
        before
    );
    assert_eq!(day_bytes(root.path()), day);
    assert!(asks(root.path()).is_empty());
}

#[tokio::test]
async fn an_edit_that_cannot_be_written_puts_the_answers_back() {
    let root = journal();
    transcript_answer(root.path(), A, 1, "bo");
    let before = (owner_file(root.path(), A), owner_file(root.path(), B));
    assert!(before.1.is_none());
    let day = day_bytes(root.path());
    let body = json!({"lines":lines(&[(A, 1), (B, 2)]),"speaker":ada()}).to_string();
    let response = set_lines_checked(
        root.path(),
        &fixed_clock(),
        DAY,
        "rec",
        "work",
        body.as_bytes(),
        || Err("the activity edit failed".to_owned()),
    );
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        (owner_file(root.path(), A), owner_file(root.path(), B)),
        before
    );
    assert_eq!(day_bytes(root.path()), day);
    assert!(asks(root.path()).is_empty());
}

#[tokio::test]
async fn removal_order_never_brings_back_a_retracted_speaker() {
    for (first, after_first) in [(0, Some(None)), (1, Some(Some("ada".to_owned())))] {
        let root = journal();
        answer(root.path(), &[(A, 1)], ada()).await;
        answer(root.path(), &[(A, 1)], json!({"kind":"unknown"})).await;
        let (status, body) = remove(root.path(), first).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["asked"], true);
        assert_eq!(speaker(root.path(), A, 1), after_first, "{first}");
        let (status, _) = remove(root.path(), 1 - first).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(speaker(root.path(), A, 1), None, "{first}");
    }
}

#[tokio::test]
async fn removing_an_edit_restores_the_answer_it_replaced() {
    let root = journal();
    let token = transcript_answer(root.path(), A, 1, "bo");
    answer(root.path(), &[(A, 1)], ada()).await;
    remove(root.path(), 0).await;
    assert_eq!(entry(root.path(), A, 1), Some((Some("bo".into()), token)));
}

#[tokio::test]
async fn a_merge_never_strands_a_removal() {
    let root = journal();
    answer(root.path(), &[(A, 1)], ada()).await;
    merge(root.path(), "ada", "sam");
    assert_eq!(speaker(root.path(), A, 1), Some(Some("sam".into())));
    let (status, body) = remove(root.path(), 0).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(speaker(root.path(), A, 1), None);
    assert!(body["record"]["edits"][0]["removed"].is_string());
}

#[tokio::test]
async fn a_replaced_answer_naming_someone_merged_away_comes_back_as_the_survivor() {
    let root = journal();
    transcript_answer(root.path(), A, 1, "ada");
    answer(root.path(), &[(A, 1)], json!({"kind":"person","id":"bo"})).await;
    merge(root.path(), "ada", "sam");
    remove(root.path(), 0).await;
    assert_eq!(speaker(root.path(), A, 1), Some(Some("sam".into())));
}

#[tokio::test]
async fn a_later_transcript_answer_outlives_the_removal() {
    let root = journal();
    answer(root.path(), &[(A, 1)], ada()).await;
    let token = transcript_answer(root.path(), A, 1, "bo");
    let (status, _) = remove(root.path(), 0).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(entry(root.path(), A, 1), Some((Some("bo".into()), token)));
}

#[tokio::test]
async fn refusals_change_nothing() {
    let root = journal();
    answer(root.path(), &[(A, 2)], ada()).await;
    write(
        &super::segment_dir(root.path(), DAY, "phone", A).join("audio.jsonl"),
        "{}\n",
    );
    let snapshot = |root: &Path| {
        (
            day_bytes(root),
            owner_file(root, A),
            owner_file(root, B),
            asks(root).len(),
        )
    };
    let before = snapshot(root.path());
    let body = |lines: Value, speaker: Value| json!({"lines":lines,"speaker":speaker});
    let other_stream = json!([{"stream":"phone","segment":A,"source":"audio","sentence_id":1}]);
    for (actor, body, status, reason) in [
        (
            None,
            body(lines(&[(A, 1)]), ada()),
            StatusCode::FORBIDDEN,
            "activity_owner_only",
        ),
        (
            Some("owner:update"),
            body(lines(&[(A, 1)]), ada()),
            StatusCode::FORBIDDEN,
            "activity_owner_only",
        ),
        (
            Some(ACTOR_OWNER),
            body(json!([]), ada()),
            StatusCode::BAD_REQUEST,
            "owner_line_empty",
        ),
        (
            Some(ACTOR_OWNER),
            json!({"speaker":ada()}),
            StatusCode::BAD_REQUEST,
            "owner_line_invalid",
        ),
        (
            Some(ACTOR_OWNER),
            body(lines(&[(A, 1), ("121000_300", 1)]), ada()),
            StatusCode::BAD_REQUEST,
            "owner_line_outside_activity",
        ),
        (
            Some(ACTOR_OWNER),
            body(other_stream, ada()),
            StatusCode::BAD_REQUEST,
            "owner_line_outside_activity",
        ),
        (
            Some(ACTOR_OWNER),
            body(lines(&[(A, 1)]), json!({"kind":"robot"})),
            StatusCode::BAD_REQUEST,
            "owner_line_speaker_invalid",
        ),
        (
            Some(ACTOR_OWNER),
            body(lines(&[(A, 1)]), json!({"kind":"person"})),
            StatusCode::BAD_REQUEST,
            "owner_line_speaker_invalid",
        ),
        (
            Some(ACTOR_OWNER),
            body(lines(&[(A, 1)]), json!({"kind":"person","id":"zed"})),
            StatusCode::CONFLICT,
            "owner_line_person_unavailable",
        ),
        (
            Some(ACTOR_OWNER),
            body(lines(&[(A, 1), (B, 9)]), ada()),
            StatusCode::CONFLICT,
            "owner_line_not_found",
        ),
    ] {
        let (got, reply) = post(root.path(), "lines", actor, body.clone()).await;
        assert_eq!(got, status, "{body}");
        assert_eq!(reply["reason_code"], reason, "{body}");
        assert_eq!(snapshot(root.path()), before, "{body}");
    }

    // Only the owner removes a lines edit, and a removal the store would
    // refuse removes nothing.
    let record = solstone_core_facets::get_activity_record(root.path(), "work", DAY, "rec")
        .unwrap()
        .unwrap();
    let edit =
        json!({"position":0,"timestamp":record["edits"][0]["timestamp"],"actor":"owner:lines"});
    let (got, reply) = post(root.path(), "edits/remove", None, edit.clone()).await;
    assert_eq!(got, StatusCode::FORBIDDEN);
    assert_eq!(reply["reason_code"], "activity_owner_only");
    let file =
        super::segment_dir(root.path(), DAY, "mic", A).join("talents/owner_line_labels.json");
    std::fs::write(&file, "not an answer").unwrap();
    let before = snapshot(root.path());
    let (got, reply) = post(root.path(), "edits/remove", Some(ACTOR_OWNER), edit).await;
    assert_eq!(got, StatusCode::CONFLICT);
    assert_eq!(reply["reason_code"], "owner_line_answers_unreadable");
    assert_eq!(snapshot(root.path()), before);
}

/// Every file under `root` with its bytes, leaving out lock files.
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if !path.to_string_lossy().ends_with(".lock") {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[tokio::test]
async fn answering_and_removing_touch_only_answers_the_activity_and_asks() {
    let root = journal();
    let before = files(root.path());
    answer(root.path(), &[(A, 1), (B, 1)], ada()).await;
    let (status, _) = remove(root.path(), 0).await;
    assert_eq!(status, StatusCode::OK);
    let after = files(root.path());
    let changed = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        format!("chronicle/{DAY}/mic/{A}/talents/owner_line_labels.json"),
        format!("chronicle/{DAY}/mic/{B}/talents/owner_line_labels.json"),
        format!("facets/work/activities/{DAY}.jsonl"),
        "health/owner-asks.json".to_owned(),
    ]);
    assert_eq!(changed, expected);
}

#[tokio::test]
async fn the_day_view_shows_an_answer_correcting_its_activity_while_it_stands() {
    let root = journal();
    let view = |root: PathBuf| async move {
        let router: Router = solstone_core_convey_shell::session_gate::apply_layer(
            routes(root.clone(), fixed_clock()),
            root,
        );
        let response = router
            .oneshot(
                Request::builder()
                    .uri(format!("/app/activities/api/day/{DAY}/view"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "rec")
            .unwrap()
            .clone()
    };
    assert_eq!(view(root.path().to_path_buf()).await["corrected"], false);
    answer(root.path(), &[(A, 1), (B, 2)], ada()).await;
    let item = view(root.path().to_path_buf()).await;
    assert_eq!(item["corrected"], true);
    let edit = &item["edits"][0];
    assert_eq!(edit["kind"], "owner_lines");
    assert_eq!(edit["lines"], 2);
    assert_eq!(edit["speaker"], ada());
    assert_eq!(edit["position"], 0);
    assert_eq!(edit["removable"], true);
    assert_eq!(edit["removed"], false);
    // Still corrected while one of its answers stands.
    transcript_answer(root.path(), A, 1, "bo");
    assert_eq!(view(root.path().to_path_buf()).await["corrected"], true);
    transcript_answer(root.path(), B, 2, "bo");
    assert_eq!(view(root.path().to_path_buf()).await["corrected"], false);
    remove(root.path(), 0).await;
    let item = view(root.path().to_path_buf()).await;
    assert_eq!(item["edits"][0]["removable"], false);
    assert_eq!(item["edits"][0]["removed"], true);
    assert_eq!(item["corrected"], false);
}
